use exagent::app_server::protocol::{
    EventsReplayParams, ThreadSpawnChildParams, ThreadStartParams, TurnStartParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use exagent::session::{AgentRole, SessionSnapshot};
use exagent::types::{AssistantTurn, SessionId, TurnId};
use tempfile::tempdir;

#[test]
fn thread_start_applies_workspace_and_cwd_overrides() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );

    let response = service
        .thread_start(ThreadStartParams {
            workspace_root: Some(dir.path().to_string_lossy().to_string()),
            cwd: Some("nested".into()),
        })
        .unwrap();

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(response.snapshot_path.as_ref()).unwrap();
    assert_eq!(snapshot.session_id, response.thread_id);
    assert_eq!(snapshot.parent_session_id, None);
    assert_eq!(snapshot.root_session_id, response.thread_id);
    assert_eq!(
        snapshot.workspace_root,
        std::fs::canonicalize(dir.path()).unwrap()
    );
    assert_eq!(snapshot.cwd, std::fs::canonicalize(nested).unwrap());
    assert!(snapshot.conversation.is_empty());
}

#[test]
fn thread_start_rejects_cwd_outside_workspace() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );

    let err = service
        .thread_start(ThreadStartParams {
            workspace_root: Some(dir.path().to_string_lossy().to_string()),
            cwd: Some(outside.path().to_string_lossy().to_string()),
        })
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));
}

#[tokio::test]
async fn turn_start_runs_existing_thread_non_streaming_and_events_replay_returns_events() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("thread turn complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "continue work".into(),
            workspace_root: None,
        })
        .await
        .unwrap();

    assert_eq!(turn.thread_id, thread.thread_id);
    assert_eq!(turn.turn_id, TurnId::new("turn_1"));
    assert_eq!(turn.output.text.as_deref(), Some("thread turn complete"));

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(replay.thread_id, thread.thread_id);
    assert_eq!(replay.events.len(), 1);
    assert!(matches!(
        &replay.events[0].kind,
        RuntimeEventKind::AssistantTurn { turn } if turn.text.as_deref() == Some("thread turn complete")
    ));
}

#[tokio::test]
async fn thread_spawn_child_uses_parent_context_and_records_spawn_events() {
    let dir = tempdir().unwrap();
    let parent_cwd = dir.path().join("parent-cwd");
    let ignored_cwd = dir.path().join("ignored-cwd");
    std::fs::create_dir_all(&parent_cwd).unwrap();
    std::fs::create_dir_all(&ignored_cwd).unwrap();

    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("child complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let parent = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("parent-cwd".into()),
        })
        .unwrap();
    let child = service
        .thread_spawn_child(ThreadSpawnChildParams {
            parent_thread_id: parent.thread_id.clone(),
            agent_role: AgentRole::Spec,
            prompt: "draft spec".into(),
            workspace_root: None,
            cwd: Some("ignored-cwd".into()),
            spawned_by_turn_id: Some(TurnId::new("turn_parent_1")),
        })
        .await
        .unwrap();

    assert_eq!(child.parent_thread_id, parent.thread_id);
    assert_eq!(child.agent_role, AgentRole::Spec);
    assert_eq!(child.output.text.as_deref(), Some("child complete"));

    let child_snapshot: SessionSnapshot =
        exagent::transcript::read_json(child.output.snapshot_path.as_ref()).unwrap();
    assert_eq!(
        child_snapshot.parent_session_id,
        Some(parent.thread_id.clone())
    );
    assert_eq!(child_snapshot.root_session_id, parent.thread_id);
    assert_eq!(
        child_snapshot.cwd,
        std::fs::canonicalize(parent_cwd).unwrap()
    );
    assert_ne!(child_snapshot.cwd, ignored_cwd);

    let parent_replay = service
        .events_replay(EventsReplayParams {
            thread_id: child.parent_thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert!(parent_replay.events.iter().any(|event| {
        matches!(
            &event.kind,
            RuntimeEventKind::SessionSpawned {
                child_session_id,
                parent_session_id,
                agent_role: AgentRole::Spec,
                spawned_by_turn_id: Some(turn_id),
            } if child_session_id == &child.child_thread_id
                && parent_session_id == &child.parent_thread_id
                && *turn_id == TurnId::new("turn_parent_1")
        )
    }));
}

#[test]
fn events_replay_returns_empty_list_for_new_thread() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: SessionId::new(thread.thread_id.as_str()),
            workspace_root: None,
        })
        .unwrap();

    assert_eq!(replay.events, vec![]);
}
