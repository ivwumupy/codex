//! Registry of model providers supported by Codex.
//!
//! Providers can be defined in two places:
//!   1. Built-in defaults compiled into the binary so Codex works out-of-the-box.
//!   2. User-defined entries inside `~/.codex/config.toml` under the `model_providers`
//!      key. These override or extend the defaults at runtime.
//!
//! API provider construction applies the process-wide managed residency policy also
//! used by default HTTP headers.

use codex_client::OPENAI_ORGANIZATION_HEADER;
use codex_client::OPENAI_PROJECT_HEADER;
use codex_client::Provider as ApiProvider;
use codex_client::RetryConfig as ApiRetryConfig;
use codex_protocol::auth::AuthMode;
use codex_protocol::config_types::ModelProviderAuthInfo;
use codex_protocol::error::CodexErr;
use codex_protocol::error::EnvVarError;
use codex_protocol::error::Result as CodexResult;
use codex_utils_redacted_string::RedactedString;
use http::HeaderMap;
use http::header::HeaderName;
use http::header::HeaderValue;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU64;
use std::path::Component;
use std::path::Path;
use std::sync::PoisonError;
use std::sync::RwLock;
use std::time::Duration;

mod capabilities;
mod gateway_oauth;
mod oauth_session;
pub use capabilities::ModelProviderCapabilities;
pub use capabilities::RemoteCompactionSupport;
pub use gateway_oauth::GatewayOAuthConfig;
pub use gateway_oauth::GatewayOAuthDelivery;
pub use gateway_oauth::GatewayOAuthSettings;
pub use oauth_session::OAuthCredentialsStore;
pub use oauth_session::OAuthSessionConfig;

pub const RESIDENCY_HEADER_NAME: &str = "x-openai-internal-codex-residency";

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResidencyRequirement {
    Us,
}

static REQUIREMENTS_RESIDENCY: RwLock<Option<ResidencyRequirement>> = RwLock::new(None);

/// Sets the process-wide residency requirement loaded from managed configuration.
pub fn set_managed_residency_requirement(enforce_residency: Option<ResidencyRequirement>) {
    // Recover the stored policy if the lock is poisoned rather than silently disabling it.
    *REQUIREMENTS_RESIDENCY
        .write()
        .unwrap_or_else(PoisonError::into_inner) = enforce_residency;
}

/// Returns the current process-wide managed residency requirement.
pub fn read_managed_residency_requirement() -> Option<ResidencyRequirement> {
    *REQUIREMENTS_RESIDENCY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_STREAM_MAX_RETRIES: u64 = 5;
const DEFAULT_REQUEST_MAX_RETRIES: u64 = 4;
const DEFAULT_CREDENTIAL_EXPORT_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Hard cap for user-configured `stream_max_retries`.
const MAX_STREAM_MAX_RETRIES: u64 = 100;
/// Hard cap for user-configured `request_max_retries`.
const MAX_REQUEST_MAX_RETRIES: u64 = 100;

pub const OPENAI_PROVIDER_NAME: &str = "OpenAI";
const OPENAI_ACTOR_AUTHORIZATION_HEADER: &str = "x-openai-actor-authorization";
pub const OPENAI_PROVIDER_ID: &str = "openai";
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const AMAZON_BEDROCK_PROVIDER_NAME: &str = "Amazon Bedrock";
pub const AMAZON_BEDROCK_PROVIDER_ID: &str = "amazon-bedrock";
pub const AMAZON_BEDROCK_RUNTIME_PROVIDER_NAME: &str = "Amazon Bedrock Runtime";
pub const AMAZON_BEDROCK_RUNTIME_PROVIDER_ID: &str = "amazon-bedrock-runtime";
pub const GCP_PROVIDER_NAME: &str = "Google Cloud Vertex AI";
pub const GCP_PROVIDER_ID: &str = "gcp";
const CHAT_WIRE_API_REMOVED_ERROR: &str = "`wire_api = \"chat\"` is no longer supported.\nHow to fix: set `wire_api = \"responses\"` in your provider config.\nMore info: https://github.com/openai/codex/discussions/7782";

/// Wire protocol that the provider speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    /// The Responses API exposed by OpenAI at `/v1/responses`.
    #[default]
    Responses,
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Responses => "responses",
        };
        f.write_str(value)
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "chat" => Err(serde::de::Error::custom(CHAT_WIRE_API_REMOVED_ERROR)),
            _ => Err(serde::de::Error::unknown_variant(&value, &["responses"])),
        }
    }
}

