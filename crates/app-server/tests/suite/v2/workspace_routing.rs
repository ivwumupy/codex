//! Check that using the public account API does not trigger workspace discovery.

use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::LoginAccountResponse;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::path;

const READ_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 60);

fn config(home: &TempDir, backend: &MockServer) -> Result<()> {
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "chatgpt_base_url = '{}/backend-api/'\ncli_auth_credentials_store = 'file'\n",
            backend.uri()
        ),
    )?;
    Ok(())
}

async fn start(home: &TempDir) -> Result<TestAppServer> {
    TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[("OPENAI_API_KEY", None)])
        .build_initialized_with_timeout(READ_TIMEOUT)
        .await
}

async fn read(server: &mut TestAppServer) -> Result<Value> {
    let request = server
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    timeout(READ_TIMEOUT, server.read_response(request)).await?
}

#[tokio::test]
async fn api_only_login_does_not_discover_chatgpt_routing() -> Result<()> {
    let backend = MockServer::start().await;
    Mock::given(path("/backend-api/wham/accounts/check"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&backend)
        .await;
    let home = TempDir::new()?;
    config(&home, &backend)?;
    std::fs::write(
        home.path().join("requirements.toml"),
        "allowed_login_methods = ['api']",
    )?;
    let mut server = start(&home).await?;
    let request = server.send_login_account_api_key_request("sk-test").await?;
    let _: LoginAccountResponse = timeout(READ_TIMEOUT, server.read_response(request)).await??;
    assert_eq!(
        read(&mut server).await?,
        json!({"account": {"type": "apiKey"}, "userId": null, "requiresOpenaiAuth": true, "workspaceRouting": null})
    );
    backend.verify().await;
    Ok(())
}
