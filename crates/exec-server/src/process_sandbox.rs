use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::process_telemetry::ProcessTelemetry;
use codex_exec_server_protocol::JSONRPCErrorError;
#[cfg(windows)]
use codex_file_system::WindowsSandboxSelection;
use codex_network_proxy::CredentialMaskingFailureMode;
use codex_network_proxy::ManagedNetworkSandboxContext;
use codex_network_proxy::ManagedProxyRouting;
use codex_network_proxy::NetworkPolicyAuditObserver;
use codex_network_proxy::NetworkPolicyDecider;
use codex_network_proxy::NetworkProxy;
use codex_network_proxy::NetworkProxyHandle;
use codex_network_proxy::NetworkProxyState;
use codex_network_proxy::RemoteCredentialBrokerCache;
use codex_sandboxing::SandboxType;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

use crate::ExecServerRuntimeOptions;
use crate::process_telemetry::trace_process_id;
use crate::protocol::ExecParams;
use crate::rpc::internal_error;
use crate::rpc::invalid_params;

const CREDENTIAL_MASKING_FAILURE_MODE: CredentialMaskingFailureMode =
    CredentialMaskingFailureMode::Warn;

pub(crate) struct PreparedExecRequest {
    pub(crate) command: Vec<String>,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) env: HashMap<String, String>,
    pub(crate) arg0: Option<String>,
    pub(crate) sandbox: SandboxType,
    pub(crate) network_proxy_handle: Option<NetworkProxyHandle>,
}

impl PreparedExecRequest {
    #[cfg(unix)]
    pub(crate) fn credential_snapshot_proxy(&self, params: &ExecParams) -> Option<&NetworkProxy> {
        let launch = params.network_proxy.as_ref()?;
        if !launch.proxy.credential_masking_enabled() || !supports_credential_snapshot(params) {
            return None;
        }
        self.network_proxy_handle.as_ref()?.proxy()
    }
}

fn supports_credential_snapshot(params: &ExecParams) -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
        && params.arg0.is_none()
        && params.shell_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.shell.name == "zsh"
                && matches!(params.argv.as_slice(), [program, flag, _]
                    if program == &snapshot.shell.path
                        && flag == "-lc"
                        && Path::new(program).file_name().is_some_and(|name| name == "zsh"))
        })
}

#[tracing::instrument(
    name = "codex.exec_server.process_prepare_sandbox",
    skip_all,
    fields(process.id = trace_process_id(params.process_id.as_str())),
)]
pub(crate) async fn prepare_exec_request_with_telemetry(
    params: &ExecParams,
    env: HashMap<String, String>,
    runtime_paths: Option<&ExecServerRuntimeOptions>,
    network_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
    network_policy_audit_observer: Option<NetworkPolicyAuditObserver>,
    credential_brokers: &RemoteCredentialBrokerCache,
    telemetry: &ProcessTelemetry,
) -> Result<PreparedExecRequest, JSONRPCErrorError> {
    if params.sandbox.is_some() || params.enforce_managed_network {
        return Err(invalid_params(
            "sandboxed process launches are unsupported in this YOLO-only build".to_string(),
        ));
    }
    let (env, _, network_proxy_handle, _) = prepare_managed_network(
        params,
        credential_brokers,
        runtime_paths.is_some_and(|paths| paths.proxy_private_ips_via_upstream),
        env,
        network_policy_decider,
        network_policy_audit_observer,
        telemetry,
    )
    .await?;
    Ok(PreparedExecRequest {
        command: params.argv.clone(),
        cwd: native_path(&params.cwd, "cwd")?,
        env,
        arg0: params.arg0.clone(),
        sandbox: SandboxType::None,
        network_proxy_handle,
    })
}

async fn prepare_managed_network(
    params: &ExecParams,
    credential_brokers: &RemoteCredentialBrokerCache,
    proxy_private_ips_via_upstream: bool,
    env: HashMap<String, String>,
    network_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
    network_policy_audit_observer: Option<NetworkPolicyAuditObserver>,
    telemetry: &ProcessTelemetry,
) -> Result<
    (
        HashMap<String, String>,
        Option<ManagedNetworkSandboxContext>,
        Option<NetworkProxyHandle>,
        Option<String>,
    ),
    JSONRPCErrorError,
