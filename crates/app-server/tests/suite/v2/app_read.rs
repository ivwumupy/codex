use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::AppsReadParams;
use codex_app_server_protocol::AppsReadResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn app_read_deserializes_legacy_tool_summaries() -> Result<()> {
    let response: AppsReadResponse = serde_json::from_value(json!({
        "apps": [{
            "id": "alpha",
            "name": "Alpha",
            "description": null,
            "iconUrl": null,
            "iconUrlDark": null,
            "distributionChannel": null,
            "installUrl": null,
            "pluginDisplayNames": [],
            "toolSummaries": [{
                "name": "search",
                "title": "Search",
                "description": "Search Alpha",
            }],
        }],
        "missingAppIds": [],
    }))?;

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "apps": [{
                "id": "alpha",
                "name": "Alpha",
                "description": null,
                "iconUrl": null,
                "iconUrlDark": null,
                "distributionChannel": null,
                "installUrl": null,
                "pluginDisplayNames": [],
                "toolSummaries": [{
                    "name": "search",
                    "title": "Search",
                    "description": "Search Alpha",
                    "isEnabled": true,
                    "disabledReason": null,
                    "isReadOnly": false,
                }],
            }],
            "missingAppIds": [],
        })
    );
    Ok(())
}

#[tokio::test]
async fn app_read_rejects_more_than_one_hundred_input_ids() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_TIMEOUT)
        .await?;

    let request_id = mcp
        .send_apps_read_request(AppsReadParams {
            app_ids: (0..101).map(|index| format!("app-{index}")).collect(),
            thread_id: None,
            include_tools: false,
        })
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.message, "app/read accepts at most 100 appIds");
    Ok(())
}
