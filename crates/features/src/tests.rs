use crate::Feature;
use crate::FeatureToml;
use crate::Features;
use crate::FeaturesToml;
use crate::Stage;
use crate::feature_for_key;
use crate::unstable_features_warning_event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use toml::Table;
use toml::Value as TomlValue;

#[test]
fn guardian_transcript_mode_rejects_invalid_values() {
    for input in [
        "[guardianv2]\ntranscript_mode = 'xml'",
        "[guardianv2]\ntranscript_mode = true",
    ] {
        assert!(toml::from_str::<FeaturesToml>(input).is_err());
    }
}

#[test]
fn sleep_tool_config_rejects_unknown_mode() {
    assert!(toml::from_str::<FeaturesToml>("[sleep_tool]\nmode = 'off'").is_err());
}

#[test]
fn runtime_auth_config_accepts_modes_without_implicitly_enabling_the_feature() {
    for mode in ["dual_header", "assertion_only"] {
        let config: FeaturesToml =
            toml::from_str(&format!("[runtime_agent_instances]\nmode = '{mode}'")).unwrap();
        let mut features = Features::with_defaults();
        features.apply_toml(&config);
        assert!(!features.enabled(Feature::RuntimeAgentInstances));

        let config: FeaturesToml = toml::from_str(&format!(
            "[runtime_agent_instances]\nenabled = true\nmode = '{mode}'"
        ))
        .unwrap();
        features.apply_toml(&config);
        assert!(features.enabled(Feature::RuntimeAgentInstances));
    }
    assert!(
        toml::from_str::<FeaturesToml>("[runtime_agent_instances]\nmode = 'credential_only'")
            .is_err()
    );
}

#[test]
fn under_development_features_are_disabled_by_default() {
    for spec in crate::FEATURES {
        // Instant interrupts are enabled by default while retaining their development stage.
        if matches!(spec.stage, Stage::UnderDevelopment) && spec.id != Feature::InstantInterrupt {
            assert_eq!(
                spec.default_enabled, false,
                "feature `{}` is under development and must be disabled by default",
                spec.key
            );
        }
    }
}

#[test]
fn tool_registry_config_is_not_a_feature_toggle() {
    let features: FeaturesToml = toml::from_str(
        "[tool_registry]\nerror_on_tool_collisions = true\nturn_metadata_includes_tool_info = true\n",
    )
    .expect("tool registry settings should deserialize");

    assert_eq!(
        features.tool_registry,
        Some(crate::ToolRegistryConfigToml {
            error_on_tool_collisions: Some(true),
            turn_metadata_includes_tool_info: Some(true),
        })
    );
    assert!(features.entries().is_empty());
    assert!(crate::is_known_feature_key("tool_registry"));
    assert_eq!(feature_for_key("tool_registry"), None);
}

#[test]
fn executor_capability_discovery_is_an_opt_in_map_feature() {
    let mut features = Features::with_defaults();
    assert!(!features.enabled(Feature::ExecutorCapabilityDiscovery));

    features.apply_map(&BTreeMap::from([(
        "executor_capability_discovery".to_string(),
        true,
    )]));

    assert!(features.enabled(Feature::ExecutorCapabilityDiscovery));
}

#[test]
fn cwd_relative_turn_diffs_is_an_opt_in_map_feature() {
    let mut features = Features::with_defaults();
    assert!(!features.enabled(Feature::CwdRelativeTurnDiffs));

    features.apply_map(&BTreeMap::from([(
        "cwd_relative_turn_diffs".to_string(),
        true,
    )]));

    assert!(features.enabled(Feature::CwdRelativeTurnDiffs));

    features.apply_map(&BTreeMap::from([(
        "cwd_relative_turn_diffs".to_string(),
        false,
    )]));

    assert!(!features.enabled(Feature::CwdRelativeTurnDiffs));
}

