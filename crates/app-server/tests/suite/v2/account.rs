use anyhow::Result;
use anyhow::bail;
use app_test_support::TestAppServer;
use app_test_support::to_response;

use app_test_support::ChatGptAuthFixture;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::write_chatgpt_auth;
use axum::routing::get;
use codex_app_server_protocol::Account;
use codex_app_server_protocol::AccountLoginCompletedNotification;
use codex_app_server_protocol::AccountUpdatedNotification;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::GetAuthStatusParams;
use codex_app_server_protocol::GetAuthStatusResponse;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LoginAccountResponse;
use codex_app_server_protocol::LogoutAccountResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::TurnStatus;
use codex_config::types::AuthCredentialsStoreMode;
use codex_http_client::HttpClientBuilder;
use codex_login::AuthDotJson;
use codex_login::AuthKeyringBackendKind;
use codex_login::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_login::load_auth_dot_json;
use codex_login::login_with_api_key;
use codex_protocol::auth::AuthMode as DomainAuthMode;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_case::test_case;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::time::timeout;
use url::Url;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const WORKSPACE_ID_INITIAL: &str = "123e4567-e89b-42d3-a456-426614174011";

#[path = "account_enterprise_tests.rs"]
mod enterprise_tests;

// Helper to create a minimal config.toml for the app server
#[derive(Default)]
struct CreateConfigTomlParams {
    forced_method: Option<String>,
    forced_workspace_id: Option<String>,
    forced_workspace_ids: Option<Vec<String>>,
    requires_openai_auth: Option<bool>,
    base_url: Option<String>,
    chatgpt_base_url: Option<String>,
    model_provider_id: Option<String>,
    extra_provider_config: Option<String>,
}

fn create_config_toml(codex_home: &Path, params: CreateConfigTomlParams) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    let base_url = params
        .base_url
        .unwrap_or_else(|| "http://127.0.0.1:0/v1".to_string());
    let forced_line = if let Some(method) = params.forced_method {
        format!("forced_login_method = \"{method}\"\n")
    } else {
        String::new()
    };
    let forced_workspace_line = if let Some(ws) = params.forced_workspace_id {
        format!("forced_chatgpt_workspace_id = \"{ws}\"\n")
    } else if let Some(workspaces) = params.forced_workspace_ids {
        let workspaces = workspaces
            .into_iter()
            .map(|workspace_id| format!("\"{workspace_id}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("forced_chatgpt_workspace_id = [{workspaces}]\n")
    } else {
        String::new()
    };
    let requires_line = match params.requires_openai_auth {
        Some(true) => "requires_openai_auth = true\n".to_string(),
        Some(false) => String::new(),
        None => String::new(),
    };
    let chatgpt_base_url_line = params
        .chatgpt_base_url
        .map(|url| format!("chatgpt_base_url = \"{url}\"\n"))
        .unwrap_or_default();
    let model_provider_id = params
        .model_provider_id
        .unwrap_or_else(|| "mock_provider".to_string());
    let provider_section = if model_provider_id == "mock_provider" {
        format!(
            r#"[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{base_url}"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
{requires_line}
"#
        )
    } else {
        params.extra_provider_config.unwrap_or_default()
    };
    let contents = format!(
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"
{chatgpt_base_url_line}
{forced_line}
{forced_workspace_line}

model_provider = "{model_provider_id}"

[features]
shell_snapshot = false

{provider_section}
"#
    );
    std::fs::write(config_toml, contents)
}

fn read_config_toml(codex_home: &Path) -> Result<toml::Value> {
    Ok(toml::from_str(&std::fs::read_to_string(
        codex_home.join("config.toml"),
    )?)?)
}

fn load_file_auth(codex_home: &Path) -> Result<Option<AuthDotJson>> {
    Ok(load_auth_dot_json(
        codex_home,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?)
}

async fn read_account(mcp: &mut TestAppServer) -> Result<GetAccountResponse> {
    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(request_id)).await?
}

async fn assert_account_updated(
    mcp: &mut TestAppServer,
    auth_mode: Option<AuthMode>,
) -> Result<()> {
    let payload: AccountUpdatedNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_notification("account/updated"),
    )
    .await??;
    assert_eq!(
        payload,
        AccountUpdatedNotification {
            auth_mode,
            plan_type: None,
        }
    );
    Ok(())
}

