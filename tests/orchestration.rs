use std::path::{Path, PathBuf};

use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::MockLlm;
use exagent::orchestration::{
    collect_session, inspect_children, ChildLifecycleStatus, CollectedOutputKind,
};
use exagent::registry::ToolRegistry;
use exagent::session::{
    AgentRole, ApprovalId, ApprovalStatus, ExecSessionId, ExecSessionRef, ExecSessionStatus,
    PendingApproval, SessionSnapshot,
};
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, SessionId, ToolResult, ToolStatus, TurnId,
};
use tempfile::tempdir;

#[test]
fn lineage_fields_round_trip() {
    let parent = SessionSnapshot {
        session_id: SessionId::new("session_parent"),
        parent_session_id: None,
        root_session_id: SessionId::new("session_parent"),
        spawned_by_turn_id: None,
        agent_role: AgentRole::Primary,
        workspace_root: PathBuf::from("/tmp/exagent"),
        cwd: PathBuf::from("/tmp/exagent"),
        conversation: vec![ConversationMessage::user("plan phase3")],
        open_exec_sessions: vec![],
        latest_compaction: None,
        pending_approvals: vec![],
    };

    let child = parent.fork_child(
        AgentRole::Spec,
        "draft goals",
        Some(TurnId::new("turn_parent_3")),
    );

    assert_ne!(child.session_id, parent.session_id);
    assert_eq!(child.parent_session_id, Some(parent.session_id.clone()));
    assert_eq!(child.root_session_id, parent.root_session_id);
    assert_eq!(child.spawned_by_turn_id, Some(TurnId::new("turn_parent_3")));
    assert_eq!(child.agent_role, AgentRole::Spec);

    let value = serde_json::to_value(&child).unwrap();
    assert_eq!(value["parent_session_id"], "session_parent");
    assert_eq!(value["root_session_id"], "session_parent");
    assert_eq!(value["spawned_by_turn_id"], "turn_parent_3");
    assert_eq!(value["agent_role"], "spec");

    let decoded: SessionSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, child);
}

#[test]
fn session_spawn_event_round_trips() {
    let event = RuntimeEvent {
        event_id: EventId::new("evt_2"),
        session_id: SessionId::new("session_parent"),
        turn_id: Some(TurnId::new("turn_7")),
        kind: RuntimeEventKind::SessionSpawned {
            child_session_id: SessionId::new("session_child"),
            parent_session_id: SessionId::new("session_parent"),
            agent_role: AgentRole::Judge,
            spawned_by_turn_id: Some(TurnId::new("turn_7")),
        },
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["kind"]["type"], "session_spawned");
    assert_eq!(value["kind"]["child_session_id"], "session_child");
    assert_eq!(value["kind"]["parent_session_id"], "session_parent");
    assert_eq!(value["kind"]["agent_role"], "judge");
    assert_eq!(value["kind"]["spawned_by_turn_id"], "turn_7");

    let decoded: RuntimeEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn spawn_event_replays_for_parent_and_child_sessions() {
    let dir = tempdir().unwrap();
    let parent_session_id = SessionId::new("session_parent");
    let child_session_id = SessionId::new("session_child");

    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent_session_id,
        &child_session_id,
        AgentRole::Test,
        Some(&TurnId::new("turn_2")),
    )
    .unwrap();

    let parent_replay =
        exagent::transcript::replay_session(dir.path(), &parent_session_id).unwrap();
    let child_replay = exagent::transcript::replay_session(dir.path(), &child_session_id).unwrap();

    assert_eq!(parent_replay.len(), 1);
    assert_eq!(child_replay.len(), 1);
    assert!(matches!(
        &parent_replay[0].kind,
        RuntimeEventKind::SessionSpawned {
            child_session_id,
            parent_session_id,
            agent_role: AgentRole::Test,
            spawned_by_turn_id: Some(turn_id),
        } if child_session_id == &SessionId::new("session_child")
            && parent_session_id == &SessionId::new("session_parent")
            && turn_id == &TurnId::new("turn_2")
    ));
    assert!(matches!(
        &child_replay[0].kind,
        RuntimeEventKind::SessionSpawned {
            child_session_id,
            parent_session_id,
            agent_role: AgentRole::Test,
            spawned_by_turn_id: Some(turn_id),
        } if child_session_id == &SessionId::new("session_child")
            && parent_session_id == &SessionId::new("session_parent")
            && turn_id == &TurnId::new("turn_2")
    ));
}