/// Serializable representation of a provider definition.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderInfo {
    /// Friendly display name.
    #[serde(default)]
    pub name: String,
    /// Base URL for the provider's OpenAI-compatible API.
    pub base_url: Option<String>,
    /// Optional URL for a Codex-format model catalog, fetched using provider credentials.
    /// The built-in OpenAI provider accepts HTTPS URLs on OpenAI API hosts.
    /// Use a custom provider for a catalog hosted elsewhere.
    pub model_catalog_url: Option<RedactedString>,
    /// Environment variable that stores the user's API key for this provider.
    pub env_key: Option<String>,

    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub env_key_instructions: Option<String>,
    /// Value to use with `Authorization: Bearer <token>` header. Use of this
    /// config is discouraged in favor of `env_key` for security reasons, but
    /// this may be necessary when using this programmatically.
    pub experimental_bearer_token: Option<RedactedString>,
    /// Command-backed bearer-token configuration for this provider.
    pub auth: Option<ModelProviderAuthInfo>,
    /// Gateway access token from OAuth or a credential export command.
    pub gateway_oauth: Option<GatewayOAuthConfig>,
    /// Which wire protocol this provider expects.
    #[serde(default)]
    pub wire_api: WireApi,
    /// Optional API capability overrides for a custom Responses-compatible provider.
    /// Unspecified capabilities retain their existing provider defaults.
    pub capabilities: Option<ModelProviderCapabilities>,
    /// Optional query parameters to append to the base URL.
    pub query_params: Option<HashMap<String, RedactedString>>,
    /// Additional HTTP headers to include in requests to this provider where
    /// the (key, value) pairs are the header name and value.
    pub http_headers: Option<HashMap<String, RedactedString>>,
    /// Optional HTTP headers to include in requests to this provider where the
    /// (key, value) pairs are the header name and _environment variable_ whose
    /// value should be used. If the environment variable is not set, or the
    /// value is empty, the header will not be included in the request.
    pub env_http_headers: Option<HashMap<String, String>>,
    /// Maximum number of times to retry a failed HTTP request to this provider.
    pub request_max_retries: Option<u64>,
    /// Number of times to retry reconnecting a dropped streaming response before failing.
    pub stream_max_retries: Option<u64>,
    /// Idle timeout (in milliseconds) to wait for activity on a streaming response before treating
    /// the connection as lost.
    pub stream_idle_timeout_ms: Option<u64>,
    /// Maximum time (in milliseconds) to wait for a websocket connection attempt before treating
    /// it as failed.
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Does this provider require an OpenAI API key or ChatGPT login token? If true,
    /// the user is presented with a login screen on first run, and credentials are
    /// stored using the backend selected by `cli_auth_credentials_store`.
    /// If false (the default), the login screen is skipped, and the API key
    /// (if needed) comes from the environment variable specified by `env_key`.
    #[serde(default)]
    pub requires_openai_auth: bool,
    /// Whether this provider supports the Responses API WebSocket transport.
    #[serde(default)]
    pub supports_websockets: bool,
    /// Whether this provider supports the standalone web-search endpoint.
    #[serde(default)]
    pub supports_standalone_web_search: bool,
    /// Runtime-only opt-in for internal metadata, independent of the destination check.
    /// This cannot be loaded from or written to serialized provider configuration.
    #[serde(skip)]
    #[schemars(skip)]
    pub include_internal_metadata: bool,
}

/// Command used to export credentials for a model provider.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CredentialExportConfig {
    /// Executable to invoke directly, without a shell.
    pub command: String,
    /// Arguments passed to the credential export command.
    #[serde(default)]
    pub args: Vec<RedactedString>,
    /// Maximum time to wait for the credential export command to complete.
    #[serde(default = "default_credential_export_timeout_ms")]
    pub timeout_ms: NonZeroU64,
}

impl CredentialExportConfig {
    pub fn validate(&self, field: &str) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err(format!("provider {field}.command must not be empty"));
        }
        let path = Path::new(&self.command);
        let mut components = path.components();
        let bare = matches!((components.next(), components.next()),
            (Some(Component::Normal(name)), None) if name == path.as_os_str());
        if !path.is_absolute() && !bare {
            return Err(format!(
                "provider {field}.command must be an absolute path or a bare executable name"
            ));
        }
        Ok(())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.get())
    }
}

