use anyhow::Result;
use codex_api::AuthError;
use codex_api::SharedAuthProvider;
use codex_features::RuntimeAuthMode;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_http_client::RequestBuilder;
use codex_http_client::RouteAwareClientPool;
use codex_login::CodexAuth;
use codex_login::default_client::get_codex_user_agent;
use codex_model_provider::ClientAuth;
use codex_runtime_agent_instance::RuntimeAuthManager;
use http::Method;
use http::StatusCode;
use http::header::CONTENT_TYPE;
use http::header::HeaderMap;
use http::header::HeaderValue;
use http::header::USER_AGENT;
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::Arc;

pub(crate) mod turn_usage;

#[derive(Debug)]
pub enum RequestError {
    Policy(codex_http_client::NetworkPolicyDenied),
    UnexpectedStatus {
        method: String,
        url: String,
        status: StatusCode,
        content_type: String,
        body: String,
    },
    Other(anyhow::Error),
}

impl RequestError {
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::UnexpectedStatus { status, .. } => Some(*status),
            Self::Policy(_) | Self::Other(_) => None,
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(denied) => denied.fmt(f),
            Self::UnexpectedStatus {
                method,
                url,
                status,
                content_type,
                body,
                ..
            } => write!(
                f,
                "{method} {url} failed: {status}; content-type={content_type}; body={body}"
            ),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Policy(denied) => Some(denied),
            Self::UnexpectedStatus { .. } => None,
            Self::Other(err) => Some(err.as_ref()),
        }
    }
}

impl From<anyhow::Error> for RequestError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl From<AuthError> for RequestError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Policy(denied) => Self::Policy(denied),
            other => Self::Other(other.into()),
        }
    }
}

impl From<codex_http_client::HttpError> for RequestError {
    fn from(error: codex_http_client::HttpError) -> Self {
        match error {
            codex_http_client::HttpError::Policy(denied) => Self::Policy(denied),
            error @ (codex_http_client::HttpError::Request(_)
            | codex_http_client::HttpError::Route(_)
            | codex_http_client::HttpError::Build(_)
            | codex_http_client::HttpError::UnsupportedRedirectScheme(_)
            | codex_http_client::HttpError::TooManyRedirects
            | codex_http_client::HttpError::Timeout) => Self::Other(error.into()),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    base_url: String,
    http: RouteAwareClientPool,
    auth: ClientAuth,
    user_agent: Option<HeaderValue>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("auth", &"<provider>")
            .field("user_agent", &self.user_agent)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(base_url: impl Into<String>, http_client_factory: HttpClientFactory) -> Self {
        let http = RouteAwareClientPool::new_without_request_logging(
            http_client_factory,
            ClientRouteClass::Api,
        );
        Self::with_http(base_url.into(), http)
    }

    /// Creates a client that never forwards its credentials to a redirect destination.
    pub fn new_without_redirects(
        base_url: impl Into<String>,
        http_client_factory: HttpClientFactory,
    ) -> Self {
        let http = RouteAwareClientPool::new_without_redirects_or_request_logging(
            http_client_factory,
            ClientRouteClass::Api,
        );
        Self::with_http(base_url.into(), http)
    }

    fn with_http(base_url: String, http: RouteAwareClientPool) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            http,
            auth: ClientAuth::new(codex_model_provider::unauthenticated_auth_provider()),
            user_agent: None,
        }
    }

    pub fn from_auth(
        base_url: impl Into<String>,
        auth: &CodexAuth,
        http_client_factory: HttpClientFactory,
    ) -> Self {
        let mut client =
            Self::new(base_url, http_client_factory).with_user_agent(get_codex_user_agent());
        client.auth = ClientAuth::new(codex_model_provider::auth_provider_from_auth(auth));
        client
    }