#[tokio::test]
async fn agent_can_fork_child_session() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("child ready".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let parent = agent.run_with_meta("lead phase3").await.unwrap();
    let child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Spec,
            "draft goals",
            Some(&TurnId::new("turn_1")),
        )
        .await
        .unwrap();

    assert_ne!(child.session_id, parent.session_id);

    let parent_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&parent.snapshot_path).unwrap();
    let child_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&child.snapshot_path).unwrap();

    assert_eq!(
        child_snapshot.parent_session_id,
        Some(parent.session_id.clone())
    );
    assert_eq!(
        child_snapshot.root_session_id,
        parent_snapshot.root_session_id
    );
    assert_eq!(
        child_snapshot.workspace_root,
        parent_snapshot.workspace_root
    );
    assert_eq!(child_snapshot.cwd, parent_snapshot.cwd);
    assert_eq!(
        child_snapshot.spawned_by_turn_id,
        Some(TurnId::new("turn_1"))
    );
    assert_eq!(child_snapshot.agent_role, AgentRole::Spec);
    assert_eq!(child_snapshot.conversation[0].content, "draft goals");
    assert_eq!(parent_snapshot.conversation.len(), 2);

    let parent_dir = exagent::transcript::session_paths(dir.path(), &parent.session_id).session_dir;
    let child_dir = exagent::transcript::session_paths(dir.path(), &child.session_id).session_dir;
    assert_ne!(parent_dir, child_dir);

    let parent_events =
        exagent::transcript::read_session_events(dir.path(), &parent.session_id).unwrap();
    let child_events =
        exagent::transcript::read_session_events(dir.path(), &child.session_id).unwrap();

    assert!(parent_events.iter().any(|event| {
        matches!(
            &event.kind,
            RuntimeEventKind::SessionSpawned {
                child_session_id,
                parent_session_id,
                agent_role: AgentRole::Spec,
                spawned_by_turn_id: Some(turn_id),
            } if child_session_id == &child.session_id
                && parent_session_id == &parent.session_id
                && turn_id == &TurnId::new("turn_1")
        )
    }));
    assert!(matches!(
        &child_events[0].kind,
        RuntimeEventKind::SessionSpawned {
            child_session_id,
            parent_session_id,
            agent_role: AgentRole::Spec,
            spawned_by_turn_id: Some(turn_id),
        } if child_session_id == &child.session_id
            && parent_session_id == &parent.session_id
            && turn_id == &TurnId::new("turn_1")
    ));
}

