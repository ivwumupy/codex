use std::path::Path;

use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::write_chatgpt_auth;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::CODEX_ACCESS_TOKEN_ENV_VAR;
use codex_login::REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_FEDERATION_RULE_ID_ENV_VAR;
use codex_protocol::shell_environment::OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::MockServer;

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEX_HOME", codex_home);
    Ok(cmd)
}

fn write_file_auth_config(codex_home: &Path) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    Ok(())
}

fn read_auth_json(codex_home: &Path) -> Result<Value> {
    let auth_json = std::fs::read_to_string(codex_home.join("auth.json"))?;
    Ok(serde_json::from_str(&auth_json)?)
}

#[test]
fn login_with_api_key_reads_stdin_and_writes_auth_json() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;

    let mut cmd = codex_command(codex_home.path())?;
    let login = cmd
        .args([
            "-c",
            "forced_login_method=\"api\"",
            "login",
        ])
        .write_stdin("sk-test\n")
        .assert()
        .success();
    insta::assert_snapshot!(String::from_utf8_lossy(&login.get_output().stderr), @r"
    Reading API key from stdin...
    API key saved. Authentication is checked when you send a request.
    ");

    let auth = read_auth_json(codex_home.path())?;
    assert_eq!(auth["OPENAI_API_KEY"], "sk-test");
    assert!(auth.get("tokens").is_none());
    assert!(auth.get("agent_identity").is_none());

    let status = codex_command(codex_home.path())?
        .env_remove(CODEX_ACCESS_TOKEN_ENV_VAR)
        .env_remove(OPENAI_FEDERATION_RULE_ID_ENV_VAR)
        .env_remove(OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR)
        .args(["login", "status"])
        .assert()
        .success();
    insta::assert_snapshot!(String::from_utf8_lossy(&status.get_output().stderr), @r"
    API key configured - ***
    Authentication is checked when you send a request.
    ");

    codex_command(codex_home.path())?
        .arg("login")
        .write_stdin("replacement-key\n")
        .assert()
        .success();
    assert_eq!(
        read_auth_json(codex_home.path())?["OPENAI_API_KEY"],
        "replacement-key"
    );

    Ok(())
}

#[test]
fn api_key_auth_commands_work_when_log_dir_is_unavailable() -> Result<()> {
    let codex_home = TempDir::new()?;
    let log_dir = codex_home.path().join("unavailable-logs");
    std::fs::write(&log_dir, "not a directory")?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!("cli_auth_credentials_store = \"file\"\nlog_dir = {log_dir:?}\n"),
    )?;

    codex_command(codex_home.path())?
        .arg("login")
        .write_stdin("sk-test\n")
        .assert()
        .success()
        .stderr(contains("Warning: failed to create login log directory"));
    assert_eq!(
        read_auth_json(codex_home.path())?["OPENAI_API_KEY"],
        "sk-test"
    );

    codex_command(codex_home.path())?
        .args(["login", "status"])
        .assert()
        .success()
        .stderr(contains("API key"));

    codex_command(codex_home.path())?
        .arg("logout")
        .assert()
        .success()
        .stderr(contains("Successfully logged out"));
    assert!(!codex_home.path().join("auth.json").exists());

    Ok(())
}

#[test]
fn login_status_reports_auth_storage_errors() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;
    std::fs::write(codex_home.path().join("auth.json"), "{invalid json")?;

    codex_command(codex_home.path())?
        .args(["login", "status"])
        .assert()
        .failure()
        .stderr(contains("Error checking login status:"));

    Ok(())
}

#[test]
fn stored_bedrock_key_is_ignored() -> Result<()> {
    let home = TempDir::new()?;
    write_file_auth_config(home.path())?;
    std::fs::write(
        home.path().join("auth.json"),
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"old-key","region":"us-east-1"}}"#,
    )?;
    codex_command(home.path())?
        .args(["login", "status"])
        .assert()
        .failure()
        .stderr(contains("Not logged in"));
    Ok(())
}

#[test]
fn removed_login_methods_are_rejected() -> Result<()> {
    let home = TempDir::new()?;
    for flag in [
        "--device-auth",
        "--with-access-token",
        "--api-key",
        "--with-api-key",
    ] {
        codex_command(home.path())?
            .args(["login", flag])
            .assert()
            .failure()
            .stderr(contains("unexpected argument"));
    }
    Ok(())
}

#[tokio::test]
async fn stored_chatgpt_session_is_ignored_without_contacting_backend() -> Result<()> {
    let server = MockServer::start().await;
    let home = TempDir::new()?;
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "cli_auth_credentials_store = 'file'\nchatgpt_base_url = '{}/backend-api'\n",
            server.uri()
        ),
    )?;
    write_chatgpt_auth(
        home.path(),
        ChatGptAuthFixture::new("old-access")
            .account_id("workspace")
            .chatgpt_user_id("user"),
        AuthCredentialsStoreMode::File,
    )?;
    codex_command(home.path())?
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove(CODEX_ACCESS_TOKEN_ENV_VAR)
        .args(["login", "status"])
        .assert()
        .failure()
        .stderr(contains("Not logged in"));
    codex_command(home.path())?
        .env(
            REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR,
            format!("{}/oauth/revoke", server.uri()),
        )
        .arg("logout")
        .assert()
        .success()
        .stderr(contains("Successfully logged out"));
    assert!(!home.path().join("auth.json").exists());
    assert!(server.received_requests().await.unwrap().is_empty());
    Ok(())
}
