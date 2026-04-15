use std::path::PathBuf;

use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use exagent::session::{AgentRole, SessionSnapshot};
use exagent::types::{AssistantTurn, ConversationMessage, EventId, SessionId, TurnId};
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