#[tokio::test]
async fn sibling_child_sessions_stay_isolated_and_parent_replay_keeps_spawn_order() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("spec ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("test ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("spec resumed".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let parent = agent.run_with_meta("lead phase3").await.unwrap();
    let spec_child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Spec,
            "draft goals",
            Some(&TurnId::new("turn_1")),
        )
        .await
        .unwrap();
    let test_child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Test,
            "draft regressions",
            Some(&TurnId::new("turn_2")),
        )
        .await
        .unwrap();
    let resumed_spec = agent
        .resume(&spec_child.session_id, "continue spec")
        .await
        .unwrap();

    assert_ne!(spec_child.session_id, test_child.session_id);
    assert_eq!(resumed_spec.session_id, spec_child.session_id);

    let spec_dir =
        exagent::transcript::session_paths(dir.path(), &spec_child.session_id).session_dir;
    let test_dir =
        exagent::transcript::session_paths(dir.path(), &test_child.session_id).session_dir;
    assert_ne!(spec_dir, test_dir);

    let parent_events =
        exagent::transcript::replay_session(dir.path(), &parent.session_id).unwrap();
    let spawn_events: Vec<_> = parent_events
        .iter()
        .filter_map(|event| match &event.kind {
            RuntimeEventKind::SessionSpawned {
                child_session_id,
                parent_session_id,
                agent_role,
                spawned_by_turn_id,
            } => Some((
                child_session_id.clone(),
                parent_session_id.clone(),
                agent_role.clone(),
                spawned_by_turn_id.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(spawn_events.len(), 2);
    assert_eq!(
        spawn_events[0],
        (
            spec_child.session_id.clone(),
            parent.session_id.clone(),
            AgentRole::Spec,
            Some(TurnId::new("turn_1"))
        )
    );
    assert_eq!(
        spawn_events[1],
        (
            test_child.session_id.clone(),
            parent.session_id.clone(),
            AgentRole::Test,
            Some(TurnId::new("turn_2"))
        )
    );

    let spec_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&spec_child.snapshot_path).unwrap();
    let test_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&test_child.snapshot_path).unwrap();

    assert_eq!(spec_snapshot.conversation.len(), 4);
    assert_eq!(spec_snapshot.conversation[2].content, "continue spec");
    assert_eq!(spec_snapshot.conversation[3].content, "spec resumed");
    assert_eq!(test_snapshot.conversation.len(), 2);
    assert_eq!(test_snapshot.conversation[0].content, "draft regressions");
    assert_eq!(test_snapshot.conversation[1].content, "test ready");
}

#[tokio::test]
async fn forked_child_uses_parent_cwd_for_tool_execution() {
    let dir = tempdir().unwrap();
    let child_cwd = dir.path().join("spec");
    std::fs::create_dir_all(&child_cwd).unwrap();

    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: None,
            tool_calls: vec![exagent::types::ToolCall {
                id: "call_1".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "pwd" }),
            }],
        },
        AssistantTurn {
            text: Some("child done".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let mut registry = ToolRegistry::new();
    registry.register(exagent::tools::run_command::RunCommandTool);

    let agent = Agent::new(config, Box::new(llm), registry);
    let parent = agent.run_with_meta("lead phase3").await.unwrap();

    let mut parent_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&parent.snapshot_path).unwrap();
    parent_snapshot.cwd = child_cwd.clone();
    exagent::transcript::write_json(&parent.snapshot_path, &parent_snapshot).unwrap();

    let child = agent
        .fork_session(&parent.session_id, AgentRole::Spec, "inspect cwd", None)
        .await
        .unwrap();

    let child_snapshot: SessionSnapshot =
        exagent::transcript::read_json(&child.snapshot_path).unwrap();
    assert_eq!(child_snapshot.cwd, child_cwd);

    let child_events =
        exagent::transcript::read_session_events(dir.path(), &child.session_id).unwrap();
    let tool_result_meta = child_events
        .iter()
        .find_map(|event| match &event.kind {
            RuntimeEventKind::ToolResult { result } => result.meta.clone(),
            _ => None,
        })
        .expect("child tool result");

    assert_eq!(
        tool_result_meta["cwd"],
        child_cwd.to_string_lossy().to_string()
    );
    let expected_cwd = child_cwd.to_string_lossy().to_string();
    assert!(
        tool_result_meta["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains(expected_cwd.as_str()),
        "expected pwd output to contain child cwd"
    );
}

#[test]
fn inspect_lists_direct_children_only() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path().to_path_buf();

    let parent = root_snapshot("session_parent", &workspace_root, "lead phase3");
    let mut spec_child = child_snapshot(
        "session_spec",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Spec,
        "draft goals",
        Some(TurnId::new("turn_1")),
    );
    spec_child.pending_approvals.push(PendingApproval {
        approval_id: ApprovalId::new("approval_spec"),
        requested_event_id: EventId::new("evt_approval"),
        tool_name: "run_command".into(),
        reason: "needs approval".into(),
        status: ApprovalStatus::Pending,
    });
    let mut test_child = child_snapshot(
        "session_test",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Test,
        "draft regressions",
        Some(TurnId::new("turn_2")),
    );
    test_child.open_exec_sessions.push(ExecSessionRef {
        exec_session_id: ExecSessionId::new("exec_test_1"),
        command: "cargo test".into(),
        cwd: workspace_root.clone(),
        status: ExecSessionStatus::Running,
    });
    let implementation_child = child_snapshot(
        "session_impl",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Implementation,
        "patch code",
        Some(TurnId::new("turn_3")),
    );
    let grandchild = child_snapshot(
        "session_grandchild",
        &spec_child.session_id,
        &parent.root_session_id,
        AgentRole::Judge,
        "review spec",
        Some(TurnId::new("turn_4")),
    );
    let unrelated = root_snapshot("session_unrelated", &workspace_root, "other root");

    for snapshot in [
        &parent,
        &spec_child,
        &test_child,
        &implementation_child,
        &grandchild,
        &unrelated,
    ] {
        write_snapshot(dir.path(), snapshot);
    }

    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent.session_id,
        &spec_child.session_id,
        AgentRole::Spec,
        spec_child.spawned_by_turn_id.as_ref(),
    )
    .unwrap();
    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent.session_id,
        &test_child.session_id,
        AgentRole::Test,
        test_child.spawned_by_turn_id.as_ref(),
    )
    .unwrap();
    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent.session_id,
        &implementation_child.session_id,
        AgentRole::Implementation,
        implementation_child.spawned_by_turn_id.as_ref(),
    )
    .unwrap();
    exagent::transcript::record_session_spawn(
        dir.path(),
        &spec_child.session_id,
        &grandchild.session_id,
        AgentRole::Judge,
        grandchild.spawned_by_turn_id.as_ref(),
    )
    .unwrap();

    let children = inspect_children(dir.path(), &parent.session_id).unwrap();

    assert_eq!(children.len(), 3);
    assert_eq!(
        children
            .iter()
            .map(|child| child.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session_spec", "session_test", "session_impl"]
    );

    let spec_summary = &children[0];
    assert_eq!(spec_summary.parent_session_id, parent.session_id);
    assert_eq!(spec_summary.root_session_id, parent.root_session_id);
    assert_eq!(spec_summary.agent_role, AgentRole::Spec);
    assert_eq!(spec_summary.spawned_by_turn_id, Some(TurnId::new("turn_1")));
    assert_eq!(spec_summary.status, ChildLifecycleStatus::WaitingApproval);
    assert_eq!(
        spec_summary.snapshot_path,
        exagent::transcript::session_paths(dir.path(), &spec_child.session_id).snapshot_path
    );
    assert_eq!(
        spec_summary.events_path,
        exagent::transcript::session_paths(dir.path(), &spec_child.session_id).events_path
    );

    assert_eq!(children[1].agent_role, AgentRole::Test);
    assert_eq!(children[1].status, ChildLifecycleStatus::Running);
    assert_eq!(children[2].agent_role, AgentRole::Implementation);
    assert_eq!(children[2].status, ChildLifecycleStatus::Completed);
}

