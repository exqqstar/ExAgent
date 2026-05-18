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
