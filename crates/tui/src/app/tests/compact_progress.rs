use super::*;
use crate::app::owned_transcript::tests::user_cell;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::WebSearchItem;
use codex_protocol::models::MessagePhase;

#[tokio::test]
async fn compact_progress_replaces_visible_rows_through_the_final_answer() -> Result<()> {
    let (mut app, mut events, _ops) = make_test_app_with_channels().await;
    let mut server = start_config_write_test_app_server(&app).await?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    tui.set_owned_screen(true)?;
    let size = Size::new(64, 24);
    tui.terminal.resize(size)?;
    app.transcript_cells.push(user_cell("Update the guide."));

    let thread = ThreadId::new();
    let turn = "guide-turn";
    let start = |item| {
        ServerNotification::ItemStarted(ItemStartedNotification::new(
            thread.to_string(),
            turn.into(),
            item,
            0,
        ))
    };
    let finish = |item| {
        ServerNotification::ItemCompleted(ItemCompletedNotification::new(
            thread.to_string(),
            turn.into(),
            item,
            Some(0),
            1,
        ))
    };
    let message = |id: &str, text: &str, phase| ThreadItemPayload::AgentMessage {
        id: id.into(),
        text: text.into(),
        phase: Some(phase),
        moderation: None,
        memory_citation: None,
        delivery: None,
        questions: None,
    };
    let edit = |status| ThreadItemPayload::FileChange {
        id: "edit".into(),
        changes: vec![FileUpdateChange {
            path: "guide.md".into(),
            kind: PatchChangeKind::Update { move_path: None },
            diff: "@@ -1 +1,2 @@\n-Outdated example\n+Current example\n+Another example".into(),
        }],
        status,
    };
    let search = ThreadItemPayload::WebSearch(WebSearchItem {
        id: "search".into(),
        query: "guide examples".into(),
        action: None,
        results: None,
    });
    let first = message(
        "first",
        "Checking the guide.\n\nI’ll compare the examples.",
        MessagePhase::Commentary,
    );
    let mut frames = Vec::new();
    for (stage, notifications) in [
        (
            "Two calls running",
            vec![
                turn_started_notification(thread, turn),
                start(first.clone()),
                finish(first),
                start(search.clone()),
                start(edit(PatchApplyStatus::InProgress)),
            ],
        ),
        (
            "Shorter commentary streaming",
            vec![
                start(message("second", "", MessagePhase::Commentary)),
                agent_message_delta_notification(thread, turn, "second", "Updating examples."),
            ],
        ),
        (
            "Edit finished",
            vec![
                finish(message(
                    "second",
                    "Updating examples.",
                    MessagePhase::Commentary,
                )),
                finish(edit(PatchApplyStatus::Completed)),
            ],
        ),
        ("Search finished", vec![finish(search)]),
        (
            "Final answer",
            vec![
                start(message("answer", "", MessagePhase::FinalAnswer)),
                agent_message_delta_notification(thread, turn, "answer", "The guide is updated."),
                finish(message(
                    "answer",
                    "The guide is updated.",
                    MessagePhase::FinalAnswer,
                )),
            ],
        ),
    ] {
        for event in notifications {
            app.chat_widget.handle_server_notification(event, None);
            drain_app_events(&mut app, &mut tui, &mut server, &mut events).await?;
        }
        app.render_owned_transcript(&mut tui, size)?;
        // Read the entire transcript, including rows vacated by the previous frame.
        let screen = buffer_text(tui.terminal.previous_buffer());
        let transcript = screen
            .lines()
            .take(usize::from(app.transcript_view.remaining_area().bottom()))
            .collect::<Vec<_>>()
            .join("\n");
        frames.push(format!("{stage}\n{}", transcript.trim_end()));
    }
    insta::assert_snapshot!("compact_progress_lifecycle", frames.join("\n\n"));
    server.shutdown().await?;
    Ok(())
}
