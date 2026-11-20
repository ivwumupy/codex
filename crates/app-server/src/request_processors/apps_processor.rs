use super::*;
use crate::app_info::app_info_to_api;
use codex_connectors::AppToolPolicyEvaluator;

mod installed;
mod read;

pub(crate) struct AppsRequestProcessor {
    thread_manager: Arc<ThreadManager>,
    config_manager: ConfigManager,
}

impl AppsRequestProcessor {
    pub(crate) fn new(thread_manager: Arc<ThreadManager>, config_manager: ConfigManager) -> Self {
        Self {
            thread_manager,
            config_manager,
        }
    }

    pub(crate) async fn apps_list(
        &self,
        params: AppsListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let start = params
            .cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| invalid_request("invalid app list cursor"))?;
        let apps = self
            .accessible_apps(params.thread_id.as_deref(), params.force_refetch)
            .await?;
        Ok(Some(paginate_apps(&apps, start, params.limit)?.into()))
    }

    async fn accessible_apps(
        &self,
        thread_id: Option<&str>,
        force_refetch: bool,
    ) -> Result<Vec<AppInfo>, JSONRPCErrorError> {
        let (config, mcp_manager) = self.load_apps_config(thread_id).await?;
        let status = connectors::list_accessible_connectors_from_mcp_tools_with_mcp_manager(
            &config,
            force_refetch,
            self.thread_manager.environment_manager(),
            mcp_manager,
        )
        .await
        .map_err(|err| internal_error(format!("failed to load accessible apps: {err}")))?;
        Ok(AppToolPolicyEvaluator::new(&config.config_layer_stack)
            .apply_app_enabled_state(status.connectors))
    }

    async fn load_thread(
        &self,
        thread_id: &str,
    ) -> Result<(ThreadId, Arc<CodexThread>), JSONRPCErrorError> {
        let thread_id = ThreadId::from_string(thread_id)
            .map_err(|err| invalid_request(format!("invalid thread id: {err}")))?;

        let thread = self
            .thread_manager
            .get_thread(thread_id)
            .await
            .map_err(|_| invalid_request(format!("thread not found: {thread_id}")))?;

        Ok((thread_id, thread))
    }

    async fn load_latest_config(
        &self,
        fallback_cwd: Option<PathBuf>,
    ) -> Result<Config, JSONRPCErrorError> {
        self.config_manager
            .load_latest_config(fallback_cwd)
            .await
            .map_err(|err| internal_error(format!("failed to reload config: {err}")))
    }

    async fn load_apps_config(
        &self,
        thread_id: Option<&str>,
    ) -> Result<(Config, Arc<McpManager>), JSONRPCErrorError> {
        let Some(thread_id) = thread_id else {
            let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
            self.thread_manager
                .runtime_auth_for_config(&config)
                .map_err(|error| internal_error(error.to_string()))?;
            return Ok((config, self.thread_manager.mcp_manager()));
        };
        let (_, thread) = self.load_thread(thread_id).await?;
        let thread_config = thread.config().await;
        let config = self
            .config_manager
            .load_latest_config_with_session_layers(
                &thread_config.config_layer_stack,
                &thread_config.cwd,
            )
            .await
            .map_err(|err| internal_error(format!("failed to reload config: {err}")))?;
        let mcp_manager = thread.mcp_manager();
        self.thread_manager
            .runtime_auth_for_mcp_manager(&config, &mcp_manager)
            .map_err(|error| internal_error(error.to_string()))?;
        Ok((config, mcp_manager))
    }
}

fn paginate_apps(
    connectors: &[AppInfo],
    start: usize,
    limit: Option<u32>,
) -> Result<AppsListResponse, JSONRPCErrorError> {
    let total = connectors.len();
    if start > total {
        return Err(invalid_request(format!(
            "cursor {start} exceeds total apps {total}"
        )));
    }

    let effective_limit = limit.unwrap_or(total as u32).max(1) as usize;
    let end = start.saturating_add(effective_limit).min(total);
    let data = connectors[start..end]
        .iter()
        .cloned()
        .map(app_info_to_api)
        .collect();
    let next_cursor = if end < total {
        Some(end.to_string())
    } else {
        None
    };

    Ok(AppsListResponse { data, next_cursor })
}