#[test_case(false; "api_key")]
#[test_case(true; "enterprise_cleanup_failure")]
#[tokio::test]
async fn logout_account_removes_auth_and_notifies(enterprise_cleanup_failure: bool) -> Result<()> {
    let oauth_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&oauth_server)
        .await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), CreateConfigTomlParams::default())?;

    if enterprise_cleanup_failure {
        write_chatgpt_auth(
            codex_home.path(),
            ChatGptAuthFixture::new("access-token")
                .account_id(WORKSPACE_ID_INITIAL)
                .chatgpt_user_id("enterprise-user"),
            AuthCredentialsStoreMode::File,
        )?;
        let mut config = read_config_toml(codex_home.path())?;
        config.as_table_mut().expect("config table").insert(
            "mcp_enterprise_managed_auth".into(),
            toml::toml! {
                [idp]
                issuer = "https://idp.example"
                client_id = "enterprise-client"
            }
            .into(),
        );
        std::fs::write(
            codex_home.path().join("config.toml"),
            toml::to_string(&config)?,
        )?;
        // Fail credential cleanup before touching the platform keyring.
        std::fs::write(codex_home.path().join("mcp-oauth-locks"), "not a directory")?;
    } else {
        login_with_api_key(
            codex_home.path(),
            "sk-test-key",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )?;
    }
    assert!(codex_home.path().join("auth.json").exists());

    let refresh_url = format!("{}/oauth/token", oauth_server.uri());
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            (REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR, Some(&refresh_url)),
        ])
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let id = mcp.send_logout_account_request().await?;
    let _ok: LogoutAccountResponse = timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(id)).await??;

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/updated"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountUpdated(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert!(
        payload.auth_mode.is_none(),
        "auth_method should be None after logout"
    );
    assert_eq!(payload.plan_type, None);

    assert!(
        !codex_home.path().join("auth.json").exists(),
        "auth.json should be deleted"
    );

    let get_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    let account: GetAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(get_id)).await??;
    assert_eq!(account.account, None);
    Ok(())
}

#[tokio::test]
async fn logout_account_requires_a_successful_auth_reload() -> Result<()> {
    let oauth_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&oauth_server)
        .await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), CreateConfigTomlParams::default())?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("access-a").refresh_token("refresh-a"),
        AuthCredentialsStoreMode::File,
    )?;
    let refresh_url = format!("{}/oauth/token", oauth_server.uri());
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            (REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR, Some(&refresh_url)),
        ])
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let auth_file = codex_home.path().join("auth.json");
    let unreadable = "{";
    std::fs::write(&auth_file, unreadable)?;
    let id = mcp.send_logout_account_request().await?;
    let error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(id)),
    )
    .await??;
    assert_eq!(error.error.code, -32603);
    assert_eq!(std::fs::read_to_string(&auth_file)?, unreadable);
    assert!(
        oauth_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() != "/oauth/revoke"),
    );

    // Once storage is readable, logout must remove the newly written credentials.
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("access-b").refresh_token("refresh-b"),
        AuthCredentialsStoreMode::File,
    )?;
    let id = mcp.send_logout_account_request().await?;
    let _: LogoutAccountResponse = timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(id)).await??;
    assert!(!auth_file.exists());
    oauth_server.verify().await;
    Ok(())
}

#[tokio::test]
async fn logout_account_succeeds_when_config_reload_fails() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), CreateConfigTomlParams::default())?;
    login_with_api_key(
        codex_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .with_env_overrides(&[("OPENAI_API_KEY", None)])
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    std::fs::write(codex_home.path().join("config.toml"), "invalid = [")?;

    let request_id = mcp.send_logout_account_request().await?;
    let response = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(
        to_response::<LogoutAccountResponse>(response)?,
        LogoutAccountResponse {}
    );
    assert_eq!(load_file_auth(codex_home.path())?, None);
    assert_account_updated(&mut mcp, /*auth_mode*/ None).await?;

    Ok(())
}

