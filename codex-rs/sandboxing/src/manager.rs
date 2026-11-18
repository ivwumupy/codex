use self::launch::WrappedSandboxCommand;
use crate::LinuxSandboxPidNamespace;
use crate::policy_transforms::should_require_platform_sandbox;
use codex_network_proxy::ManagedNetworkSandboxContext;
use codex_network_proxy::NetworkProxy;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::sandbox::SandboxOverride;
pub use codex_protocol::sandbox::SandboxType;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::Path;

#[path = "launch.rs"]
pub(super) mod launch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxablePreference {
    Auto,
    Require,
    Forbid,
}

pub fn get_platform_sandbox(_windows_sandbox_enabled: bool) -> Option<SandboxType> {
    None
}

pub fn with_managed_mitm_ca_readable_root(
    permission_profile: PermissionProfile,
    managed_mitm_ca_trust_bundle_path: Option<&AbsolutePathBuf>,
    sandbox_policy_cwd: &Path,
) -> PermissionProfile {
    let Some(managed_mitm_ca_trust_bundle_path) = managed_mitm_ca_trust_bundle_path else {
        return permission_profile;
    };
    let (file_system_sandbox_policy, network_sandbox_policy) =
        permission_profile.to_runtime_permissions();
    let file_system_sandbox_policy = file_system_sandbox_policy.with_additional_readable_roots(
        sandbox_policy_cwd,
        std::slice::from_ref(managed_mitm_ca_trust_bundle_path),
    );
    PermissionProfile::from_runtime_permissions_with_enforcement(
        permission_profile.enforcement(),
        &file_system_sandbox_policy,
        network_sandbox_policy,
    )
}

#[derive(Debug)]
pub struct SandboxCommand {
    pub program: OsString,
    pub args: Vec<String>,
    pub cwd: PathUri,
    pub env: HashMap<String, String>,
    pub managed_network: Option<ManagedNetworkSandboxContext>,
    pub additional_permissions: Option<AdditionalPermissionProfile>,
}

/// A host-native launch request produced after the sandbox manager validates URI inputs.
/// Build this only at the execution boundary: in exec-server, or in its logical equivalent within
/// app-server. Orchestration and transport code should retain [`PathUri`] values and defer
/// conversion to native paths until this request is created.
#[derive(Clone, Debug)]
pub struct SandboxExecRequest {
    /// Controller-selected override, retained for observations without altering the sandbox command.
    pub sandbox_override: SandboxOverride,
    pub command: Vec<String>,
    pub cwd: PathUri,
    pub sandbox_policy_cwd: PathUri,
    pub env: HashMap<String, String>,
    pub network: Option<NetworkProxy>,
    pub network_environment_id: Option<String>,
    pub sandbox: SandboxType,
    // TODO(anp): Reconcile these backend copies with the supplied sandbox context
    // (TurnEnvironment::sandbox_context for turns), preserving this launch snapshot.
    pub windows_sandbox_level: WindowsSandboxLevel,
    pub permission_profile: PermissionProfile,
    pub arg0: Option<String>,
}

/// How the execution host selects a backend for this launch.
#[derive(Clone, Copy)]
pub enum SandboxSelection {
    /// Use the selected backend, including an explicitly native launch.
    Explicit(SandboxType),
    /// Refuse the launch if this host cannot provide its platform sandbox.
    RequirePlatform,
}

/// Bundled arguments for sandbox transformation.
///
/// This keeps call sites self-documenting when several fields are optional.
pub struct SandboxTransformRequest<'a> {
    pub command: SandboxCommand,
    pub permissions: &'a PermissionProfile,
    /// Bind symbolic workspace rules to these executor roots. `None` means they are already resolved.
    pub workspace_roots: Option<&'a [PathUri]>,
    /// Executor-selected helper binaries and validated trust bundles needed by the child.
    pub runtime_readable_paths: &'a [AbsolutePathBuf],
    pub selection: SandboxSelection,
    pub enforce_managed_network: bool,
    pub environment_id: Option<&'a str>,
    // TODO(viyatb): Evaluate switching this to Option<Arc<NetworkProxy>>
    // to make shared ownership explicit across runtime/sandbox plumbing.
    pub network: Option<&'a NetworkProxy>,
    pub sandbox_policy_cwd: &'a PathUri,
    pub sandbox_exe: Option<&'a Path>,
    // TODO(anp): Reconcile these backend inputs with the supplied sandbox context
    // (TurnEnvironment::sandbox_context for turns) so selection shares its authority.
    pub use_legacy_landlock: bool,
    pub windows_sandbox_level: WindowsSandboxLevel,
}

/// Bundled arguments for a sandbox transformation whose result will be spawned
/// directly from argv.
///
/// Direct-spawn callers will not run a later platform-specific launcher, so the
/// returned command must encode any sandbox wrapper it needs.
pub struct SandboxDirectSpawnTransformRequest<'a> {
    pub transform: SandboxTransformRequest<'a>,
    pub workspace_roots: &'a [AbsolutePathBuf],
    pub windows_sandbox_proxy_settings_mode:
        codex_protocol::config_types::WindowsSandboxProxySettingsMode,
}

#[derive(Debug)]
pub enum SandboxTransformError {
    InvalidPolicy(io::Error),
    InvalidCommandCwd { cwd: PathUri, source: io::Error },
    InvalidSandboxPolicyCwd { cwd: PathUri, source: io::Error },
    SandboxUnavailable,
}

