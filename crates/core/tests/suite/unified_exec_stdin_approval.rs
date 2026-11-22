//! Exercises session grants and stdin review through the agent API.

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Constrained;
use codex_features::Feature;
use codex_protocol::approvals::ExecApprovalKind;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GranularApprovalConfig;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::mount_function_call_agent_response;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_sandbox;
use core_test_support::skip_if_target_windows;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn tool_response(id: &str, tool: &str, args: Value) -> String {
    sse(vec![
        ev_function_call(id, tool, &args.to_string()),
        ev_completed(id),
    ])
}

async fn harness(builder: TestCodexBuilder) -> Result<TestCodexHarness> {
    TestCodexHarness::with_auto_env_builder(builder.with_config(|config| {
        for feature in [
            Feature::ExecPermissionApprovals,
            Feature::RequestPermissionsTool,
        ] {
            config
                .features
                .enable(feature)
                .expect("enable stdin review");
        }
        config
            .permissions
            .set_permission_profile(PermissionProfile::read_only())
            .expect("set read-only permissions");
        config.permissions.approval_policy = Constrained::allow_any(AskForApproval::OnRequest);
    }))
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_reviews_retained_session_grants() -> Result<()> {
    skip_if_target_windows!(Ok(()), "uses a POSIX interactive shell");
    skip_if_no_network!(Ok(()));
    skip_if_sandbox!(Ok(()));
    let harness = harness(test_codex()).await?;
    let test = harness.test();
    harness.create_dir_all("allowed").await?;
    let directory = test
        .fs()
        .canonicalize(&test.workspace_path_uri("allowed")?, /*sandbox*/ None)
        .await?;
    let permissions = json!({"file_system": {"write": [directory.to_abs_path()?]}});
    let grant = mount_sse_once(
        harness.server(),
        tool_response(
            "grant",
            "request_permissions",
            json!({"reason":"allow terminal writes", "permissions":permissions}),
        ),
    )
    .await;
    let opened = mount_function_call_agent_response(
        harness.server(),
        "open",
        &json!({"cmd":"/bin/bash --noprofile --norc", "tty":true, "yield_time_ms":200}).to_string(),
        "exec_command",
    )
    .await;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "open terminal".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let request = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::RequestPermissions(request) => Some(request.clone()),
        _ => None,
    })
    .await;
    test.codex
        .submit(Op::RequestPermissionsResponse {
            id: request.call_id,
            response: RequestPermissionsResponse {
                permissions: request.permissions,
                scope: PermissionGrantScope::Session,
                strict_auto_review: false,
            },
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(grant.requests().len(), 1);
    let output = opened
        .completion
        .single_request()
        .function_call_output("open");
    assert!(
        output
            .to_string()
            .contains("Process running with session ID 1000"),
        "{output}"
    );

    let writes = mount_sse_sequence(harness.server(), vec![
        tool_response("poll", "write_stdin", json!({"session_id":1000, "chars":"", "yield_time_ms":1000})),
        tool_response("denied", "write_stdin", json!({"session_id":1000, "chars":"REJECTED=1\n"})),
        tool_response("allowed", "write_stdin", json!({"session_id":1000, "chars":"test -z \"${REJECTED-}\" && printf allowed > allowed/result; exit\n"})),
        sse(vec![ev_completed("done")]),
    ]).await;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "send input".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    for (id, decision) in [
        ("denied", ReviewDecision::denied("blocked input")),
        ("allowed", ReviewDecision::Approved),
    ] {
        let request = wait_for_event_match(&test.codex, |event| match event {
            EventMsg::ExecApprovalRequest(request) => Some(request.clone()),
            _ => None,
        })
        .await;
        assert_eq!(
            (
                request.kind,
                request.call_id.as_str(),
                request.effective_approval_id().as_str()
            ),
            (ExecApprovalKind::WriteStdin, "open", id)
        );
        assert_eq!(
            request.additional_permissions,
            Some(serde_json::from_value::<AdditionalPermissionProfile>(
                permissions.clone()
            )?)
        );
        test.codex
            .submit(Op::ExecApproval {
                id: id.into(),
                turn_id: Some(request.turn_id),
                decision,
            })
            .await?;
    }
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let output = writes
        .requests()
        .last()
        .unwrap()
        .function_call_output("allowed");
    assert!(
        output.to_string().contains("Process exited with code 0"),
        "{output}"
    );
    assert_eq!(harness.read_file_text("allowed/result").await?, "allowed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_reviews_escalated_terminal_with_denied_reads_before_delivering_input() -> Result<()>
{
    skip_if_target_windows!(Ok(()), "uses a POSIX interactive shell");
    skip_if_no_network!(Ok(()));
    skip_if_sandbox!(Ok(()));
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.features.enable(Feature::WriteStdinApproval).unwrap();
        config
            .features
            .enable(Feature::ExecPermissionApprovals)
            .unwrap();
        config.features.enable(Feature::UnifiedExec).unwrap();
        config.permissions.approval_policy = Constrained::allow_any(AskForApproval::OnRequest);
        let mut file_system = PermissionProfile::read_only().file_system_sandbox_policy();
        file_system.entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::GlobPattern {
                pattern: "**/secret.env".into(),
            },
            FileSystemAccessMode::Deny,
        ));
        config
            .permissions
            .set_permission_profile(PermissionProfile::from_runtime_permissions(
                &file_system,
                NetworkSandboxPolicy::Restricted,
            ))
            .unwrap();
    }))
    .await?;
    let codex = &harness.test().codex;
    harness.write_file("secret.env", "secret").await?;
    let _open = mount_function_call_agent_response(
        harness.server(),
        "open",
        &json!({"cmd":"/bin/bash --noprofile --norc", "tty":true, "yield_time_ms":200, "sandbox_permissions":"require_escalated", "justification":"open an escalated terminal"}).to_string(),
        "exec_command",
    ).await;
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "open terminal".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let launch = wait_for_event_match(codex, |event| match event {
        EventMsg::ExecApprovalRequest(request) => Some(request.clone()),
        _ => None,
    })
    .await;
    assert_eq!(launch.kind, ExecApprovalKind::Command);
    codex
        .submit(Op::ExecApproval {
            id: launch.effective_approval_id(),
            turn_id: Some(launch.turn_id),
            decision: ReviewDecision::Approved,
        })
        .await?;
    wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let _writes = mount_sse_sequence(harness.server(), vec![
        tool_response("denied", "write_stdin", json!({"session_id":1000,"chars":"printf denied > denied-marker\n"})),
        tool_response("allowed", "write_stdin", json!({"session_id":1000,"chars":"if test -e denied-marker || cat secret.env; then exit 12; fi; printf approved > approved-marker; exit\n"})),
        sse(vec![ev_completed("done")]),
    ]).await;
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "send input".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    for (id, decision) in [
        ("denied", ReviewDecision::denied("blocked input")),
        ("allowed", ReviewDecision::Approved),
    ] {
        let request = wait_for_event_match(codex, |event| match event {
            EventMsg::ExecApprovalRequest(request) => Some(request.clone()),
            _ => None,
        })
        .await;
        assert_eq!(
            (
                request.kind,
                request.call_id.as_str(),
                request.effective_approval_id().as_str()
            ),
            (ExecApprovalKind::WriteStdin, "open", id)
        );
        assert!(!harness.path_exists("denied-marker").await?);
        assert!(!harness.path_exists("approved-marker").await?);
        codex
            .submit(Op::ExecApproval {
                id: request.effective_approval_id(),
                turn_id: Some(request.turn_id),
                decision,
            })
            .await?;
    }
    let mut process_end = None;
    let mut turn_completed = false;
    while process_end.is_none() || !turn_completed {
        match wait_for_event(codex, |_| true).await {
            EventMsg::ExecCommandEnd(event) if event.call_id == "open" => process_end = Some(event),
            EventMsg::TurnComplete(_) => turn_completed = true,
            EventMsg::ExecApprovalRequest(request) => {
                anyhow::bail!("unexpected additional stdin approval: {request:?}")
            }
            _ => {}
        }
    }
    let process_end = process_end.expect("retained terminal should finish");
    assert_eq!(process_end.exit_code, 0);
    assert!(!harness.path_exists("denied-marker").await?);
    assert_eq!(harness.read_file_text("approved-marker").await?, "approved");
    Ok(())
}

