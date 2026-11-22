//! Command-line startup for `codex exec-server`.
//!
//! Transport, configuration, authentication, and shutdown are kept together.

use std::sync::Arc;

use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_core::config::Config;
use codex_exec_server::ExecServerRuntimeOptions;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_utils_cli::CliConfigOverrides;
use codex_websocket_auth::WebsocketAuthArgs;

use crate::config::load_exec_server_config;
use crate::config::load_exec_server_remote_auth_provider;
use crate::exec_server_credential::ExecutorCredentialPipe;
use crate::exec_server_telemetry;
use crate::exec_server_telemetry::ParentLifetime;

/// [EXPERIMENTAL] Run the standalone exec-server service.
#[derive(Debug, Parser)]
pub struct ExecServerCommand {
    #[command(subcommand)]
    command: Option<ExecServerSubcommand>,

    /// Error out when config.toml contains fields that are not recognized by this version of Codex.
    #[arg(
        id = "exec_server_strict_config",
        long = "strict-config",
        default_value_t = false,
        global = true
    )]
    pub strict_config: bool,

    /// Allow permitted private IP destinations to use the configured upstream proxy.
    /// If no valid upstream proxy applies to the request protocol, connect directly.
    /// Loopback stays local. This flag does not require upstream routing.
    #[arg(
        long,
        env = "CODEX_EXEC_SERVER_PROXY_PRIVATE_IPS_VIA_UPSTREAM",
        global = true
    )]
    proxy_private_ips_via_upstream: bool,

    /// Maximum number of requests to process concurrently on each connection.
    #[arg(
        long = "concurrent-requests",
        value_name = "COUNT",
        default_value = "1"
    )]
    request_dispatch_mode: codex_exec_server::RequestDispatchMode,

    /// Transport endpoint URL. Supported values: `ws://IP:PORT` (default), `stdio`, `stdio://`.
    #[arg(
        long = "listen",
        value_name = "URL",
        conflicts_with = "exec_server_remote"
    )]
    listen: Option<String>,

    #[command(flatten)]
    websocket_auth: WebsocketAuthArgs,

    /// Register this exec-server as a remote environment using the given base URL.
    #[arg(
        long = "remote",
        id = "exec_server_remote",
        value_name = "URL",
        requires = "environment_id",
        global = true
    )]
    remote: Option<String>,

    /// Transport used for the remote executor connection.
    #[arg(
        long = "remote-transport",
        value_enum,
        default_value_t = ExecServerRemoteTransport::Noise,
        requires = "exec_server_remote",
        global = true
    )]
    remote_transport: ExecServerRemoteTransport,

    /// Environment id to attach to when registering remotely.
    #[arg(long = "environment-id", value_name = "ID", global = true)]
    environment_id: Option<String>,

    /// Human-readable environment name.
    #[arg(long = "name", value_name = "NAME", global = true)]
    name: Option<String>,

    /// Use Agent Identity auth from CODEX_ACCESS_TOKEN for remote registration.
    #[arg(
        long = "use-agent-identity-auth",
        requires = "exec_server_remote",
        global = true
    )]
    use_agent_identity_auth: bool,

    /// Read one registration bearer from stdin.
    #[arg(
        long,
        requires = "exec_server_remote",
        conflicts_with_all = ["use_agent_identity_auth", "executor_credential_stdin"],
        global = true
    )]
    registration_bearer_stdin: bool,

    /// Read rotating executor credentials from stdin; exit when the pipe closes.
    #[arg(
        long = "executor-credential-stdin",
        requires = "exec_server_remote",
        conflicts_with_all = ["use_agent_identity_auth"],
        global = true
    )]
    executor_credential_stdin: bool,

    /// Exit when the parent-owned standard-input pipe closes.
    #[arg(
        long = "exit-on-stdin-close",
        env = codex_exec_server::CODEX_EXEC_SERVER_EXIT_ON_STDIN_CLOSE_ENV_VAR,
        requires_if("true", "exec_server_remote"),
        global = true
    )]
    exit_on_stdin_close: bool,
}

