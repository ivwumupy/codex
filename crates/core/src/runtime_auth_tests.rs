//! Runtime credential registration and destination environments.

#![cfg(test)]

use super::*;
use codex_model_provider::validate_runtime_environment;
use pretty_assertions::assert_eq;

#[test]
fn runtime_auth_rejects_a_different_registration_environment() {
    for (chatgpt_base_url, expected_staging) in [
        ("https://chatgpt.com/backend-api", false),
        ("https://chatgpt-staging.com/backend-api", true),
    ] {
        let runtime = RuntimeAuthManager::for_codex(
            AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("test-key")),
            chatgpt_base_url,
            &uuid::Uuid::nil().to_string(),
        );
        for (destination, staging) in [
            ("wss://api.openai.com/v1", false),
            ("https://chatgpt.com/backend-api", false),
            ("wss://api.openai-staging.com/v1", true),
            ("https://api.openai.org/v1", true),
            ("https://workspace.chatgpt-staging.com/backend-api", true),
            ("https://chatgpt-staging.com/backend-api", true),
        ] {
            assert_eq!(
                validate_runtime_environment(&runtime, Some(destination)).is_ok(),
                staging == expected_staging,
                "{chatgpt_base_url} -> {destination}",
            );
        }
    }
}