#[derive(Clone, Copy)]
enum LaterApprovalPolicy {
    Inherit,
    Never,
}

/// A session grant permits reviewed stdin without sandbox escalation.
#[test_case::test_case(false, LaterApprovalPolicy::Never; "session_grant_never_policy")]
#[test_case::test_case(true, LaterApprovalPolicy::Inherit; "unchanged_interleaved_denials")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_reviews_current_grants_with_sandbox_prompts_disabled(
    ordered_denials: bool,
    later_policy: LaterApprovalPolicy,
) -> Result<()> {
    skip_if_target_windows!(Ok(()), "uses a POSIX interactive shell");
    skip_if_no_network!(Ok(()));
    skip_if_sandbox!(Ok(()));
    let harness = harness(test_codex()).await?;
    let test = harness.test();
    harness.create_dir_all("allowed").await?;
    let allowed = test.workspace_path_uri("allowed")?.to_abs_path()?;
    let grants = if ordered_denials {
        harness
            .write_file("allowed/a/private/secret", "secret")
            .await?;
        harness.create_dir_all("allowed/b").await?;
        let write_a =
            FileSystemSandboxEntry::new(allowed.join("a").into(), FileSystemAccessMode::Write);
        let deny_a = FileSystemSandboxEntry::new(
            allowed.join("a/private").into(),
            FileSystemAccessMode::Deny,
        );
        let write_b =
            FileSystemSandboxEntry::new(allowed.join("b").into(), FileSystemAccessMode::Write);
        let retained = FileSystemPermissions {
            entries: vec![write_a, deny_a],
            glob_scan_max_depth: None,
        };
        let extra = FileSystemPermissions {
            entries: vec![write_b],
            glob_scan_max_depth: None,
        };
        vec![
            (json!({"file_system":retained})),
            (json!({"file_system":extra})),
        ]
    } else {
        vec![json!({"network":{"enabled":true}})]
    };
    let mut responses = Vec::new();
    for (id, permissions) in ["grant", "extra_grant"].into_iter().zip(&grants) {
        responses.push(tool_response(
            id,
            "request_permissions",
            json!({"reason":"allow terminal access", "permissions":permissions}),
        ));
    }
    responses.push(tool_response(
        "open",
        "exec_command",
        json!({"cmd":"/bin/bash --noprofile --norc", "tty":true, "yield_time_ms":200}),
    ));
    let grant_probe = if ordered_denials {
        "printf launch > allowed/a/launched && printf launch > allowed/b/launched && ! cat allowed/a/private/secret && "
    } else {
        ""
    };
    responses.push(tool_response("allowed", "write_stdin", json!({"session_id":1000, "chars":format!("{grant_probe}printf 'CURRENT_%s_OK\\n' GRANT\n")})));
    responses.push(sse(vec![ev_completed("done")]));
    let writes = mount_sse_sequence(harness.server(), responses).await;
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "grant permission, open a terminal, and send reviewed input".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                permission_profile: Some(PermissionProfile::read_only()),
                approval_policy: Some(AskForApproval::Granular(GranularApprovalConfig {
                    sandbox_approval: false,
                    rules: false,
                    skill_approval: true,
                    request_permissions: true,
                    mcp_elicitations: false,
                })),
                ..Default::default()
            }),
        )
        .await?;
    let mut grants_remaining = grants.len();
    let mut approvals = 0;
    loop {
        match wait_for_event(&test.codex, |_| true).await {
            EventMsg::RequestPermissions(request) => {
                grants_remaining -= 1;
                test.codex
                    .submit(Op::RequestPermissionsResponse {
                        id: request.call_id,
                        response: RequestPermissionsResponse {
                            permissions: request.permissions,
                            scope: PermissionGrantScope::Session,
                            strict_auto_review: false,
                        },
                    })
                    .await?;
            }
            EventMsg::ExecApprovalRequest(request) => {
                assert_eq!(
                    (request.kind, request.effective_approval_id()),
                    (ExecApprovalKind::WriteStdin, "allowed".to_string())
                );
                approvals += 1;
                test.codex
                    .submit(Op::ExecApproval {
                        id: request.effective_approval_id(),
                        turn_id: Some(request.turn_id),
                        decision: ReviewDecision::Approved,
                    })
                    .await?;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
    assert_eq!(grants_remaining, 0);
    assert_eq!(approvals, 1);
    let output = writes
        .function_call_output_text("allowed")
        .expect("stdin output");
    assert!(output.contains("CURRENT_GRANT_OK"), "{output}");

    let later = mount_function_call_agent_response(
        harness.server(),
        "later",
        &json!({"session_id":1000, "chars":"LATER=1; printf 'LATER_%s_OK\\n' GRANT\n"}).to_string(),
        "write_stdin",
    )
    .await;
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "send input in a later turn".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                approval_policy: match later_policy {
                    LaterApprovalPolicy::Inherit => None,
                    LaterApprovalPolicy::Never => Some(AskForApproval::Never),
                },
                ..Default::default()
            }),
        )
        .await?;
    let later_input_allowed = matches!(later_policy, LaterApprovalPolicy::Inherit);
    let mut later_approvals = 0;
    loop {
        match wait_for_event(&test.codex, |_| true).await {
            EventMsg::ExecApprovalRequest(request) => {
                assert!(later_input_allowed, "unexpected stdin review: {request:?}");
                assert_eq!(
                    (request.kind, request.effective_approval_id()),
                    (ExecApprovalKind::WriteStdin, "later".to_string())
                );
                later_approvals += 1;
                test.codex
                    .submit(Op::ExecApproval {
                        id: request.effective_approval_id(),
                        turn_id: Some(request.turn_id),
                        decision: ReviewDecision::Approved,
                    })
                    .await?;
            }
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
    assert_eq!(later_approvals, usize::from(later_input_allowed));
    let output = later
        .completion
        .function_call_output_text("later")
        .expect("later stdin output");
    if matches!(later_policy, LaterApprovalPolicy::Never) {
        assert!(
            output.contains("AskForApproval is set to Never"),
            "{output}"
        );
    } else {
        assert!(output.contains("LATER_GRANT_OK"), "{output}");
    }
    if !later_input_allowed {
        let probe = mount_function_call_agent_response(
            harness.server(),
            "probe",
            &json!({"session_id":1000, "chars":"test -z \"${LATER-}\" && printf 'REJECTED_%s_OK\\n' INPUT; exit\n"}).to_string(),
            "write_stdin",
        )
        .await;
        test.codex
            .start_or_steer_turn(
                TurnInputRequest::user_input(vec![UserInput::Text {
                    text: "verify rejected input was not delivered".into(),
                    text_elements: Vec::new(),
                }])
                .with_thread_settings(ThreadSettingsOverrides {
                    approval_policy: Some(AskForApproval::OnRequest),
                    ..Default::default()
                }),
            )
            .await?;
        loop {
            match wait_for_event(&test.codex, |_| true).await {
                EventMsg::ExecApprovalRequest(request) => {
                    assert_eq!(
                        (request.kind, request.effective_approval_id()),
                        (ExecApprovalKind::WriteStdin, "probe".to_string())
                    );
                    test.codex
                        .submit(Op::ExecApproval {
                            id: request.effective_approval_id(),
                            turn_id: Some(request.turn_id),
                            decision: ReviewDecision::Approved,
                        })
                        .await?;
                }
                EventMsg::TurnComplete(_) => break,
                _ => {}
            }
        }
        let output = probe
            .completion
            .function_call_output_text("probe")
            .expect("probe output");
        assert!(output.contains("REJECTED_INPUT_OK"), "{output}");
    }
    Ok(())
}