#[test]
fn codex_apps_mcp_protocol_can_be_enabled_independently_of_generic_mcp() {
    let features_toml = FeaturesToml::from(BTreeMap::from([
        (Feature::CodexAppsMcp20260728.key().to_string(), true),
        (Feature::Mcp20260728.key().to_string(), false),
    ]));
    let features = Features::from_sources(
        Some(&features_toml),
        None,
    );

    assert_eq!(
        (
            features.enabled(Feature::CodexAppsMcp20260728),
            features.enabled(Feature::Mcp20260728),
        ),
        (true, false),
    );
}

#[test]
fn default_enabled_features_have_shipped() {
    for spec in crate::FEATURES {
        // Instant interrupts are enabled by default while retaining their development stage.
        if spec.default_enabled && spec.id != Feature::InstantInterrupt {
            assert!(
                matches!(
                    spec.stage,
                    Stage::Stable
                ),
                "feature `{}` is enabled by default at stage {:?}",
                spec.key,
                spec.stage
            );
        }
    }
}

#[test]
fn code_mode_only_requires_code_mode() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeModeOnly);
    features.normalize_dependencies();

    assert_eq!(features.enabled(Feature::CodeModeOnly), true);
    assert_eq!(features.enabled(Feature::CodeMode), true);
}

#[test]
fn code_mode_host_feature_config_preserves_boolean_toggle() {
    let features: FeaturesToml =
        toml::from_str("code_mode_host = false").expect("features table should deserialize");

    assert_eq!(features.code_mode_host, Some(FeatureToml::Enabled(false)));
    assert_eq!(
        features.entries(),
        BTreeMap::from([("code_mode_host".to_string(), false)])
    );
}

#[test]
fn guardian_v2_feature_config_preserves_boolean_toggle() {
    let features: FeaturesToml =
        toml::from_str("guardianv2 = true").expect("Guardian v2 boolean should deserialize");

    assert_eq!(features.guardianv2, Some(FeatureToml::Enabled(true)));
    assert_eq!(
        features.entries(),
        BTreeMap::from([("guardianv2".to_owned(), true)])
    );
}

#[test]
fn guardian_v3_feature_config_resolves_base_and_profile_toggles() {
    for (base, profile, enabled) in [
        ("guardianv3 = true", "", true),
        ("guardianv3 = false", "", false),
        ("[guardianv3]", "", false),
        ("[guardianv3]\nenabled = true", "", true),
        ("[guardianv3]\nenabled = false", "", false),
        (
            "[guardianv3]\nenabled = true",
            "[guardianv3]\nenabled = false",
            false,
        ),
        (
            "[guardianv3]\nenabled = false",
            "[guardianv3]\nenabled = true",
            true,
        ),
        ("[guardianv3]\nenabled = true", "[guardianv3]", true),
    ] {
        let base = toml::from_str::<FeaturesToml>(base).expect("base features");
        let profile = toml::from_str::<FeaturesToml>(profile).expect("profile features");
        let features = Features::from_sources(
            Some(&base),
            Some(&profile),
        );
        assert_eq!(features.enabled(Feature::GuardianV3), enabled);
    }
}

