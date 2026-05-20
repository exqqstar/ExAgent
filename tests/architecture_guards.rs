#[test]
fn agent_does_not_mutate_session_snapshot_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in [
        "snapshot.conversation.push",
        "snapshot.open_exec_sessions",
        "snapshot.pending_approvals",
        "LiveEventSink",
    ] {
        assert!(
            !agent.contains(forbidden),
            "src/runtime/agent.rs should not contain {forbidden}"
        );
    }
}

#[test]
fn agent_does_not_keep_mutable_working_conversation() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains("let mut messages ="),
        "Agent should not keep a mutable working conversation"
    );
}

#[test]
fn agent_does_not_execute_tools_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains(".execute(call"),
        "Agent should not execute tools directly"
    );
    assert!(
        !agent.contains("ToolContext"),
        "Agent should not build ToolContext directly"
    );
}

#[test]
fn agent_does_not_parse_tool_meta() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in [
        "approval_status",
        "approval_id",
        "exec_session_id",
        "lifecycle",
    ] {
        assert!(
            !agent.contains(forbidden),
            "Agent should not parse tool meta key {forbidden}"
        );
    }
}

#[test]
fn turn_loop_does_not_sample_from_session_snapshot_conversation() {
    let source =
        std::fs::read_to_string("src/runtime/thread_session/turn.rs").expect("read turn loop");

    assert!(!source.contains("snapshot.conversation.clone()"));
    assert!(!source.contains("ContextManager::for_prompt(snapshot)"));
    assert!(source.contains("context_manager.for_prompt()"));
}

#[test]
fn context_manager_is_stateful_history_owner() {
    let source = std::fs::read_to_string("src/runtime/context.rs").expect("read context manager");

    assert!(source.contains("items: Vec<ConversationMessage>"));
    assert!(source.contains("reference_turn_context: Option<TurnContextItem>"));
}

#[test]
fn rollout_schema_has_codex_style_top_level_items() {
    let source = std::fs::read_to_string("src/state/rollout.rs").expect("read rollout schema");

    for expected in [
        "SessionMeta",
        "ResponseItem",
        "TurnContext",
        "Compacted",
        "EventMsg",
    ] {
        assert!(source.contains(expected), "missing {expected}");
    }
}