#[test]
fn collect_returns_latest_useful_output() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path().to_path_buf();
    let parent = root_snapshot("session_parent", &workspace_root, "lead phase3");
    let text_child = child_snapshot(
        "session_text_child",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Spec,
        "draft goals",
        Some(TurnId::new("turn_1")),
    );
    let tool_child = child_snapshot(
        "session_tool_child",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Test,
        "run tests",
        Some(TurnId::new("turn_2")),
    );
    let empty_child = child_snapshot(
        "session_empty_child",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Judge,
        "review diff",
        Some(TurnId::new("turn_3")),
    );

    let mut text_snapshot = text_child.clone();
    text_snapshot
        .conversation
        .push(ConversationMessage::assistant(
            Some("spec summary".into()),
            vec![],
        ));
    write_snapshot(dir.path(), &parent);
    write_snapshot(dir.path(), &text_snapshot);
    write_snapshot(dir.path(), &tool_child);
    write_snapshot(dir.path(), &empty_child);

    write_tool_result_event(
        dir.path(),
        &text_child.session_id,
        ToolResult {
            tool_call_id: "call_text".into(),
            tool_name: "run_command".into(),
            status: ToolStatus::Success,
            content: "stdout:\nraw tool output".into(),
            meta: None,
        },
    );
    write_tool_result_event(
        dir.path(),
        &tool_child.session_id,
        ToolResult {
            tool_call_id: "call_tool".into(),
            tool_name: "run_command".into(),
            status: ToolStatus::Success,
            content: "stdout:\nonly tool output".into(),
            meta: None,
        },
    );

    let text_snapshot_before = std::fs::read(
        exagent::transcript::session_paths(dir.path(), &text_child.session_id).snapshot_path,
    )
    .unwrap();
    let text_events_before = std::fs::read(
        exagent::transcript::session_paths(dir.path(), &text_child.session_id).events_path,
    )
    .unwrap();

    let collected_text = collect_session(dir.path(), &text_child.session_id).unwrap();
    let collected_tool = collect_session(dir.path(), &tool_child.session_id).unwrap();
    let collected_empty = collect_session(dir.path(), &empty_child.session_id).unwrap();

    assert_eq!(
        collected_text.latest_useful_output.as_ref().unwrap().kind,
        CollectedOutputKind::AssistantText
    );
    assert_eq!(
        collected_text
            .latest_useful_output
            .as_ref()
            .unwrap()
            .content,
        "spec summary"
    );
    assert_eq!(
        collected_tool.latest_useful_output.as_ref().unwrap().kind,
        CollectedOutputKind::ToolResult
    );
    assert_eq!(
        collected_tool
            .latest_useful_output
            .as_ref()
            .unwrap()
            .content,
        "stdout:\nonly tool output"
    );
    assert!(collected_empty.latest_useful_output.is_none());

    assert_eq!(
        text_snapshot_before,
        std::fs::read(
            exagent::transcript::session_paths(dir.path(), &text_child.session_id).snapshot_path
        )
        .unwrap()
    );
    assert_eq!(
        text_events_before,
        std::fs::read(
            exagent::transcript::session_paths(dir.path(), &text_child.session_id).events_path
        )
        .unwrap()
    );
}