> {
    let Some(launch) = &params.network_proxy else {
        return Ok((env, params.managed_network.clone(), None, None));
    };
    #[cfg(windows)]
    let routing = match params
        .sandbox
        .as_ref()
        .map(|sandbox| sandbox.windows_sandbox_selection)
    {
        None => {
            // Native Windows commands have no sandbox token SID for shared ingress.
            let mut native_env = env;
            codex_network_proxy::strip_managed_proxy_env(&mut native_env);
            return Ok((native_env, params.managed_network.clone(), None, None));
        }
        Some(WindowsSandboxSelection::Mxc) => ManagedProxyRouting::DedicatedListeners,
        Some(
            WindowsSandboxSelection::Disabled
            | WindowsSandboxSelection::RestrictedToken
            | WindowsSandboxSelection::Elevated,
        ) => ManagedProxyRouting::SharedIngress,
    };
    #[cfg(not(windows))]
    let routing = ManagedProxyRouting::SharedIngress;
    let mut state = NetworkProxyState::from_remote_launch_config(
        launch.clone(),
        codex_utils_path_uri::Platform::native(),
        credential_brokers,
        &env,
    )
    .map_err(|err| invalid_params(format!("invalid network proxy config: {err}")))?;
    let credential_masking = state
        .current_cfg()
        .await
        .map_err(|err| {
            internal_error(format!(
                "failed to read executor network proxy config: {err}"
            ))
        })?
        .credential_broker;
    state.set_proxy_private_ips_via_upstream(proxy_private_ips_via_upstream);
    if let Some(observer) = network_policy_audit_observer {
        state.set_policy_audit_observer(observer);
    }
    if let Some(launch_context) = &telemetry.launch_context {
        state.set_launch_span_context(launch_context.clone());
    }
    state.set_process_log_metadata(codex_network_proxy::NetworkProxyProcessLogMetadata {
        thread_id: telemetry.thread_id.clone(),
        tool_call_id: telemetry.tool_call_id.clone(),
        executor_identity: telemetry
            .executor_registration
            .as_ref()
            .map(|registration| codex_network_proxy::ExecutorLogIdentity {
                environment_id: registration.environment_id.clone(),
                registration_id: registration.executor_registration_id.clone(),
            }),
    });
    let mut builder = NetworkProxy::builder()
        .state(Arc::new(state))
        .managed_proxy_routing(routing);
    if let Some(network_policy_decider) = network_policy_decider {
        builder = builder.policy_decider_arc(network_policy_decider);
    }
    let proxy = builder
        .build()
        .await
        .map_err(|err| internal_error(format!("failed to build executor network proxy: {err}")))?;
    let handle = proxy
        .run()
        .await
        .map_err(|err| internal_error(format!("failed to start executor network proxy: {err}")))?;
    #[cfg(target_os = "windows")]
    let network_proxy_restricting_sid = if routing == ManagedProxyRouting::SharedIngress {
        Some(
            proxy
                .network_proxy_restricting_sid(/*environment_id*/ None)
                .ok_or_else(|| {
                    internal_error(
                        "managed Windows proxy route is missing its restricting SID".to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    #[cfg(not(target_os = "windows"))]
    let network_proxy_restricting_sid = None;
    let supported_shell = matches!(params.argv.as_slice(), [program, flag, _]
        if Path::new(program).file_name().is_some_and(|name| name == "bash") && flag == "-c")
        && !["BASH_ENV", "ENV", "SSH_CLIENT", "SSH2_CLIENT"]
            .iter()
            .any(|key| env.contains_key(*key));
    let mut prepared = proxy
        .prepare_for_optional_environment(env, /*environment_id*/ None)
        .map_err(|err| {
            internal_error(format!("failed to prepare executor network proxy: {err}"))
        })?;
    if credential_masking {
        if supports_credential_snapshot(params) {
            // Requested snapshots trust startup profiles before masking their captured state.
            proxy.restore_and_disable_brokered_credentials(&mut prepared.env, &mut []);
        } else if cfg!(any(target_os = "linux", target_os = "macos"))
            && supported_shell
            && params.arg0.is_none()
            && params.shell_snapshot.is_none()
        {
            proxy
                .handle_unmasked_credentials(&mut prepared.env, CREDENTIAL_MASKING_FAILURE_MODE)
                .map_err(|err| invalid_params(err.to_string()))?;
        } else {
            tracing::warn!(
                "Credential masking is unavailable for this shell startup; preserving shell profiles and snapshots and running with unmasked credentials."
            );
            // Restore this child's environment without disabling translation of placeholders
            // issued to earlier commands in the same environment.
            proxy.restore_and_disable_brokered_credentials(&mut prepared.env, &mut []);
        }
    }
    Ok((
        prepared.env,
        Some(prepared.sandbox_context),
        Some(handle),
        network_proxy_restricting_sid,
    ))
}

fn native_path(path: &PathUri, label: &str) -> Result<AbsolutePathBuf, JSONRPCErrorError> {
    path.to_abs_path().map_err(|err| {
        invalid_params(format!(
            "{label} URI `{path}` is not valid on this exec-server host: {err}"
        ))
    })
}

#[cfg(test)]
#[path = "process_sandbox_tests.rs"]
mod tests;