impl ExecServerCommand {
    /// Starts the executor with the given runtime paths and configuration overrides.
    pub async fn run(
        mut self,
        arg0_paths: &Arg0DispatchPaths,
        root_config_overrides: &CliConfigOverrides,
    ) -> anyhow::Result<()> {
        let strict_config = self.strict_config;
        self.validate_remote_transport()?;
        let websocket_auth = self.websocket_auth.try_into_settings()?;
        if websocket_auth.config.is_some() && (self.remote.is_some() || self.command.is_some()) {
            anyhow::bail!("WebSocket listener auth cannot be used with --remote or forward");
        }
        let codex_self_exe = arg0_paths
            .codex_self_exe
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Codex executable path is not configured"))?;
        let runtime_paths = ExecServerRuntimeOptions::new(
            codex_self_exe,
            arg0_paths.codex_linux_sandbox_exe.clone(),
        )?
        .with_proxy_private_ips_via_upstream(self.proxy_private_ips_via_upstream);
        if let Some(base_url) = self.remote.take() {
            let environment_id = self.environment_id.take().ok_or_else(|| {
                anyhow::anyhow!("--environment-id is required when --remote is set")
            })?;
            let registration_auth = self
                .registration_bearer_stdin
                .then(codex_exec_server::registration_auth_from_stdin)
                .transpose()?;
            let config = load_exec_server_config(root_config_overrides, strict_config).await?;
            let mut parent_lifetime = if self.exit_on_stdin_close {
                ParentLifetime::StdinPipe
            } else {
                ParentLifetime::Independent
            };
            // Configured exporters may contain account-wide credentials or client keys.
            // The scoped executor must not initialize those exporters.
            let telemetry_config = (!self.executor_credential_stdin).then_some(&config);
            let (_otel, telemetry) = exec_server_telemetry::init(telemetry_config);
            let mut credential_provider = None;
            let auth_provider: codex_api::SharedAuthProvider = if self.executor_credential_stdin {
                let ExecutorCredentialPipe {
                    provider,
                    parent_closed,
                } = ExecutorCredentialPipe::from_stdin(environment_id.trim().to_string());
                provider.wait_for_initial().await?;
                parent_lifetime = ParentLifetime::MonitoredStdin(parent_closed);
                credential_provider = Some(Arc::clone(&provider));
                provider
            } else if let Some(provider) = registration_auth {
                provider
            } else {
                load_exec_server_remote_auth_provider(
                    &config,
                    &base_url,
                    self.use_agent_identity_auth,
                )
                .await?
            };
            let mut remote_config = codex_exec_server::RemoteEnvironmentConfig::new_with_transport(
                base_url,
                environment_id,
                codex_exec_server::RemoteEnvironmentTransport::Noise,
                auth_provider,
                config.http_client_factory(),
            )?;
            if self.executor_credential_stdin || self.registration_bearer_stdin {
                remote_config = remote_config.with_executor_credential_auth()?;
            }
            if let Some(name) = self.name {
                remote_config.name = name;
            }
            remote_config.request_dispatch_mode = self.request_dispatch_mode;
            let remote_config = remote_config.with_telemetry(telemetry);
            let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
            #[cfg(target_os = "macos")]
            let runtime_paths = runtime_paths.with_allowed_symlinked_codex_home(
                codex_config::allowed_symlinked_codex_home(
                    &config.config_layer_stack,
                    &config.codex_home,
                ),
            );
            let result = exec_server_telemetry::run_until_shutdown(
                async move {
                    let shutdown = async move {
                        let _ = shutdown_receiver.await;
                    };
                    match self.command {
                        Some(ExecServerSubcommand::Forward { connect }) => {
                            codex_exec_server::run_remote_environment_forward_until_shutdown(
                                remote_config,
                                connect,
                                shutdown,
                            )
                            .await
                        }
                        None => {
                            codex_exec_server::run_remote_environment_until_shutdown(
                                remote_config,
                                runtime_paths,
                                shutdown,
                            )
                            .await
                        }
                    }
                    .map_err(anyhow::Error::new)
                },
                parent_lifetime,
                exec_server_telemetry::ShutdownBehavior::Graceful(shutdown_sender),
            )
            .await;
            if let Some(provider) = credential_provider {
                provider.check_pipe_failure()?;
            }
            result
        } else {
            let config_result = load_exec_server_config(root_config_overrides, strict_config).await;
            let config = if strict_config {
                Some(config_result?)
            } else {
                config_result.ok()
            };
            let (_otel, telemetry) = exec_server_telemetry::init(config.as_ref());
            #[cfg(target_os = "macos")]
            let runtime_paths = runtime_paths.with_allowed_symlinked_codex_home(
                config.as_ref().and_then(|config| {
                    codex_config::allowed_symlinked_codex_home(
                        &config.config_layer_stack,
                        &config.codex_home,
                    )
                }),
            );
            let http_client_factory = config
                .as_ref()
                .map(Config::http_client_factory)
                .unwrap_or_else(|| HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault));
            let listen_url = self
                .listen
                .unwrap_or_else(|| codex_exec_server::DEFAULT_LISTEN_URL.to_string());
            let run = exec_server_telemetry::run_until_shutdown(
                codex_exec_server::run_main_with_telemetry(
                    &listen_url,
                    runtime_paths,
                    telemetry,
                    http_client_factory,
                    self.request_dispatch_mode,
                    websocket_auth,
                ),
                ParentLifetime::Independent,
                exec_server_telemetry::ShutdownBehavior::Immediate,
            );
            run.await.map_err(anyhow::Error::from_boxed)
        }
    }

    fn validate_remote_transport(&self) -> anyhow::Result<()> {
        // Clap checks global flag conflicts separately on either side of `forward`.
        if self.registration_bearer_stdin && self.executor_credential_stdin {
            anyhow::bail!(
                "--registration-bearer-stdin cannot be used with --executor-credential-stdin"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
enum ExecServerRemoteTransport {
    #[default]
    Noise,
}

#[derive(Debug, clap::Subcommand)]
enum ExecServerSubcommand {
    /// Register an existing WebSocket exec-server as a remote environment.
    Forward {
        /// Destination exec-server WebSocket URL.
        #[arg(long, value_name = "URL", requires = "exec_server_remote")]
        connect: String,
    },
}

#[cfg(test)]
#[path = "exec_server_args_tests.rs"]
mod tests;
