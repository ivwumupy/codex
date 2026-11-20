//! Fields reported by the server for each provider, including values it manages externally.

use codex_app_server_protocol::GatewayAuth;
use codex_app_server_protocol::ModelProviderSummary;
use codex_app_server_protocol::ProviderSettings;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Field {
    Name,
    Url,
    ManagedUrl,
    Variable,
    ExternalAuth,
}

impl Field {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Url | Self::ManagedUrl => "Base URL",
            Self::Variable => "API key variable",
            Self::ExternalAuth => "External authentication",
        }
    }
}

pub(super) fn provider_fields(provider: &ModelProviderSummary) -> Vec<(Field, Option<&str>)> {
    let url = provider.base_url.as_deref();
    let mut fields = match &provider.settings {
        ProviderSettings::OpenAi => Vec::new(),
        ProviderSettings::CustomGateway { auth } => {
            let auth_field = match auth {
                GatewayAuth::None => (Field::Variable, None),
                GatewayAuth::EnvironmentVariable { env_key } => {
                    (Field::Variable, Some(env_key.as_str()))
                }
                GatewayAuth::ExternallyConfigured { env_key } => {
                    (Field::ExternalAuth, env_key.as_deref())
                }
            };
            let endpoint = if auth_field.0 == Field::ExternalAuth && url.is_none() {
                Field::ManagedUrl
            } else {
                Field::Url
            };
            return vec![
                (Field::Name, Some(provider.name.as_str())),
                (endpoint, url),
                auth_field,
            ];
        }
    };
    if let Some(url) = url {
        fields.push((Field::ManagedUrl, Some(url)));
    }
    fields
}