#[test_case(false, false; "local_auth_before_cloud")]
#[test_case(true, false; "local_mxc_after_cloud_selection")]
#[test_case(false, true; "cloud_mxc_without_default_recovery")]
#[tokio::test]
async fn startup_enforces_requirements_at_the_correct_load_stage(
    local_mxc: bool,
    cloud_mxc: bool,
) -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_server = MockServer::start().await;
    let require_mxc = "[windows]\nrequire_mxc = true\n";
    let fetch_cloud = local_mxc || cloud_mxc;
    if fetch_cloud {
        let bundle = codex_config::test_support::CloudConfigBundleFixture::enterprise_requirement(
            if cloud_mxc { require_mxc } else { "" },
        )
        .add_enterprise_config(format!("[features]\nprefer_mxc = {local_mxc}\n"))
        .into_bundle();
        Mock::given(path("/backend-api/wham/config/bundle"))
            .respond_with(ResponseTemplate::new(200).set_body_json(bundle))
            .expect(1..)
            .mount(&mock_server)
            .await;
    }
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            chatgpt_base_url: Some(format!("{}/backend-api", mock_server.uri())),
            ..Default::default()
        },
    )?;
    std::fs::write(
        codex_home.path().join("requirements.toml"),
        if local_mxc {
            require_mxc
        } else if cloud_mxc {
            ""
        } else {
            "allowed_login_methods = [\"api\"]\n"
        },
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .plan_type("enterprise")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123")
            .account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;

    if cfg!(windows) && fetch_cloud && (!local_mxc || !codex_sandboxing::windows_mxc_available()) {
        assert!(
            !timeout(DEFAULT_READ_TIMEOUT, mcp.wait_for_exit())
                .await??
                .success()
        );
    } else {
        timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
        if !fetch_cloud {
            for method in [
                "account/securitySetup/read",
                "account/rateLimits/read",
                "account/usage/read",
                "account/workspaceMessages/read",
            ] {
                let request_id = mcp.send_request(method, None).await?;
                let error = timeout(
                    DEFAULT_READ_TIMEOUT,
                    mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
                )
                .await??;
                assert_eq!(
                    error.error.message,
                    "ChatGPT account features are unsupported."
                );
            }
            assert_eq!(read_account(&mut mcp).await?.account, None);
        }
    }
    assert_eq!(
        mock_server
            .received_requests()
            .await
            .expect("recorded requests")
            .is_empty(),
        !fetch_cloud,
    );

    Ok(())
}

#[tokio::test]
async fn login_account_api_key_succeeds_and_notifies() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), CreateConfigTomlParams::default())?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let req_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let login: LoginAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(req_id)).await??;
    assert_eq!(login, LoginAccountResponse::ApiKey {});

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/login/completed"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountLoginCompleted(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    pretty_assertions::assert_eq!(payload.login_id, None);
    pretty_assertions::assert_eq!(payload.success, true);
    pretty_assertions::assert_eq!(payload.error, None);

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/updated"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountUpdated(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    pretty_assertions::assert_eq!(payload.auth_mode, Some(AuthMode::ApiKey));
    pretty_assertions::assert_eq!(payload.plan_type, None);

    assert!(codex_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn login_account_api_key_rejected_when_forced_chatgpt() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            forced_method: Some("chatgpt".to_string()),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::builder()
        .with_mock_chatgpt_backend()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let request_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(
        err.error.message,
        "API key login is disabled by this workspace."
    );
    Ok(())
}

