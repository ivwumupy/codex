use super::*;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn runtime_internal_metadata_opt_in_is_not_serialized_or_configurable() {
    let trusted = ModelProviderInfo {
        include_internal_metadata: true,
        ..Default::default()
    };
    assert_eq!(
        toml::to_string(&trusted).unwrap(),
        toml::to_string(&ModelProviderInfo::default()).unwrap(),
    );
    assert_eq!(
        toml::from_str::<ModelProviderInfo>("include_internal_metadata = true").unwrap(),
        ModelProviderInfo::default(),
    );
}

#[test]
fn test_api_provider_applies_current_managed_residency() {
    let info = ModelProviderInfo {
        http_headers: Some(maplit::hashmap! {
            "X-OpenAI-Internal-Codex-Residency".to_string() => "eu".into(),
            "x-provider-header".to_string() => "preserved".into(),
        }),
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };
    let original_info = info.clone();
    let previous_requirement = read_managed_residency_requirement();
    set_managed_residency_requirement(Some(ResidencyRequirement::Us));
    let managed = info.to_api_provider(/*auth_mode*/ None);
    set_managed_residency_requirement(/*enforce_residency*/ None);
    let unmanaged = info.to_api_provider(/*auth_mode*/ None);
    set_managed_residency_requirement(previous_requirement);

    assert_eq!(
        managed.expect("managed provider should resolve").headers,
        HeaderMap::from_iter([
            (
                HeaderName::from_static(RESIDENCY_HEADER_NAME),
                HeaderValue::from_static("us")
            ),
            (
                HeaderName::from_static("x-provider-header"),
                HeaderValue::from_static("preserved")
            ),
        ])
    );
    assert_eq!(
        unmanaged
            .expect("unmanaged provider should resolve")
            .headers,
        HeaderMap::from_iter([
            (
                HeaderName::from_static(RESIDENCY_HEADER_NAME),
                HeaderValue::from_static("eu")
            ),
            (
                HeaderName::from_static("x-provider-header"),
                HeaderValue::from_static("preserved")
            ),
        ])
    );
    assert_eq!(info, original_info);
}

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        gateway_oauth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        capabilities: None,
        include_internal_metadata: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.openai.azure.com/openai".into()),
        model_catalog_url: None,
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        gateway_oauth: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".into(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        capabilities: None,
        include_internal_metadata: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