fn default_credential_export_timeout_ms() -> NonZeroU64 {
    match NonZeroU64::new(DEFAULT_CREDENTIAL_EXPORT_TIMEOUT_MS) {
        Some(timeout_ms) => timeout_ms,
        None => panic!("credential export timeout must be non-zero"),
    }
}

impl ModelProviderInfo {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(gateway) = &self.gateway_oauth {
            gateway.validate(self)?;
        }
        let Some(auth) = self.auth.as_ref() else {
            return Ok(());
        };

        if auth.command.trim().is_empty() {
            return Err("provider auth.command must not be empty".to_string());
        }

        let mut conflicts = Vec::new();
        if self.env_key.is_some() {
            conflicts.push("env_key");
        }
        if self.experimental_bearer_token.is_some() {
            conflicts.push("experimental_bearer_token");
        }
        if self.requires_openai_auth {
            conflicts.push("requires_openai_auth");
        }

        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "provider auth cannot be combined with {}",
                conflicts.join(", ")
            ))
        }
    }

    /// Resolve configured request headers before choosing credentials or a runtime.
    pub fn build_header_map(&self) -> CodexResult<HeaderMap> {
        let capacity = self.http_headers.as_ref().map_or(0, HashMap::len)
            + self.env_http_headers.as_ref().map_or(0, HashMap::len);
        let mut headers = HeaderMap::with_capacity(capacity);
        if let Some(extra) = &self.http_headers {
            for (k, v) in extra {
                if let (Ok(name), Ok(value)) =
                    (HeaderName::try_from(k), HeaderValue::try_from(v.as_str()))
                {
                    headers.insert(name, value);
                }
            }
        }

        if let Some(env_headers) = &self.env_http_headers {
            for (header, env_var) in env_headers {
                if let Ok(val) = std::env::var(env_var)
                    && !val.trim().is_empty()
                    && let (Ok(name), Ok(value)) =
                        (HeaderName::try_from(header), HeaderValue::try_from(val))
                {
                    headers.insert(name, value);
                }
            }
        }

        Ok(headers)
    }

    /// Builds an API provider with managed residency taking precedence over configured headers.
    pub fn to_api_provider(&self, _auth_mode: Option<AuthMode>) -> CodexResult<ApiProvider> {
        let base_url = self
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let mut headers = self.build_header_map()?;
        if let Some(requirement) = read_managed_residency_requirement() {
            let value = match requirement {
                ResidencyRequirement::Us => HeaderValue::from_static("us"),
            };
            headers.insert(RESIDENCY_HEADER_NAME, value);
        }
        let retry = ApiRetryConfig {
            max_attempts: self.request_max_retries(),
            base_delay: Duration::from_millis(200),
            retry_429: false,
            retry_5xx: true,
            retry_transport: true,
        };

        Ok(ApiProvider {
            name: self.name.clone(),
            base_url,
            query_params: self.query_params.clone().map(|params| {
                params
                    .into_iter()
                    .map(|(name, value)| (name, value.into_inner()))
                    .collect()
            }),
            headers,
            retry,
            stream_idle_timeout: self.stream_idle_timeout(),
        })
    }

    /// If `env_key` is Some, returns the API key for this provider if present
    /// (and non-empty) in the environment. If `env_key` is required but
    /// cannot be found, returns an error.
    pub fn api_key(&self) -> CodexResult<Option<String>> {
        match &self.env_key {
            Some(env_key) => {
                let api_key = std::env::var(env_key)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        CodexErr::EnvVar(EnvVarError {
                            var: env_key.clone(),
                            instructions: self.env_key_instructions.clone(),
                        })
                    })?;
                Ok(Some(api_key))
            }
            None => Ok(None),
        }
    }

    /// Effective maximum number of request retries for this provider.
    pub fn request_max_retries(&self) -> u64 {
        self.request_max_retries
            .unwrap_or(DEFAULT_REQUEST_MAX_RETRIES)
            .min(MAX_REQUEST_MAX_RETRIES)
    }

    /// Effective maximum number of stream reconnection attempts for this provider.
    pub fn stream_max_retries(&self) -> u64 {
        self.stream_max_retries
            .unwrap_or(DEFAULT_STREAM_MAX_RETRIES)
            .min(MAX_STREAM_MAX_RETRIES)
    }

    /// Effective idle timeout for streaming responses.
    pub fn stream_idle_timeout(&self) -> Duration {
        self.stream_idle_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_STREAM_IDLE_TIMEOUT_MS))
    }

    /// Effective timeout for websocket connect attempts.
    pub fn websocket_connect_timeout(&self) -> Duration {
        self.websocket_connect_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS))
    }

    pub fn create_openai_provider(base_url: Option<String>) -> ModelProviderInfo {
        ModelProviderInfo {
            name: OPENAI_PROVIDER_NAME.into(),
            base_url,
            model_catalog_url: None,
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            gateway_oauth: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(
                [("version".to_string(), env!("CARGO_PKG_VERSION").into())]
                    .into_iter()
                    .collect(),
            ),
            env_http_headers: Some(
                [
                    (
                        OPENAI_ORGANIZATION_HEADER.to_string(),
                        "OPENAI_ORGANIZATION".to_string(),
                    ),
                    (
                        OPENAI_PROJECT_HEADER.to_string(),
                        "OPENAI_PROJECT".to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            // Use global defaults for retry/timeout unless overridden in config.toml.
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: true,
            supports_websockets: true,
            supports_standalone_web_search: true,
            capabilities: None,
            include_internal_metadata: true,
        }
    }

    /// Whether explicit Cyber programs may be selected and forwarded to this provider.
    /// Use the configured provider ID, never its user-controlled display name, for defaults.
    pub fn supports_cyber_access_programs(&self, provider_id: &str) -> bool {
        self.capabilities
            .and_then(|capabilities| capabilities.cyber_access_programs)
            .unwrap_or(provider_id == OPENAI_PROVIDER_ID)
    }

    pub fn is_openai(&self) -> bool {
        self.name == OPENAI_PROVIDER_NAME
    }

    pub fn supports_codex_backend_routes(&self) -> bool {
        self.is_openai()
            && self.base_url.as_deref().is_none_or(|base_url| {
                base_url
                    .trim_end_matches('/')
                    .ends_with("/backend-api/codex")
            })
    }

    pub fn uses_openai_actor_authorization(&self) -> bool {
        !self.requires_openai_auth
            && self.http_headers.as_ref().is_some_and(|headers| {
                headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case(OPENAI_ACTOR_AUTHORIZATION_HEADER)
                        && !value.trim().is_empty()
                })
            })
    }

    pub fn has_command_auth(&self) -> bool {
        self.auth.is_some()
    }
}

