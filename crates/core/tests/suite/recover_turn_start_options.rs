use anyhow::Result;
use codex_core::RecoverTurnRequest;
use codex_core::StartIfIdleSubmission;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_core::TurnInputSubmission;
use codex_core::TurnStartOptions;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutRecorder;
use core_test_support::apps_test_server::AppsTestServer;
use core_test_support::apps_test_server::SEARCH_CALENDAR_CREATE_TOOL;
use core_test_support::apps_test_server::SEARCH_CALENDAR_NAMESPACE;
use core_test_support::apps_test_server::search_capable_apps_builder;
use core_test_support::responses;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_case::test_case;
use tokio::sync::oneshot;

#[test_case(false; "user_input")]
#[test_case(true; "standalone_tool_output")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recover_turn_restores_start_options_without_making_them_sticky(
    standalone_tool_output: bool,
) -> Result<()> {
    // Keep sampling pending until interruption so restart must reload the unfinished turn.
    let (release_response, response_gate) = oneshot::channel();
    let (initial_server, _completions) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse_completed("seed"),
        }],
        vec![StreamingSseChunk {
            gate: Some(response_gate),
            body: responses::sse(vec![responses::ev_completed("resp-initial")]),
        }],
    ])
    .await;
    let server = responses::start_mock_server().await;
    let apps = AppsTestServer::mount_searchable(&server).await?;
    let builder = || {
        search_capable_apps_builder(apps.chatgpt_base_url.clone()).with_config(|config| {
            config
                .features
                .enable(Feature::CodeModeOnly)
                .expect("enable code mode only");
            config.code_mode.direct_only_tool_namespaces =
                vec![SEARCH_CALENDAR_NAMESPACE.to_owned()];
        })
    };
    let initial = builder()
        .build_with_streaming_server(&initial_server)
        .await?;
    initial.submit_text_turn("seed user turn").await?;
    let request = if standalone_tool_output {
        TurnInputRequest::new(TurnInput::ResponseItem(serde_json::from_value(json!({
            "type": "function_call_output", "name": "result", "namespace": "example",
            "output": "recover this tool output",
        }))?))
    } else {
        TurnInputRequest::user_input(vec![UserInput::Text {
            text: "recover this turn".to_owned(),
            text_elements: Vec::new(),
        }])
    };
    let TurnInputSubmission::Started { turn_id, .. } = initial
        .codex
        .start_or_steer_turn(request.on_start(TurnStartOptions {
            direct_only_tool_namespaces: Some(Vec::new()),
            ..Default::default()
        }))
        .await?
    else {
        panic!("expected a new turn");
    };
    initial_server.wait_for_request_count(/*count*/ 2).await;
    initial.codex.submit(Op::Interrupt).await?;
    wait_for_event(&initial.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    drop(release_response);
    initial_server.shutdown().await;
    initial.codex.flush_rollout().await?;

    let test = builder().restart(&server, &initial).await?;
    let (rollout, _, _) = RolloutRecorder::load_rollout_items(
        &test
            .codex
            .rollout_path()
            .expect("recovered turn rollout path"),
    )
    .await?;
    let persisted_context = rollout
        .iter()
        .rev()
        .find_map(|item| match item {
            RolloutItem::TurnContext(context)
                if context.turn_id.as_deref() == Some(turn_id.as_str()) =>
            {
                Some(context)
            }
            _ => None,
        })
        .expect("persisted context for the interrupted turn");

    assert_eq!(
        persisted_context.direct_only_tool_namespaces,
        Some(Vec::new())
    );

    let response_mock =
        responses::mount_sse_once(&server, responses::sse_completed("resp-1")).await;
    let submission = test
        .codex
        .recover_turn_if_idle(RecoverTurnRequest {
            turn_id: turn_id.clone(),
            thread_settings: Default::default(),
            trace: None,
            prompt_cache_ttl: None,
        })
        .await?;
    assert_eq!(
        submission,
        StartIfIdleSubmission::Started {
            root_turn_id: turn_id.clone(),
            turn_id: turn_id.clone()
        }
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let recovered_request = response_mock.single_request();
    let recovered_input_count = if standalone_tool_output {
        recovered_request
            .inputs_of_type("function_call_output")
            .iter()
            .filter(|item| item["name"] == "result")
            .count()
    } else {
        recovered_request
            .message_input_texts("user")
            .iter()
            .filter(|text| text.as_str() == "recover this turn")
            .count()
    };
    assert_eq!(recovered_input_count, 1);

    let declares_calendar = |body: &serde_json::Value| {
        responses::namespace_child_tool(
            body,
            SEARCH_CALENDAR_NAMESPACE,
            SEARCH_CALENDAR_CREATE_TOOL,
        )
        .is_some()
    };
    assert!(!declares_calendar(
        &response_mock.single_request().body_json()
    ));

    let next_response = responses::mount_sse_once(
        &server,
        responses::sse(vec![responses::ev_completed("resp-next")]),
    )
    .await;
    let TurnInputSubmission::Started {
        turn_id: next_turn_id,
        ..
    } = test
        .codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "start a new turn".to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?
    else {
        panic!("expected a new turn");
    };
    assert_ne!(next_turn_id, turn_id);
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(declares_calendar(
        &next_response.single_request().body_json()
    ));
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
