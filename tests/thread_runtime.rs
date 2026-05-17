use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntime, ThreadRuntimeOptions, ThreadRuntimeStatus,
};
use exagent::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
use exagent::session::SessionSnapshot;
use exagent::types::{AssistantTurn, ConversationMessage, EventId, SessionId, TurnId};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn thread_session_can_be_constructed_as_runtime_state_owner() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_thread_session_construct");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
    let session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("create thread session");

    assert_eq!(session.thread_id(), &thread_id);
}

#[tokio::test]
async fn thread_runtime_starts_idle_and_accepts_shutdown_op() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_runtime_test");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    assert_eq!(runtime.thread_id(), &thread_id);
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Idle);

    runtime.shutdown().await.expect("submit shutdown");
    runtime.wait_until_terminated().await;
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
}

#[tokio::test]
async fn thread_runtime_live_view_uses_loaded_session_state_not_disk_mutations() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_runtime_live_view");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    exagent::transcript::append_json_line(
        &paths.events_path,
        &RuntimeEvent {
            event_id: EventId::new("evt_disk_only"),
            session_id: thread_id.clone(),
            turn_id: Some(TurnId::new("turn_disk_only")),
            kind: RuntimeEventKind::RuntimeError {
                message: "disk mutation after runtime load".into(),
            },
        },
    )
    .expect("append disk-only event");

    let live_view = runtime.live_view().expect("read live view");

    assert_eq!(live_view.thread_id, thread_id);
    assert!(live_view.events.is_empty());
    assert!(live_view.snapshot.conversation.is_empty());
}

#[tokio::test]
async fn thread_runtime_runs_user_input_through_agent_and_records_turn_lifecycle() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_runtime_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
    let final_turn = AssistantTurn {
        text: Some("runtime turn complete".into()),
        tool_calls: vec![],
    };
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![final_turn]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait(turn_id.clone(), "continue".into(), None)
        .await
        .expect("run turn");

    let ThreadOpResult::UserInput { output, .. } = result else {
        panic!("expected user input result");
    };
    assert_eq!(
        output.final_turn.text.as_deref(),
        Some("runtime turn complete")
    );

    let replay = exagent::transcript::read_session_events(&config.workspace_root, &thread_id)
        .expect("read events");
    assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
    assert!(matches!(
        replay[1].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(replay[2].kind, RuntimeEventKind::TurnCompleted));
    assert_eq!(replay[0].turn_id.as_ref(), Some(&turn_id));
    assert_eq!(replay[2].turn_id.as_ref(), Some(&turn_id));
}

#[tokio::test]
async fn thread_runtime_live_view_tracks_snapshot_after_turn_without_disk_read() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_runtime_live_snapshot");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![AssistantTurn {
            text: Some("live snapshot complete".into()),
            tool_calls: vec![],
        }]),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait(turn_id, "continue".into(), None)
        .await
        .expect("run turn");
    exagent::transcript::write_json(
        &paths.snapshot_path,
        &SessionSnapshot::new_thread(
            thread_id.clone(),
            paths.session_dir.clone(),
            paths.session_dir.clone(),
        ),
    )
    .unwrap();

    let live_view = runtime.live_view().expect("read live view");

    assert_eq!(live_view.snapshot.conversation.len(), 2);
    assert_eq!(live_view.snapshot.conversation[0].content, "continue");
    assert_eq!(
        live_view.snapshot.conversation[1].content,
        "live snapshot complete"
    );
}

fn agent_factory(turns: Vec<AssistantTurn>) -> AgentFactory {
    Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(turns.clone())),
            ToolRegistry::new(),
        ))
    })
}

struct PanicLlm;

#[async_trait]
impl LlmClient for PanicLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<AssistantTurn> {
        panic!("simulated llm panic to verify StoppedGuard");
    }
}

#[tokio::test]
async fn thread_runtime_marks_stopped_when_loop_handler_panics() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_runtime_panic_guard");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let paths = exagent::transcript::session_paths(&config.workspace_root, &thread_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();

    let panicking_factory: AgentFactory =
        Arc::new(move |config| Ok(Agent::new(config, Box::new(PanicLlm), ToolRegistry::new())));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        panicking_factory,
    ))
    .expect("spawn runtime");

    // Submitting user input triggers the panic inside the loop. We expect the
    // completion oneshot to be dropped (sender lost during unwinding), so the
    // await returns Err -- but the important guarantee is below.
    let _ = runtime
        .submit_user_input_and_wait(TurnId::new("turn_panic_1"), "trigger panic".into(), None)
        .await;

    tokio::time::timeout(Duration::from_secs(2), runtime.wait_until_terminated())
        .await
        .expect("StoppedGuard must report termination even when a handler panics");
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
}
