use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::write_chatgpt_auth;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::CODEX_ACCESS_TOKEN_ENV_VAR;
use codex_login::REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_login::login_with_bedrock_access_keys;
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

const SITES_GIT_REQUEST: &str = "capability[]=authtype\nprotocol=https\nhost=chatgpt.com\npath=backend-api/codex/sites/git/fixture.git\n\n";

#[test]
fn git_credential_advertises_authtype_before_login() -> Result<()> {
    let codex_home = TempDir::new()?;
    codex_command(codex_home.path())?
        .args(["git-credential", "capability"])
        .assert()
        .success()
        .stdout(contains("capability authtype\n"));
    Ok(())
}

#[test]
fn git_credential_reads_only_the_first_record() -> Result<()> {
    let home = TempDir::new()?;
    codex_command(home.path())?
        .args(["git-credential", "get"])
        .write_stdin(format!(
            "protocol=https\nhost=chatgpt.com\n\n{SITES_GIT_REQUEST}"
        ))
        .assert()
        .success()
        .stdout("");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_credential_declines_personal_access_tokens_without_a_lookup() -> Result<()> {
    let home = TempDir::new()?;
    write_file_auth_config(home.path())?;
    let auth_api = MockServer::start().await;
    codex_command(home.path())?
        .env_remove(OPENAI_FEDERATION_RULE_ID_ENV_VAR)
        .env_remove(OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR)
        .env(CODEX_ACCESS_TOKEN_ENV_VAR, "at-fixture")
        .env("CODEX_AUTHAPI_BASE_URL", auth_api.uri())
        .args(["git-credential", "get"])
        .write_stdin(SITES_GIT_REQUEST)
        .assert()
        .success()
        .stdout("quit=true\n\n");
    assert!(auth_api.received_requests().await.unwrap().is_empty());
    Ok(())
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
            "--with-api-key",
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
        .args(["login", "--with-api-key"])
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
fn login_status_validates_configured_workload_identity() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;
    let missing_assertion = codex_home.path().join("missing-identity-token");

    for login_method in ["chatgpt", "api"] {
        let mut command = codex_command(codex_home.path())?;
        command
            .env_remove(CODEX_ACCESS_TOKEN_ENV_VAR)
            .env(OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR, &missing_assertion)
            .env(OPENAI_FEDERATION_RULE_ID_ENV_VAR, "rule-test")
            .args([
                "-c",
                &format!("forced_login_method=\"{login_method}\""),
                "login",
                "status",
            ]);
        command
            .assert()
            .failure()
            .stderr(contains("could not read workload identity assertion file"));
    }

    Ok(())
}

#[test]
fn logout_clears_only_the_selected_bedrock_provider() -> Result<()> {
    for (model_provider_id, managed_bedrock_auth, model) in [
        ("amazon-bedrock", true, "openai.gpt-5.6-sol"),
        ("amazon-bedrock-runtime", true, "global.openai.gpt-5.6-sol"),
        ("openai", true, "gpt-5.6-sol"),
        ("amazon-bedrock", false, "gpt-5.6-sol"),
        ("amazon-bedrock-runtime", false, "us.openai.gpt-5.6-sol"),
        ("openai", false, "gpt-5.6-sol"),
    ] {
        let codex_home = TempDir::new()?;
        let config_path = codex_home.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "cli_auth_credentials_store = \"file\"\n\
                 model_provider = \"{model_provider_id}\"\n\
                 model = \"{model}\"\n\
                 model_reasoning_effort = \"high\"\n\
                 [model_providers.amazon-bedrock]\n\
                 base_url = \"https://mantle.example.com/v1\"\n\
                 [model_providers.amazon-bedrock.aws]\n\
                 profile = \"mantle-profile\"\n\
                 region = \"us-west-2\"\n\
                 auth_refresh = {{ command = \"aws\", args = [\"sso\", \"login\"] }}\n\
                 [model_providers.amazon-bedrock-runtime]\n\
                 base_url = \"https://runtime.example.com/v1\"\n\
                 [model_providers.amazon-bedrock-runtime.aws]\n\
                 profile = \"runtime-profile\"\n\
                 region = \"us-east-1\"\n\
                 auth_refresh = {{ command = \"aws\", args = [\"login\"] }}\n"
            ),
        )?;
        if managed_bedrock_auth {
            login_with_bedrock_access_keys(
                codex_home.path(),
                "managed-access-key-id",
                "managed-secret-access-key",
                Some("managed-session-token"),
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
            )?;
        }
        let mut expected_config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&config_path)?)?;
        if model_provider_id != "openai" {
            let expected_root = expected_config
                .as_table_mut()
                .expect("config should be a table");
            expected_root.remove("model_provider");
            expected_root.remove("model");
            expected_root["model_providers"][model_provider_id]
                .as_table_mut()
                .expect("selected Bedrock provider should be a table")
                .remove("aws");
        }
        let expected_message = if managed_bedrock_auth || model_provider_id != "openai" {
            "Successfully logged out"
        } else {
            "Not logged in"
        };

        codex_command(codex_home.path())?
            .env_remove(CODEX_ACCESS_TOKEN_ENV_VAR)
            .env("AWS_ACCESS_KEY_ID", "environment-access-key-id")
            .env("AWS_SECRET_ACCESS_KEY", "environment-secret-access-key")
            .args(["logout"])
            .assert()
            .success()
            .stderr(contains(expected_message));

        assert!(!codex_home.path().join("auth.json").exists());
        let actual_config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
        assert_eq!(actual_config, expected_config);
    }

    Ok(())
}

#[test]
fn removed_login_methods_are_rejected() -> Result<()> {
    let home = TempDir::new()?;
    for flag in ["--device-auth", "--with-access-token"] {
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