impl std::fmt::Display for SandboxTransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPolicy(err) => err.fmt(f),
            Self::InvalidCommandCwd { cwd, source } => {
                write!(
                    f,
                    "command cwd URI `{cwd}` is not valid on this host: {source}"
                )
            }
            Self::InvalidSandboxPolicyCwd { cwd, source } => write!(
                f,
                "sandbox policy cwd URI `{cwd}` is not valid on this host: {source}"
            ),
            Self::SandboxUnavailable => {
                write!(f, "sandbox intent cannot be enforced on this executor")
            }
        }
    }
}

impl std::error::Error for SandboxTransformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPolicy(source) => Some(source),
            Self::InvalidCommandCwd { source, .. }
            | Self::InvalidSandboxPolicyCwd { source, .. } => Some(source),
            Self::SandboxUnavailable => None,
        }
    }
}

#[derive(Clone, Default)]
pub struct SandboxManager;

impl SandboxManager {
    pub fn new() -> Self {
        Self
    }

    pub fn for_file_system_helpers() -> Self {
        Self
    }

    pub fn for_inherited_terminal(_allow_unix_sockets: &[AbsolutePathBuf]) -> Self {
        Self
    }

    pub fn with_linux_sandbox_pid_namespace(self, _mode: LinuxSandboxPidNamespace) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    pub fn with_allowed_symlinked_codex_home(self, _home: Option<AbsolutePathBuf>) -> Self {
        self
    }

    pub fn select_initial(
        &self,
        _permission_profile: &PermissionProfile,
        _pref: SandboxablePreference,
        _windows_sandbox_type: SandboxType,
        _has_managed_network_requirements: bool,
    ) -> SandboxType {
        SandboxType::None
    }

    /// Returns whether the request needs a sandbox, independently of whether
    /// this host can provide a concrete sandbox implementation.
    pub fn should_sandbox(
        &self,
        permission_profile: &PermissionProfile,
        pref: SandboxablePreference,
        has_managed_network_requirements: bool,
    ) -> bool {
        match pref {
            SandboxablePreference::Forbid => false,
            SandboxablePreference::Require => true,
            SandboxablePreference::Auto => {
                let (file_system_policy, network_policy) =
                    permission_profile.to_runtime_permissions();
                should_require_platform_sandbox(
                    &file_system_policy,
                    network_policy,
                    has_managed_network_requirements,
                )
            }
        }
    }

    /// Prepare filesystem and network enforcement for a command on the execution host.
    pub fn transform(
        &self,
        request: SandboxTransformRequest<'_>,
    ) -> Result<WrappedSandboxCommand, SandboxTransformError> {
        if !matches!(
            request.selection,
            SandboxSelection::Explicit(SandboxType::None)
        ) || request.permissions != &PermissionProfile::Disabled
            || request.enforce_managed_network
        {
            return Err(SandboxTransformError::SandboxUnavailable);
        }
        let command = request.command;
        let native_program = command
            .program
            .to_str()
            .is_none()
            .then(|| command.program.clone());
        let mut argv = Vec::with_capacity(1 + command.args.len());
        argv.push(command.program.to_string_lossy().into_owned());
        argv.extend(command.args);
        Ok(WrappedSandboxCommand {
            native_program,
            request: SandboxExecRequest {
                sandbox_override: SandboxOverride::NoOverride,
                command: argv,
                cwd: command.cwd,
                sandbox_policy_cwd: request.sandbox_policy_cwd.clone(),
                env: command.env,
                network: request.network.cloned(),
                network_environment_id: request.environment_id.map(str::to_owned),
                sandbox: SandboxType::None,
                windows_sandbox_level: WindowsSandboxLevel::Disabled,
                permission_profile: PermissionProfile::Disabled,
                arg0: None,
            },
        })
    }

    pub fn transform_for_direct_spawn(
        &self,
        request: SandboxDirectSpawnTransformRequest<'_>,
    ) -> Result<SandboxExecRequest, SandboxTransformError> {
        self.transform(request.transform)
            .map(WrappedSandboxCommand::into_command_request)
    }
}

pub fn compatibility_sandbox_policy_for_permission_profile(
    permissions: &PermissionProfile,
    cwd: &Path,
) -> SandboxPolicy {
    permissions
        .to_legacy_sandbox_policy(cwd)
        .unwrap_or_else(|_| {
            let (file_system_policy, network_policy) = permissions.to_runtime_permissions();
            compatibility_workspace_write_policy(file_system_policy, network_policy, cwd)
        })
}

fn compatibility_workspace_write_policy(
    file_system_policy: FileSystemSandboxPolicy,
    network_policy: NetworkSandboxPolicy,
    cwd: &Path,
) -> SandboxPolicy {
    let cwd_abs = AbsolutePathBuf::from_absolute_path(cwd).ok();
    let writable_roots = file_system_policy
        .get_writable_roots_with_cwd(cwd)
        .into_iter()
        .map(|root| root.root)
        .filter(|root| cwd_abs.as_ref() != Some(root))
        .collect();
    let tmpdir_writable = std::env::var_os("TMPDIR")
        .filter(|tmpdir| !tmpdir.is_empty())
        .and_then(|tmpdir| {
            AbsolutePathBuf::from_absolute_path(std::path::PathBuf::from(tmpdir)).ok()
        })
        .is_some_and(|tmpdir| {
            file_system_policy.can_write_local_path_with_cwd(tmpdir.as_path(), cwd)
        });
    let slash_tmp = Path::new("/tmp");
    let slash_tmp_writable = slash_tmp.is_absolute()
        && slash_tmp.is_dir()
        && file_system_policy.can_write_local_path_with_cwd(slash_tmp, cwd);

    SandboxPolicy::WorkspaceWrite {
        writable_roots,
        network_access: network_policy.is_enabled(),
        exclude_tmpdir_env_var: !tmpdir_writable,
        exclude_slash_tmp: !slash_tmp_writable,
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
