//! Windows sandbox configuration, runtime policy, and lifecycle diagnostics.

use crate::config::Config;
use codex_config::config_toml::ConfigToml;
use codex_config::types::WindowsSandboxModeToml;
use codex_features::Feature;
use codex_features::Features;
use codex_features::FeaturesToml;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::sandbox::effective_windows_sandbox_type;
use codex_sandboxing::SandboxType;
use std::collections::BTreeMap;

/// Selects local binding policy from the sandbox and executor OS.
pub fn local_binding_policy_for_sandbox(
    sandbox_type: SandboxType,
    platform_os: Option<&str>,
) -> codex_network_proxy::LocalBindingPolicy {
    if sandbox_type == SandboxType::WindowsMxc && platform_os == Some("windows") {
        codex_network_proxy::LocalBindingPolicy::RequireTrue
    } else {
        codex_network_proxy::LocalBindingPolicy::DefaultFalse
    }
}

pub fn managed_proxy_routing_for_windows_sandbox(
    sandbox_type: SandboxType,
) -> codex_network_proxy::ManagedProxyRouting {
    if cfg!(windows) && sandbox_type == SandboxType::WindowsMxc {
        codex_network_proxy::ManagedProxyRouting::DedicatedListeners
    } else {
        codex_network_proxy::ManagedProxyRouting::SharedIngress
    }
}

/// Adapts the selected implementation for legacy checks that only understand setup levels.
/// Backend selection must continue to use [`SandboxType`] directly.
pub(crate) fn windows_sandbox_level_for_legacy_checks(
    sandbox_type: SandboxType,
    sandbox_level: WindowsSandboxLevel,
) -> WindowsSandboxLevel {
    if effective_windows_sandbox_type(sandbox_type, sandbox_level) == SandboxType::WindowsMxc {
        WindowsSandboxLevel::RestrictedToken
    } else {
        sandbox_level
    }
}

pub trait WindowsSandboxLevelExt {
    fn from_config(config: &Config) -> WindowsSandboxLevel;
    fn from_features(features: &Features) -> WindowsSandboxLevel;
}

impl WindowsSandboxLevelExt for WindowsSandboxLevel {
    fn from_config(config: &Config) -> WindowsSandboxLevel {
        match config.permissions.windows_sandbox_mode {
            Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
            Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
            Some(WindowsSandboxModeToml::Mxc) => WindowsSandboxLevel::Disabled,
            None => Self::from_features(&config.features),
        }
    }

    fn from_features(features: &Features) -> WindowsSandboxLevel {
        if features.enabled(Feature::WindowsSandboxElevated) {
            return WindowsSandboxLevel::Elevated;
        }
        if features.enabled(Feature::WindowsSandbox) {
            WindowsSandboxLevel::RestrictedToken
        } else {
            WindowsSandboxLevel::Disabled
        }
    }
}

pub fn resolve_windows_sandbox_mode(cfg: &ConfigToml) -> Option<WindowsSandboxModeToml> {
    cfg.windows
        .as_ref()
        .and_then(|windows| windows.sandbox)
        .or_else(|| legacy_windows_sandbox_mode(cfg.features.as_ref()))
}

pub fn legacy_windows_sandbox_mode(
    features: Option<&FeaturesToml>,
) -> Option<WindowsSandboxModeToml> {
    let entries = features.map(FeaturesToml::entries)?;
    legacy_windows_sandbox_mode_from_entries(&entries)
}

pub fn legacy_windows_sandbox_mode_from_entries(
    entries: &BTreeMap<String, bool>,
) -> Option<WindowsSandboxModeToml> {
    if entries
        .get(Feature::WindowsSandboxElevated.key())
        .copied()
        .unwrap_or(false)
    {
        return Some(WindowsSandboxModeToml::Elevated);
    }
    if entries
        .get(Feature::WindowsSandbox.key())
        .copied()
        .unwrap_or(false)
        || entries
            .get("enable_experimental_windows_sandbox")
            .copied()
            .unwrap_or(false)
    {
        Some(WindowsSandboxModeToml::Unelevated)
    } else {
        None
    }
}
