use super::*;
use codex_app_server_protocol::ConnectorMetadata;

const APP_READ_MAX_IDS: usize = 100;
const APPS_READ_DURATION_METRIC: &str = "codex.apps.read.duration_ms";

impl AppsRequestProcessor {
    pub(crate) async fn apps_read(
        &self,
        params: AppsReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let started_at = Instant::now();
        let AppsReadParams {
            app_ids,
            thread_id,
            include_tools,
        } = params;
        if app_ids.len() > APP_READ_MAX_IDS {
            return Err(invalid_params(format!(
                "app/read accepts at most {APP_READ_MAX_IDS} appIds"
            )));
        }

        let mut seen_app_ids = HashSet::new();
        let app_ids = app_ids
            .into_iter()
            .filter(|app_id| seen_app_ids.insert(app_id.clone()))
            .collect::<Vec<_>>();
        let mut available: HashMap<_, _> = self
            .accessible_apps(thread_id.as_deref(), false)
            .await?
            .into_iter()
            .map(|app| (app.id.clone(), app))
            .collect();
        let mut apps = Vec::new();
        let mut missing_app_ids = Vec::new();
        for id in app_ids {
            if let Some(app) = available.remove(&id) {
                apps.push(ConnectorMetadata {
                    id,
                    name: app.name,
                    description: app.description,
                    icon_url: app.logo_url,
                    icon_url_dark: app.logo_url_dark,
                    distribution_channel: app.distribution_channel,
                    install_url: app.install_url,
                    plugin_display_names: app.plugin_display_names,
                    tool_summaries: None,
                });
            } else {
                missing_app_ids.push(id);
            }
        }
        let response = AppsReadResponse {
            apps,
            missing_app_ids,
        };
        record_apps_read_duration(started_at, include_tools);
        Ok(Some(response.into()))
    }
}

fn record_apps_read_duration(started_at: Instant, include_tools: bool) {
    let include_tools = if include_tools { "true" } else { "false" };
    if let Some(metrics) = codex_otel::global() {
        let _ = metrics.record_duration(
            APPS_READ_DURATION_METRIC,
            started_at.elapsed(),
            &[("include_tools", include_tools)],
        );
    }
}