    /// Build a client using the captured credential and the request's organization/project headers.
    pub async fn from_auth_with_runtime(
        base_url: impl Into<String>,
        auth: &CodexAuth,
        http_client_factory: HttpClientFactory,
        runtime: Option<&(Arc<RuntimeAuthManager>, RuntimeAuthMode)>,
        headers: &HeaderMap,
    ) -> Result<Self> {
        let base_url = base_url.into();
        let runtime = codex_model_provider::runtime_auth_for_url(runtime, &base_url)?;
        Self::from_auth_with_selected_runtime(
            base_url,
            auth,
            http_client_factory,
            runtime,
            headers,
            Self::new,
        )
        .await
    }

    /// Build an authenticated client without following redirects.
    pub async fn from_auth_with_runtime_without_redirects(
        base_url: impl Into<String>,
        auth: &CodexAuth,
        http_client_factory: HttpClientFactory,
        runtime: Option<&(Arc<RuntimeAuthManager>, RuntimeAuthMode)>,
        headers: &HeaderMap,
    ) -> Result<Self> {
        let base_url = base_url.into();
        let runtime = codex_model_provider::runtime_auth_for_url(runtime, &base_url)?;
        Self::from_auth_with_selected_runtime(
            base_url,
            auth,
            http_client_factory,
            runtime,
            headers,
            Self::new_without_redirects,
        )
        .await
    }

    async fn from_auth_with_selected_runtime(
        base_url: String,
        auth: &CodexAuth,
        http_client_factory: HttpClientFactory,
        runtime: Option<&(Arc<RuntimeAuthManager>, RuntimeAuthMode)>,
        headers: &HeaderMap,
        new_client: fn(String, HttpClientFactory) -> Self,
    ) -> Result<Self> {
        let (auth, http_client_factory) =
            ClientAuth::for_auth(auth, runtime, http_client_factory, headers).await?;
        let mut client = new_client(base_url, http_client_factory);
        client.auth = auth;
        Ok(client.with_user_agent(get_codex_user_agent()))
    }

    /// Configure provider auth, returning an error for a runtime-bound client.
    pub fn with_auth_provider(mut self, auth: SharedAuthProvider) -> Result<Self> {
        self.auth.set_provider(auth)?;
        Ok(self)
    }

    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        if let Ok(hv) = HeaderValue::from_str(&ua.into()) {
            self.user_agent = Some(hv);
        }
        self
    }

    async fn headers_with(&self, mut h: HeaderMap) -> Result<HeaderMap, RequestError> {
        if let Some(ua) = &self.user_agent {
            h.insert(USER_AGENT, ua.clone());
        } else {
            h.insert(USER_AGENT, HeaderValue::from_static("codex-cli"));
        }

        let selected = self.auth.resolve(&h).await?;
        h = selected.auth.apply_auth_headers(h).await?;
        selected
            .validate_headers(&h)
            .map_err(|error| RequestError::Other(error.into()))?;
        Ok(h)
    }

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        self.http.request(method, url)
    }

    async fn exec_request_detailed(
        &self,
        req: RequestBuilder,
        method: &str,
        url: &str,
    ) -> std::result::Result<(String, String), RequestError> {
        self.auth
            .validate_destination(&self.base_url, url)
            .map_err(|error| RequestError::Other(error.into()))?;
        let res = req.send().await?;
        let status = res.status();
        let content_type = res
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = res.text().await?;
        if !status.is_success() {
            return Err(RequestError::UnexpectedStatus {
                method: method.to_string(),
                url: url.to_string(),
                status,
                content_type,
                body,
            });
        }
        Ok((body, content_type))
    }

    fn decode_json<T: DeserializeOwned>(&self, url: &str, ct: &str, body: &str) -> Result<T> {
        match serde_json::from_str::<T>(body) {
            Ok(v) => Ok(v),
            Err(e) => {
                anyhow::bail!("Decode error for {url}: {e}; content-type={ct}; body={body}");
            }
        }
    }
}

#[cfg(test)]
#[path = "client/runtime_auth_tests.rs"]
mod runtime_auth_tests;
