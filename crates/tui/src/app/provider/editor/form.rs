//! Provider editor drafts retain only changed fields, keeping server-only settings out of writes.

use super::super::Field;
use super::super::provider_fields;
use codex_app_server_protocol::CustomGatewaySettingsParams;
use codex_app_server_protocol::ModelProviderConfigWriteParams;
use codex_app_server_protocol::ModelProviderSummary;
use codex_app_server_protocol::ProviderSettings;
use codex_app_server_protocol::ProviderSettingsPatch;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Clone, Default)]
pub(crate) struct Draft {
    pub(super) provider: Option<ModelProviderSummary>,
    pub(super) changes: BTreeMap<Field, String>,
    pub(super) variables: BTreeSet<String>,
    pub(super) selected: usize,
}

impl Draft {
    pub(crate) fn new(
        provider: Option<ModelProviderSummary>,
        catalog: &[ModelProviderSummary],
    ) -> Self {
        let variables = catalog
            .iter()
            .flat_map(provider_fields)
            .filter(|(field, _)| matches!(field, Field::Variable | Field::ExternalAuth))
            .filter_map(|(_, name)| name.map(str::to_owned))
            .collect();
        Self {
            provider,
            variables,
            ..Self::default()
        }
    }

    pub(super) fn fields(&self) -> Vec<(Field, Option<&str>)> {
        let Some(p) = &self.provider else {
            return [Field::Name, Field::Url, Field::Variable]
                .map(|field| (field, None))
                .into();
        };
        provider_fields(p)
            .into_iter()
            .filter(|(field, _)| !matches!(field, Field::ManagedUrl | Field::ExternalAuth))
            .collect()
    }

    pub(super) fn value<'a>(&'a self, field: Field, original: Option<&'a str>) -> &'a str {
        self.changes
            .get(&field)
            .map_or(original.unwrap_or_default(), String::as_str)
    }

    pub(super) fn required(&self, field: Field) -> bool {
        field == Field::Name
            || (field == Field::Url && self.provider.as_ref().is_none_or(|p| p.base_url.is_some()))
    }

    pub(super) fn patch(&self) -> Result<ModelProviderConfigWriteParams, &'static str> {
        if self.provider.as_ref().is_some_and(|p| !p.can_edit) {
            return Err("This provider configuration is read-only.");
        }
        for (field, original) in self.fields() {
            if self.required(field) && self.value(field, original).trim().is_empty() {
                return Err(if field == Field::Name {
                    "Enter a provider name."
                } else {
                    "Enter a base URL."
                });
            }
        }
        if let Some(key) = self.changes.get(&Field::Variable)
            && !key.is_empty()
        {
            let original_url = self.provider.as_ref().and_then(|p| p.base_url.as_deref());
            if self.value(Field::Url, original_url).trim().is_empty() {
                return Err("Enter a base URL to use this API key variable.");
            }
            if !(key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
            {
                return Err("Enter an environment variable name such as TEAM_API_KEY.");
            }
        }
        let changed = |field| {
            self.changes
                .get(&field)
                .map(|s| (!s.is_empty()).then(|| s.clone()))
        };
        let provider = self.provider.as_ref();
        let settings = match provider.map(|p| &p.settings) {
            None | Some(ProviderSettings::CustomGateway { .. }) => {
                ProviderSettingsPatch::CustomGateway(CustomGatewaySettingsParams {
                    provider_id: provider.map(|p| p.id.clone()),
                    name: self.changes.get(&Field::Name).cloned(),
                    base_url: changed(Field::Url),
                    env_key: changed(Field::Variable),
                })
            }
            Some(ProviderSettings::OpenAi) => {
                return Err("This provider configuration is read-only.");
            }
        };
        Ok(ModelProviderConfigWriteParams { settings })
    }
}