#[test]
fn agent_can_inspect_and_collect_children() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path().to_path_buf();
    let parent = root_snapshot("session_parent", &workspace_root, "lead phase3");
    let mut child = child_snapshot(
        "session_spec_child",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Spec,
        "draft goals",
        Some(TurnId::new("turn_1")),
    );
    child.conversation.push(ConversationMessage::assistant(
        Some("child summary".into()),
        vec![],
    ));

    write_snapshot(dir.path(), &parent);
    write_snapshot(dir.path(), &child);
    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent.session_id,
        &child.session_id,
        AgentRole::Spec,
        child.spawned_by_turn_id.as_ref(),
    )
    .unwrap();

    let config = AgentConfig {
        workspace_root: workspace_root.clone(),
        cwd: workspace_root,
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Box::new(MockLlm::new(vec![])), ToolRegistry::new());

    let parent_events_before = std::fs::read(
        exagent::transcript::session_paths(dir.path(), &parent.session_id).events_path,
    )
    .unwrap();
    let child_snapshot_before = std::fs::read(
        exagent::transcript::session_paths(dir.path(), &child.session_id).snapshot_path,
    )
    .unwrap();

    let children = agent.inspect_children(&parent.session_id).unwrap();
    let collected = agent.collect_session(&child.session_id).unwrap();

    assert_eq!(children.len(), 1);
    assert_eq!(children[0].session_id, child.session_id);
    assert_eq!(
        collected
            .latest_useful_output
            .as_ref()
            .map(|output| output.content.as_str()),
        Some("child summary")
    );
    assert_eq!(
        parent_events_before,
        std::fs::read(
            exagent::transcript::session_paths(dir.path(), &parent.session_id).events_path
        )
        .unwrap()
    );
    assert_eq!(
        child_snapshot_before,
        std::fs::read(
            exagent::transcript::session_paths(dir.path(), &child.session_id).snapshot_path
        )
        .unwrap()
    );
}

fn root_snapshot(session_id: &str, workspace_root: &Path, prompt: &str) -> SessionSnapshot {
    SessionSnapshot::new_root(
        SessionId::new(session_id),
        workspace_root.to_path_buf(),
        workspace_root.to_path_buf(),
        prompt,
    )
}

fn child_snapshot(
    session_id: &str,
    parent_session_id: &SessionId,
    root_session_id: &SessionId,
    agent_role: AgentRole,
    prompt: &str,
    spawned_by_turn_id: Option<TurnId>,
) -> SessionSnapshot {
    SessionSnapshot {
        session_id: SessionId::new(session_id),
        parent_session_id: Some(parent_session_id.clone()),
        root_session_id: root_session_id.clone(),
        spawned_by_turn_id,
        agent_role,
        workspace_root: PathBuf::new(),
        cwd: PathBuf::new(),
        conversation: vec![ConversationMessage::user(prompt)],
        open_exec_sessions: vec![],
        latest_compaction: None,
        pending_approvals: vec![],
    }
}

fn write_snapshot(workspace_root: &Path, snapshot: &SessionSnapshot) {
    let mut snapshot = snapshot.clone();
    snapshot.workspace_root = workspace_root.to_path_buf();
    snapshot.cwd = workspace_root.to_path_buf();
    let paths = exagent::transcript::session_paths(workspace_root, &snapshot.session_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
}

fn write_tool_result_event(workspace_root: &Path, session_id: &SessionId, result: ToolResult) {
    exagent::transcript::append_runtime_event(
        workspace_root,
        session_id,
        None,
        RuntimeEventKind::ToolResult { result },
    )
    .unwrap();
}
