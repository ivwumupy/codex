//! Model-switch compaction uses the historical model across replay.

use super::*;
use codex_core::CodexThread;
use codex_core::ForkSnapshot;
use codex_core::StartThreadOptions;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[derive(Clone, Copy, Debug)]
enum History {
    Live,
    Resume,
    Fork,
    Rollback,
    IncompleteRollback,
}

#[derive(Clone, Copy, Debug)]
enum Compaction {
    RemoteHash,
    RemoteDownshift,
    LocalHash,
    LocalDownshift,
    Fallback,
}

async fn submit_model(thread: &CodexThread, model: &str) -> Result<()> {
    thread
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: format!("turn with {model}"),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                model: Some(model.to_owned()),
                ..Default::default()
            }),
        )
        .await?;
    let event = wait_for_event(thread, |event| {
        matches!(event, EventMsg::TurnComplete(_) | EventMsg::Error(_))
    })
    .await;
    if let EventMsg::Error(error) = event {
        anyhow::bail!("turn failed: {error:?}");
    }
    Ok(())
}

#[test_case(Compaction::RemoteHash, History::Live; "remote hash")]
#[test_case(Compaction::RemoteDownshift, History::Live; "remote downshift")]
#[test_case(Compaction::LocalHash, History::Live; "local hash")]
#[test_case(Compaction::LocalDownshift, History::Live; "local downshift")]
#[test_case(Compaction::RemoteHash, History::Resume; "resume")]
#[test_case(Compaction::RemoteHash, History::Fork; "fork")]
#[test_case(Compaction::RemoteHash, History::Rollback; "rollback")]
#[test_case(Compaction::RemoteHash, History::IncompleteRollback; "incomplete rollback")]
#[test_case(Compaction::Fallback, History::Live; "fallback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_switch_compacts_with_historical_model(
    compaction: Compaction,
    history: History,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let previous_model = "gpt-5.5";
    let next_model = "gpt-5.6-cyber";
    let discarded_model = "discarded-model";
    let downshift = matches!(
        compaction,
        Compaction::RemoteDownshift | Compaction::LocalDownshift
    );
    let local = matches!(
        compaction,
        Compaction::LocalHash | Compaction::LocalDownshift
    );
    let mut old = model_info_with_context_window(previous_model, /*context_window*/ 273_000);
    let mut new = model_info_with_context_window(next_model, /*context_window*/ 125_000);
    if !downshift {
        old.comp_hash = Some("blue-hash".to_owned());
        new.comp_hash = Some("red-hash".to_owned());
    }
    let mut discarded = old.clone();
    discarded.slug = discarded_model.to_owned();
    let models = ModelsResponse {
        models: vec![old, new, discarded],
    };
    let provider = if local {
        local_compaction_provider(&server)
    } else {
        openai_model_provider(&server)
    };
    let mut builder = test_codex()
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_model(previous_model)
        .with_config(move |config| {
            config.model_catalog = Some(models.clone());
            config.model_provider = provider;
            set_test_compact_prompt(config);
        });
    let initial = builder.build_with_auto_env(&server).await?;
    mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("first-message", "surviving history"),
            ev_completed_with_tokens("first", if downshift { 120_000 } else { 100 }),
        ]),
    )
    .await;
    submit_model(&initial.codex, previous_model).await?;

    if matches!(
        history,
        History::Fork | History::Rollback | History::IncompleteRollback
    ) {
        let discarded = mount_sse_once(
            &server,
            sse(vec![
                ev_assistant_message("discarded-message", "discarded history"),
                ev_completed("discarded"),
            ]),
        )
        .await;
        submit_model(&initial.codex, discarded_model).await?;
        assert_eq!(
            discarded.single_request().body_json()["model"],
            discarded_model
        );
    }

    let resumed;
    let thread = match history {
        History::Live => Arc::clone(&initial.codex),
        History::Fork => {
            initial.codex.flush_rollout().await?;
            let mut config = initial.config.clone();
            config.model = Some(next_model.to_owned());
            initial
                .thread_manager
                .fork_legacy_thread(
                    ForkSnapshot::TruncateBeforeNthUserMessage(1),
                    StartThreadOptions::new(config),
                    initial.codex.rollout_path().expect("rollout"),
                )
                .await?
                .thread
        }
        History::Resume | History::Rollback | History::IncompleteRollback => {
            initial.codex.shutdown_and_wait().await?;
            let rollout_path = initial.codex.rollout_path().expect("rollout");
            if matches!(history, History::Rollback | History::IncompleteRollback) {
                // Live rollback is unsupported; replay its persisted marker, as older rollouts do.
                let text = fs::read_to_string(&rollout_path)?;
                let mut lines = text
                    .lines()
                    .map(serde_json::from_str::<Value>)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if matches!(history, History::IncompleteRollback) {
                    let completion = lines
                        .iter()
                        .rposition(|line| {
                            line["type"] == "event_msg"
                                && line["payload"]["type"] == "task_complete"
                        })
                        .expect("last completion");
                    lines.truncate(completion);
                }
                lines.push(
                    json!({"timestamp": "2026-01-01T00:00:00Z", "type": "event_msg", "payload": {
                        "type": "thread_rolled_back", "num_turns": 1
                    }}),
                );
                fs::write(
                    &rollout_path,
                    lines
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n",
                )?;
            }
            let resume_config = initial.config.clone();
            builder = builder.with_config(move |config| {
                config.model_catalog = resume_config.model_catalog;
                config.model_provider = resume_config.model_provider;
                set_test_compact_prompt(config);
            });
            resumed = builder
                .resume(&server, Arc::clone(&initial.home), rollout_path)
                .await?;
            Arc::clone(&resumed.codex)
        }
    };
    let summary = if local {
        ev_assistant_message("summary", "summary of surviving history")
    } else {
        json!({"type": "response.output_item.done", "item": {
            "type": "compaction", "encrypted_content": "SWITCH_SUMMARY"
        }})
    };
    let mut replies = Vec::new();
    if matches!(compaction, Compaction::Fallback) {
        replies.push(invalid_request_response("previous model rejected"));
    }
    replies.push(sse_response(sse(vec![
        summary,
        ev_completed_with_tokens("compact", /*total_tokens*/ 10),
    ])));
    replies.push(sse_response(sse(vec![
        ev_assistant_message("next-message", "next answer"),
        ev_completed("next"),
    ])));
    let requests = mount_response_sequence(&server, replies).await;
    submit_model(&thread, next_model).await?;
    thread.shutdown_and_wait().await?;
    let requests = requests.requests();
    let actual_models = requests
        .iter()
        .map(|request| request.body_json()["model"].clone())
        .collect::<Vec<_>>();
    let mut expected_models = vec![json!(previous_model), json!(next_model)];
    if matches!(compaction, Compaction::Fallback) {
        expected_models.push(json!(next_model));
    }
    assert_eq!(actual_models, expected_models);
    if !local {
        assert_eq!(requests[0].inputs_of_type("compaction_trigger").len(), 1);
    }

    // Even when the previous model summarizes, the replacement belongs to the current model.
    let rollout = fs::read_to_string(thread.rollout_path().expect("rollout"))?
        .lines()
        .map(codex_rollout::parse_rollout_line)
        .collect::<Result<Vec<_>, _>>()?;
    let checkpoint = rollout
        .iter()
        .rposition(|line| matches!(&line.item, RolloutItem::Compacted(_)))
        .expect("model-switch compaction");
    let replacement = &rollout[checkpoint + 1..];
    let world_state = replacement
        .iter()
        .find_map(|line| match &line.item {
            RolloutItem::WorldState(state) => Some(state),
            _ => None,
        })
        .expect("replacement world state");
    let turn_context = replacement
        .iter()
        .find_map(|line| match &line.item {
            RolloutItem::TurnContext(context) => Some(context),
            _ => None,
        })
        .expect("replacement turn context");
    assert_eq!(
        (
            world_state.full,
            world_state.state.get("model"),
            turn_context.model.as_str(),
        ),
        (true, Some(&json!(next_model)), next_model),
    );
    Ok(())
}
