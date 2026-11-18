use std::collections::HashMap;
use std::path::PathBuf;

use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::SandboxType;

/// Resolved filesystem overrides for the Windows sandbox backends.
///
/// The elevated Windows backend consumes extra deny-read paths plus explicit
/// read and write roots during setup/refresh. The unelevated restricted-token
/// backend only consumes extra deny-write carveouts on top of the legacy
/// `WorkspaceWrite` allow set. Read-root overrides are layered on top of the
/// baseline helper roots that the elevated setup path needs to launch the
/// sandboxed command; split policies that opt into platform defaults carry
/// that explicitly with the override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsSandboxFilesystemOverrides {
    pub read_roots_override: Option<Vec<PathBuf>>,
    pub read_roots_include_platform_defaults: bool,
    pub write_roots_override: Option<Vec<PathBuf>>,
    pub additional_deny_read_paths: Vec<AbsolutePathBuf>,
    pub additional_deny_write_paths: Vec<AbsolutePathBuf>,
}

impl WindowsSandboxFilesystemOverrides {
    /// Resolve filesystem overrides for the selected Windows sandbox using the child's environment.
    pub fn resolve(
        sandbox: SandboxType,
        _permission_profile: &PermissionProfile,
        _sandbox_policy_cwd: &AbsolutePathBuf,
        _windows_sandbox_level: WindowsSandboxLevel,
        _workload_env: &HashMap<String, String>,
    ) -> Result<Option<Self>, String> {
        if sandbox != SandboxType::None {
            return Err("Windows sandbox is unsupported in this YOLO-only build".into());
        }
        Ok(None)
    }
}

pub fn windows_sandbox_uses_elevated_backend(sandbox_level: WindowsSandboxLevel) -> bool {
    matches!(sandbox_level, WindowsSandboxLevel::Elevated)
}
