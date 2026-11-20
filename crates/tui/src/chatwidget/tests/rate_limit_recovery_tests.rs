use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn api_rate_limit_error_reports_failure_and_releases_queued_input() {
    let (mut chat, mut events, mut ops) = make_chatwidget_manual(Some("test-model-a")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "failed-turn");
    chat.queue_user_message(UserMessage::from("queued follow-up"));
    chat.on_rate_limit_error(RateLimitErrorKind::UsageLimit, "Usage exhausted".into());
    let history = drain_insert_history(&mut events);
    assert!(
        history
            .iter()
            .any(|lines| lines_to_single_string(lines).contains("Usage exhausted"))
    );
    let Op::UserTurn { model, items, .. } = next_submit_op(&mut ops) else {
        panic!("expected queued follow-up");
    };
    assert_eq!(model, "test-model-a");
    assert!(
        matches!(items.as_slice(), [UserInput::Text { text, .. }] if text == "queued follow-up")
    );
    assert!(chat.queued_user_message_texts().is_empty());
    assert_no_submit_op(&mut ops);
}

#[tokio::test]
async fn api_rate_limit_error_preserves_settings_hold() {
    let (mut chat, _events, mut ops) = make_chatwidget_manual(Some("test-model-a")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_queue_autosend_suppressed(/*suppressed*/ true);
    chat.queue_user_message(UserMessage::from("queued follow-up"));
    chat.on_rate_limit_error(RateLimitErrorKind::UsageLimit, "Usage exhausted".into());
    assert!(chat.input_queue.suppress_queue_autosend);
    assert!(!chat.input_queue.rate_limit_recovery_pending);
    assert_eq!(chat.queued_user_message_texts(), vec!["queued follow-up"]);
    assert_no_submit_op(&mut ops);
}
