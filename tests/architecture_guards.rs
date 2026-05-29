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
        "ThreadMeta",
        "ResponseItem",
        "TurnContext",
        "Compacted",
        "EventMsg",
    ] {
        assert!(source.contains(expected), "missing {expected}");
    }
}

#[test]
fn runtime_overlay_is_explicit_live_only_state() {
    let source =
        std::fs::read_to_string("src/runtime/thread_session/overlay.rs").expect("read overlay");

    assert!(source.contains("struct RuntimeOverlay"));
    assert!(source.contains("open_exec_sessions"));
    assert!(source.contains("pending_approvals"));
}

#[test]
fn rollout_reconstruction_does_not_restore_runtime_overlay_fields() {
    let source = std::fs::read_to_string("src/state/rollout.rs").expect("read rollout schema");

    assert!(source.contains("open_exec_sessions: vec![]"));
    assert!(source.contains("pending_approvals: vec![]"));
}

#[test]
fn run_command_tool_does_not_bypass_rollout_with_transcript_writes() {
    let source =
        std::fs::read_to_string("src/tools/run_command.rs").expect("read run command tool");

    assert!(!source.contains("transcript::append_json_line"));
}

#[test]
fn turn_loop_does_not_mutate_snapshot_live_only_fields() {
    let source =
        std::fs::read_to_string("src/runtime/thread_session/turn.rs").expect("read turn loop");

    assert!(!source.contains("snapshot.open_exec_sessions"));
    assert!(!source.contains("snapshot.pending_approvals"));
}
