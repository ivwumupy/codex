//! Plugin mutation errors and their stable telemetry classifications.

use crate::marketplace::MarketplaceError;
use crate::store::PluginStoreError;
use crate::store::error_context_sub_error_type;
use codex_plugin::PluginIdError;

#[derive(Debug, thiserror::Error)]
pub enum PluginInstallError {
    #[error("{0}")]
    Marketplace(#[from] MarketplaceError),

    #[error("{0}")]
    Store(#[from] PluginStoreError),

    #[error("{0}")]
    Config(#[from] anyhow::Error),

    #[error("failed to join plugin install task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl PluginInstallError {
    pub(super) fn join(source: tokio::task::JoinError) -> Self {
        Self::Join(source)
    }

    pub fn is_invalid_request(&self) -> bool {
        matches!(
            self,
            Self::Marketplace(
                MarketplaceError::MarketplaceNotFound { .. }
                    | MarketplaceError::InvalidMarketplaceFile { .. }
                    | MarketplaceError::PluginNotFound { .. }
                    | MarketplaceError::PluginNotAvailable { .. }
                    | MarketplaceError::InvalidPlugin(_)
            ) | Self::Store(PluginStoreError::Invalid(_))
        )
    }

    pub fn sub_error_type(&self) -> Option<String> {
        match self {
            Self::Marketplace(err) => marketplace_error_sub_error_type(err),
            Self::Store(err) => err.sub_error_type(),
            Self::Config(_) => Some("failed_to_enable_plugin".to_string()),
            Self::Join(_) => Some("plugin_install_task_failed".to_string()),
        }
    }
}

pub(super) fn marketplace_error_sub_error_type(err: &MarketplaceError) -> Option<String> {
    match err {
        MarketplaceError::Io { context, .. } => Some(error_context_sub_error_type(context)),
        MarketplaceError::MarketplaceNotFound { .. }
        | MarketplaceError::InvalidMarketplaceFile { .. }
        | MarketplaceError::PluginNotFound { .. }
        | MarketplaceError::PluginNotAvailable { .. }
        | MarketplaceError::PluginsDisabled
        | MarketplaceError::InvalidPlugin(_) => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginUninstallError {
    #[error("{0}")]
    InvalidPluginId(#[from] PluginIdError),

    #[error("{0}")]
    Store(#[from] PluginStoreError),

    #[error("{0}")]
    Config(#[from] anyhow::Error),

    #[error("failed to join plugin uninstall task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl PluginUninstallError {
    pub(super) fn join(source: tokio::task::JoinError) -> Self {
        Self::Join(source)
    }

    pub fn is_invalid_request(&self) -> bool {
        matches!(self, Self::InvalidPluginId(_))
    }
}