#[test]
fn guardian_v2_feature_config_deserializes_classifier_and_transcript_settings() {
    let features: FeaturesToml = toml::from_str(
        r#"
[guardianv2]
enabled = true
persist_scores = true
classifier_instructions = "Review this action"
review_threshold = 0.65
max_tool_call_lag = 2
async_classifier_conversation_token_limit = 120000
reasoning_effort = "minimal"
max_action_tokens = 512
max_classifier_instruction_tokens = 256
reuse_parent_compaction = false
max_parent_compaction_tokens = 384

[guardianv2.review_scope]
computer_use_only = true
sandboxed_exec_commands = true

[guardianv2.transcript]
sources = ["tool_outputs", "reasoning"]
include_images = true
max_message_entry_tokens = 128
max_tool_entry_tokens = 128
max_message_transcript_tokens = 512
max_tool_transcript_tokens = 256
max_recent_non_user_entries = 12
"#,
    )
    .expect("Guardian v2 settings should deserialize");

    assert_eq!(
        features.guardianv2,
        Some(FeatureToml::Config(crate::GuardianV2ConfigToml {
            sync_elastic_budgeting: None,
            sync_elastic_sections: None,
            transcript_mode: None,
            async_classifier_conversation_token_limit: Some(120_000),
            enabled: Some(true),
            persist_scores: Some(true),
            classifier_instructions: Some("Review this action".to_owned()),
            review_threshold: Some(0.65),
            max_tool_call_lag: Some(2),
            reasoning_effort: Some(codex_protocol::openai_models::ReasoningEffort::Minimal),
            max_action_tokens: Some(512),
            max_classifier_instruction_tokens: Some(256),
            reuse_parent_compaction: Some(false),
            max_parent_compaction_tokens: Some(384),
            review_scope: Some(crate::GuardianV2ReviewScopeConfigToml {
                computer_use_only: Some(true),
                sandboxed_exec_commands: Some(true),
            }),
            transcript: Some(crate::GuardianV2TranscriptConfigToml {
                sources: Some(vec![
                    crate::GuardianV2TranscriptSource::ToolOutputs,
                    crate::GuardianV2TranscriptSource::Reasoning,
                ]),
                include_images: Some(true),
                max_message_entry_tokens: Some(128),
                max_tool_entry_tokens: Some(128),
                max_message_transcript_tokens: Some(512),
                max_tool_transcript_tokens: Some(256),
                max_recent_non_user_entries: Some(12),
            }),
        }))
    );
    assert_eq!(
        features.entries(),
        BTreeMap::from([("guardianv2".to_owned(), true)])
    );
}

#[test]
fn guardian_v2_feature_config_rejects_invalid_settings() {
    for setting in [
        "max_action_tokens",
        "max_classifier_instruction_tokens",
        "max_parent_compaction_tokens",
        "transcript.max_message_entry_tokens",
        "transcript.max_tool_entry_tokens",
        "transcript.max_message_transcript_tokens",
        "transcript.max_tool_transcript_tokens",
    ] {
        for value in [99, 100_001] {
            let settings = format!("[guardianv2]\n{setting} = {value}");
            assert!(
                toml::from_str::<FeaturesToml>(&settings).is_err(),
                "out-of-range Guardian v2 setting should fail: {setting} = {value}"
            );
        }
    }

    for settings in [
        "review_threshold = -0.1",
        "review_threshold = 1.1",
        "review_threshold = nan",
        "async_classifier_conversation_token_limit = 0",
        "transcript.max_recent_non_user_entries = 0",
        "transcript.max_message_entry_tokens = 200\ntranscript.max_message_transcript_tokens = 100",
        "transcript.max_tool_entry_tokens = 200\ntranscript.max_tool_transcript_tokens = 100",
    ] {
        assert!(
            toml::from_str::<FeaturesToml>(&format!("[guardianv2]\n{settings}")).is_err(),
            "invalid Guardian v2 settings should fail: {settings}"
        );
    }
}

#[test]
fn guardian_v2_feature_config_accepts_boundary_values() {
    for (threshold, tokens) in [(0.0, 100), (1.0, 100_000)] {
        let features: FeaturesToml = toml::from_str(&format!(
            "[guardianv2]\n\
             review_threshold = {threshold:.1}\n\
             max_action_tokens = {tokens}\n\
             max_classifier_instruction_tokens = {tokens}\n\
             max_parent_compaction_tokens = {tokens}\n\
             [guardianv2.transcript]\n\
             max_message_entry_tokens = {tokens}\n\
             max_tool_entry_tokens = {tokens}\n\
             max_message_transcript_tokens = {tokens}\n\
             max_tool_transcript_tokens = {tokens}\n\
             max_recent_non_user_entries = 1"
        ))
        .expect("Guardian v2 boundary settings should deserialize");

        assert!(matches!(features.guardianv2, Some(FeatureToml::Config(_))));
    }

    for setting in [
        "max_message_transcript_tokens",
        "max_tool_transcript_tokens",
    ] {
        let features: FeaturesToml =
            toml::from_str(&format!("[guardianv2.transcript]\n{setting} = 100"))
                .expect("partial Guardian v2 transcript settings should deserialize");

        assert!(matches!(features.guardianv2, Some(FeatureToml::Config(_))));
    }
}