supports_standalone_web_search = true
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        model_catalog_url: None,
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        gateway_oauth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".into(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: true,
        capabilities: None,
        include_internal_metadata: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api_shows_helpful_error() {
    let provider_toml = r#"
name = "OpenAI using Chat Completions"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
wire_api = "chat"
        "#;

    let err = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap_err();
    assert!(err.to_string().contains(CHAT_WIRE_API_REMOVED_ERROR));
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.openai.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn codex_backend_routes_require_codex_base_url() {
    for (base_url, expected) in [
        (None, true),
        (Some(CHATGPT_CODEX_BASE_URL), true),
        (Some("https://chatgpt-staging.com/backend-api/codex/"), true),
        (Some("https://backend.example.com/api/codex"), false),
        (Some("https://backend.example.com/api/codex/"), false),
        (Some("https://proxy.example.com/v1"), false),
    ] {
        let provider = ModelProviderInfo::create_openai_provider(base_url.map(str::to_owned));
        assert_eq!(provider.supports_codex_backend_routes(), expected);
    }
}

#[test]
fn test_uses_openai_actor_authorization() {
    let mut provider = ModelProviderInfo {
        http_headers: Some(maplit::hashmap! {
            "X-OpenAI-Actor-Authorization".to_string() => "actor-token".into(),
        }),
        ..ModelProviderInfo::default()
    };
    assert!(provider.uses_openai_actor_authorization());

    provider.http_headers = None;
    assert!(!provider.uses_openai_actor_authorization());

    provider.http_headers = Some(maplit::hashmap! {
        OPENAI_ACTOR_AUTHORIZATION_HEADER.to_string() => "  ".into(),
    });
    assert!(!provider.uses_openai_actor_authorization());

    provider.http_headers = Some(maplit::hashmap! {
        OPENAI_ACTOR_AUTHORIZATION_HEADER.to_string() => "actor-token".into(),
    });
    provider.requires_openai_auth = true;
    assert!(!provider.uses_openai_actor_authorization());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".into()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn built_in_providers_only_include_openai() {
    let providers = built_in_model_providers(None);
    assert_eq!(providers.len(), 1);
    assert!(providers.contains_key(OPENAI_PROVIDER_ID));
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn cloud_account_provider_overrides_are_rejected() {
    for id in [
        AMAZON_BEDROCK_PROVIDER_ID,
        AMAZON_BEDROCK_RUNTIME_PROVIDER_ID,
        GCP_PROVIDER_ID,
    ] {
        let configured = HashMap::from([(id.to_string(), ModelProviderInfo::default())]);
        assert!(
            merge_configured_model_providers(built_in_model_providers(None), configured).is_err()
        );
    }
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}

#[test]
fn model_catalog_url_deserializes_without_changing_inference_routing() {
    let provider: ModelProviderInfo = toml::from_str(
        r#"
name = "Gateway"
base_url = "https://gateway.example/v1"
model_catalog_url = "https://gateway.example/codex/catalog?token=catalog-secret"
"#,
    )
    .unwrap();
    assert!(!format!("{provider:?}").contains("catalog-secret"));
    assert_eq!(
        provider,
        ModelProviderInfo {
            name: "Gateway".to_string(),
            base_url: Some("https://gateway.example/v1".to_string()),
            model_catalog_url: Some(
                "https://gateway.example/codex/catalog?token=catalog-secret".into()
            ),
            ..ModelProviderInfo::default()
        }
    );
    assert_eq!(
        provider
            .to_api_provider(Some(AuthMode::ApiKey))
            .unwrap()
            .base_url,
        "https://gateway.example/v1"
    );
}

#[test]
fn builtin_openai_catalog_override_preserves_inference_and_identity() {
    for inference_url in [None, Some("https://us.api.openai.com/v1")] {
        for catalog_url in [None, Some("https://api.openai.com/codex/catalog")] {
            let configured: ModelProviderInfo = toml::from_str(&format!(
                "name = \"Ignored name\"\nbase_url = \"https://ignored.example/v1\"\nrequires_openai_auth = false\n{}",
                catalog_url.map(|url| format!("model_catalog_url = {url:?}")).unwrap_or_default()
            )).unwrap();
            let builtins = built_in_model_providers(inference_url.map(str::to_owned));
            let mut expected = builtins.clone();
            expected.get_mut("openai").unwrap().model_catalog_url = catalog_url.map(Into::into);
            assert_eq!(
                merge_configured_model_providers(
                    builtins,
                    HashMap::from([("openai".to_string(), configured)]),
                )
                .unwrap(),
                expected,
            );
        }
    }
}

#[test]
fn builtin_openai_catalog_rejects_untrusted_destinations() {
    for url in [
        "http://chatgpt.com/backend-api/codex/models",
        "https://chatgpt.com.evil.example/models",
        "https://evilchatgpt.com/models",
        "https://chatgpt.com/backend-api/codex/models",
        "https://example.com/models?secret=do-not-log",
        "https://user:secret@chatgpt.com/models",
        "/models",
        "not a URL",
    ] {
        let provider = ModelProviderInfo {
            model_catalog_url: Some(url.into()),
            ..Default::default()
        };
        assert_eq!(
            merge_configured_model_providers(
                built_in_model_providers(/*openai_base_url*/ None),
                HashMap::from([("openai".to_string(), provider.clone())]),
            )
            .unwrap_err(),
            "model_providers.openai.model_catalog_url must use an OpenAI API HTTPS endpoint. If you need to use a 3rd party host, configure a custom model_provider",
        );
        let mut expected = built_in_model_providers(/*openai_base_url*/ None);
        expected.insert("custom".into(), provider.clone());
        assert_eq!(
            merge_configured_model_providers(
                built_in_model_providers(/*openai_base_url*/ None),
                HashMap::from([("custom".into(), provider)]),
            )
            .unwrap(),
            expected,
        );
    }
}

#[test]
fn builtin_openai_catalog_accepts_openai_api_hosts() {
    for host in ["api.openai.com", "gov.api.openai.com", "eu.api.openai.com"] {
        let url = format!("https://{host}/codex/catalog");
        let provider = ModelProviderInfo {
            model_catalog_url: Some(url.clone().into()),
            ..Default::default()
        };
        let mut expected = built_in_model_providers(/*openai_base_url*/ None);
        expected.get_mut("openai").unwrap().model_catalog_url = Some(url.into());
        assert_eq!(
            merge_configured_model_providers(
                built_in_model_providers(/*openai_base_url*/ None),
                HashMap::from([("openai".into(), provider)]),
            )
            .unwrap(),
            expected
        );
    }
}
