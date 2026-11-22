use crate::agent::LocalAgentControl;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;

fn control_with_limit(max_threads: usize) -> LocalAgentControl {
    let control = LocalAgentControl::default();
    control
        .runtime
        .agent_execution_limiter
        .initialize(max_threads);
    control
}

#[test]
fn execution_guards_count_active_v2_subagent_turns() {
    let control = control_with_limit(/*max_threads*/ 1);
    // Child role configs cannot replace the root-derived session limit.
    control
        .runtime
        .agent_execution_limiter
        .initialize(/*max_threads*/ 2);
    let source = SessionSource::SubAgent(SubAgentSource::Other("worker".to_string()));

    control
        .ensure_execution_capacity(&source)
        .expect("first active turn should fit");
    let first = control
        .execution_guard(&source)
        .expect("v2 subagent execution should be counted");
    let Err(err) = control.ensure_execution_capacity(&source) else {
        panic!("second active turn should exceed the derived non-root cap");
    };
    let CodexErrorDetails::AgentLimitReached { max_threads } = err.details() else {
        panic!("expected AgentLimitReached");
    };
    assert_eq!(*max_threads, 1);

    drop(first);
    control
        .ensure_execution_capacity(&source)
        .expect("capacity should be released when the running task drops");
}

#[test]
fn execution_guards_ignore_root_turns() {
    let control = control_with_limit(/*max_threads*/ 0);

    assert!(control.execution_guard(&SessionSource::Cli).is_none());
}