#[test]
fn code_mode_host_feature_config_deserializes_fallback_setting() {
    let features: FeaturesToml = toml::from_str(
        r#"
[code_mode_host]
enabled = true
disable_in_process_fallback = true
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.code_mode_host,
        Some(FeatureToml::Config(crate::CodeModeHostConfigToml {
            enabled: Some(true),
            disable_in_process_fallback: Some(true),
        }))
    );
    assert_eq!(
        features.entries(),
        BTreeMap::from([("code_mode_host".to_string(), true)])
    );
}

#[test]
fn image_generation_toggle_controls_extension_backed_generation() {
    let mut entries = BTreeMap::new();
    entries.insert("image_generation".to_string(), false);
    let mut features = Features::with_defaults();
    features.apply_map(&entries);
    assert!(!features.enabled(Feature::ImageGeneration));

    entries.insert("image_generation".to_string(), true);
    features.disable(Feature::ImageGeneration);
    features.apply_map(&entries);
    assert!(features.enabled(Feature::ImageGeneration));
}

#[test]
fn apps_require_feature_flag_and_chatgpt_auth() {
    let mut features = Features::with_defaults();
    assert!(!features.apps_enabled_for_auth(/*has_chatgpt_auth*/ false));

    features.enable(Feature::Apps);
    assert!(!features.apps_enabled_for_auth(/*has_chatgpt_auth*/ false));
    assert!(features.apps_enabled_for_auth(/*has_chatgpt_auth*/ true));
}

#[test]
fn from_sources_applies_base_and_profile() {
    let mut base_entries = BTreeMap::new();
    base_entries.insert("plugins".to_string(), true);
    let base_features = FeaturesToml {
        entries: base_entries,
        ..Default::default()
    };

    let mut profile_entries = BTreeMap::new();
    profile_entries.insert("code_mode_only".to_string(), true);
    let profile_features = FeaturesToml {
        entries: profile_entries,
        ..Default::default()
    };

    let features = Features::from_sources(
        Some(&base_features),
        Some(&profile_features),
    );

    assert_eq!(features.enabled(Feature::Plugins), true);
    assert_eq!(features.enabled(Feature::CodeModeOnly), true);
    assert_eq!(features.enabled(Feature::CodeMode), true);
}

#[test]
fn multi_agent_v2_feature_config_deserializes_boolean_toggle() {
    let features: FeaturesToml = toml::from_str(
        r#"
multi_agent_v2 = true
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("multi_agent_v2".to_string(), true)])
    );
    assert_eq!(features.multi_agent_v2, Some(FeatureToml::Enabled(true)));
}

#[test]
fn multi_agent_v2_feature_config_deserializes_table() {
    let features: FeaturesToml = toml::from_str(
        r#"
[multi_agent_v2]
enabled = true
max_concurrent_threads_per_session = 4
min_wait_timeout_ms = 2500
max_wait_timeout_ms = 120000
default_wait_timeout_ms = 30000
usage_hint_text = "Custom delegation guidance."
root_agent_usage_hint_text = "Root guidance."
subagent_usage_hint_text = "Subagent guidance."
subagent_developer_instructions = "Delegate carefully."
preserve_fork_prefix = true
multi_agent_mode_hint_text = "Custom mode guidance."
tool_namespace = "agents"
hide_spawn_agent_metadata = true
expose_spawn_agent_model_overrides = true
wait_agent_enabled = false
disable_native_tools = true
disable_direct_message = true
message_board_in_memory = true
non_code_mode_only = true
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("multi_agent_v2".to_string(), true)])
    );
    assert_eq!(
        features.multi_agent_v2,
        Some(crate::FeatureToml::Config(crate::MultiAgentV2ConfigToml {
            enabled: Some(true),
            max_concurrent_threads_per_session: Some(4),
            min_wait_timeout_ms: Some(2500),
            max_wait_timeout_ms: Some(120000),
            default_wait_timeout_ms: Some(30000),
            usage_hint_text: Some("Custom delegation guidance.".to_string()),
            root_agent_usage_hint_text: Some("Root guidance.".to_string()),
            subagent_usage_hint_text: Some("Subagent guidance.".to_string()),
            subagent_developer_instructions: Some("Delegate carefully.".to_string()),
            preserve_fork_prefix: Some(true),
            multi_agent_mode_hint_text: Some("Custom mode guidance.".to_string()),
            tool_namespace: Some("agents".to_string()),
            hide_spawn_agent_metadata: Some(true),
            expose_spawn_agent_model_overrides: Some(true),
            wait_agent_enabled: Some(false),
            disable_native_tools: Some(true),
            disable_direct_message: Some(true),
            message_board_in_memory: Some(true),
            message_board_remote: None,
            non_code_mode_only: Some(true),
        }))
    );
}