#[tokio::test]
async fn cloud_login_methods_are_rejected_without_changing_the_account() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), CreateConfigTomlParams::default())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;

    mcp.initialize_with_capabilities(
        ClientInfo {
            name: DEFAULT_CLIENT_NAME.into(),
            title: None,
            version: "1".into(),
        },
        Some(InitializeCapabilities {
            experimental_api: true,
            ..Default::default()
        }),
    )
    .await?;
    let config_before = std::fs::read(codex_home.path().join("config.toml"))?;
    for params in [
        json!({"type": "amazonBedrock", "apiKey":"test-key", "region":"us-east-1"}),
        json!({"type": "amazonBedrockAccessKeys", "accessKeyId":"test-id", "secretAccessKey":"test-secret", "region":"us-east-1"}),
        json!({"type": "chatgpt"}),
        json!({"type": "chatgptDeviceCode"}),
        json!({"type": "chatgptAuthTokens", "accessToken": "irrelevant", "chatgptAccountId": "workspace"}),
    ] {
        let request_id = mcp
            .send_request("account/login/start", Some(params))
            .await?;
        let error = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, -32600);
        assert!(error.error.message.contains("unsupported"));
    }
    assert_eq!(read_account(&mut mcp).await?.account, None);
    assert_eq!(load_file_auth(codex_home.path())?, None);
    assert_eq!(
        std::fs::read(codex_home.path().join("config.toml"))?,
        config_before
    );
    Ok(())
}

#[tokio::test]
async fn get_account_no_auth() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .with_env_overrides(&[("OPENAI_API_KEY", None)])
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let params = GetAccountParams {
        refresh_token: false,
    };
    let request_id = mcp.send_get_account_request(params).await?;

    let account: GetAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(request_id)).await??;

    assert_eq!(
        account,
        GetAccountResponse {
            account: None,
            user_id: None,
            requires_openai_auth: true,
            workspace_routing: None,
        }
    );
    Ok(())
}

#[tokio::test]
async fn get_account_with_api_key() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let req_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let _login_ok: LoginAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(req_id)).await??;

    let params = GetAccountParams {
        refresh_token: false,
    };
    let request_id = mcp.send_get_account_request(params).await?;

    let received: GetAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(request_id)).await??;

    let expected = GetAccountResponse {
        user_id: None,
        workspace_routing: None,
        account: Some(Account::ApiKey {}),
        requires_openai_auth: true,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn get_account_when_auth_not_required() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(false),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let params = GetAccountParams {
        refresh_token: false,
    };
    let request_id = mcp.send_get_account_request(params).await?;

    let received: GetAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(request_id)).await??;

    let expected = GetAccountResponse {
        user_id: None,
        workspace_routing: None,
        account: None,
        requires_openai_auth: false,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn account_reads_use_startup_config_when_config_reload_fails() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            model_provider_id: Some("openai".to_owned()),
            ..Default::default()
        },
    )?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    std::fs::write(codex_home.path().join("config.toml"), "invalid = [")?;

    assert_eq!(
        read_account(&mut mcp).await?,
        GetAccountResponse {
            user_id: None,
            workspace_routing: None,
            account: None,
            requires_openai_auth: true,
        }
    );

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(false),
            refresh_token: Some(false),
        })
        .await?;
    let response = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(
        to_response::<GetAuthStatusResponse>(response)?,
        GetAuthStatusResponse {
            auth_method: None,
            auth_token: None,
            requires_openai_auth: Some(true),
        }
    );

    Ok(())
}

#[tokio::test]
async fn get_account_ignores_stored_chatgpt_credentials() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(
        codex_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("access-chatgpt")
            .account_id("account-123")
            .email("user@example.com")
            .chatgpt_user_id("user-123")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .with_env_overrides(&[("OPENAI_API_KEY", None)])
        .build_initialized_with_timeout(DEFAULT_READ_TIMEOUT)
        .await?;

    let params = GetAccountParams {
        refresh_token: false,
    };
    let request_id = mcp.send_get_account_request(params).await?;

    let received: GetAccountResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(request_id)).await??;

    let expected = GetAccountResponse {
        user_id: None,
        workspace_routing: None,
        account: None,
        requires_openai_auth: true,
    };
    assert_eq!(received, expected);
    Ok(())
}