/// Built-in default provider list.
pub fn built_in_model_providers(
    openai_base_url: Option<String>,
) -> HashMap<String, ModelProviderInfo> {
    HashMap::from([(
        OPENAI_PROVIDER_ID.to_owned(),
        ModelProviderInfo::create_openai_provider(openai_base_url),
    )])
}

pub fn is_removed_cloud_provider_id(id: &str) -> bool {
    matches!(
        id,
        AMAZON_BEDROCK_PROVIDER_ID | AMAZON_BEDROCK_RUNTIME_PROVIDER_ID | GCP_PROVIDER_ID
    )
}

/// Merge configured providers into the built-in provider catalog.
///
/// Configured providers extend the built-in set. Built-in providers are not
/// generally overridable. OpenAI allows a catalog URL override.
pub fn merge_configured_model_providers(
    mut model_providers: HashMap<String, ModelProviderInfo>,
    configured_model_providers: HashMap<String, ModelProviderInfo>,
) -> Result<HashMap<String, ModelProviderInfo>, String> {
    for (key, mut provider) in configured_model_providers {
        if key == "openai" {
            // Keep the built-in identity, inference routing, and authentication.
            // Only catalog routing is configurable through this provider entry.
            if let Some(catalog_url) = provider.model_catalog_url.take()
                && let Some(built_in_provider) = model_providers.get_mut(&key)
            {
                if !codex_http_client::is_openai_model_api_url(&catalog_url) {
                    return Err("model_providers.openai.model_catalog_url must use an OpenAI API HTTPS endpoint. If you need to use a 3rd party host, configure a custom model_provider".to_string());
                }
                built_in_provider.model_catalog_url = Some(catalog_url);
            }
        } else {
            if is_removed_cloud_provider_id(&key)
                || provider.gateway_oauth.is_some()
                || provider.auth.is_some()
            {
                return Err(format!(
                    "model_providers.{key}: cloud and account authentication are unsupported. Configure a base_url and an API key."
                ));
            }
            model_providers.entry(key).or_insert(provider);
        }
    }

    Ok(model_providers)
}

pub fn custom_provider_with_base_url(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "Custom".into(),
        base_url: Some(base_url.into()),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        gateway_oauth: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        capabilities: None,
        include_internal_metadata: false,
    }
}

#[cfg(test)]
#[path = "model_provider_info_tests.rs"]
mod tests;