#[test]
fn non_prefixed_mcp_tool_names_feature_config_deserializes_boolean_toggle() {
    let features: FeaturesToml = toml::from_str("non_prefixed_mcp_tool_names = true")
        .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("non_prefixed_mcp_tool_names".to_string(), true)])
    );
    assert_eq!(
        features.non_prefixed_mcp_tool_names,
        Some(FeatureToml::Enabled(true))
    );
}

#[test]
fn non_prefixed_mcp_tool_names_feature_config_deserializes_table() {
    let features: FeaturesToml = toml::from_str(
        r#"
[non_prefixed_mcp_tool_names]
enabled = true
server_names = ["history", "notes"]
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("non_prefixed_mcp_tool_names".to_string(), true)])
    );
    assert_eq!(
        features.non_prefixed_mcp_tool_names,
        Some(FeatureToml::Config(
            crate::NonPrefixedMcpToolNamesConfigToml {
                enabled: Some(true),
                server_names: Some(vec!["history".to_string(), "notes".to_string()]),
            }
        ))
    );
}

#[test]
fn unstable_warning_event_only_mentions_enabled_under_development_features() {
    let mut configured_features = Table::new();
    configured_features.insert(
        "apply_patch_streaming_events".to_string(),
        TomlValue::Boolean(true),
    );
    configured_features.insert("fast_mode".to_string(), TomlValue::Boolean(true));
    configured_features.insert("unknown".to_string(), TomlValue::Boolean(true));

    let mut features = Features::with_defaults();
    features.enable(Feature::ApplyPatchStreamingEvents);

    let warning = unstable_features_warning_event(
        Some(&configured_features),
        /*suppress_unstable_features_warning*/ false,
        &features,
        "/tmp/config.toml",
    )
    .expect("warning event");

    let EventMsg::Warning(WarningEvent { message }) = warning.msg else {
        panic!("expected warning event");
    };
    assert!(message.contains("apply_patch_streaming_events"));
    assert!(!message.contains("fast_mode"));
    assert!(message.contains("/tmp/config.toml"));
}

#[test]
fn unstable_warning_event_ignores_enabled_structured_stable_feature() {
    let configured_features: Table = toml::from_str(
        r#"
multi_agent_v2 = { enabled = true, tool_namespace = "agents" }
code_mode = true
"#,
    )
    .expect("features table should deserialize");

    let mut features = Features::with_defaults();
    features.enable(Feature::MultiAgentV2);
    features.enable(Feature::CodeMode);

    let warning = unstable_features_warning_event(
        Some(&configured_features),
        /*suppress_unstable_features_warning*/ false,
        &features,
        "/tmp/config.toml",
    )
    .expect("warning event");

    let EventMsg::Warning(WarningEvent { message }) = warning.msg else {
        panic!("expected warning event");
    };
    assert_eq!(
        "Under-development features enabled: code_mode. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in /tmp/config.toml.".to_string(),
        message
    );
}

#[test]
fn model_change_suppression_config_preserves_omission_and_explicit_values() {
    for (input, expected) in [
        ("", false),
        ("suppress_model_change_announcements = false", false),
        ("suppress_model_change_announcements = true", true),
    ] {
        let config: FeaturesToml = toml::from_str(input).unwrap();
        let mut features = Features::with_defaults();
        features.apply_toml(&config);
        assert_eq!(
            features.enabled(Feature::SuppressModelChangeAnnouncements),
            expected,
            "{input}"
        );
    }
}
