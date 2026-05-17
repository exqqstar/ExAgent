use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use exagent::runtime::thread_runtime::{
    AgentFactory, ThreadOp, ThreadOpResult, ThreadRuntime, ThreadRuntimeOptions,
    ThreadRuntimeStatus,
};
use exagent::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
use exagent::session::SessionSnapshot;
use exagent::types::{AssistantTurn, SessionId, TurnId};
use std::sync::Arc;
use tempfile::tempdir;

#[test]
fn thread_session_can_be_constructed_as_runtime_state_owner() {
    let thread_id = SessionId::new("session_thread_session_construct");
    let config = AgentConfig::default();
    let session = ThreadSession::new(ThreadSessionOptions::new(thread_id.clone(), config));

    assert_eq!(session.thread_id(), &thread_id);
}

#[tokio::test]
async fn thread_runtime_starts_idle_and_accepts_shutdown_op() {
    let thread_id = SessionId::new("session_runtime_test");
    let config = AgentConfig::default();
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(thread_id.clone(), config))
        .expect("spawn runtime");

    assert_eq!(runtime.thread_id(), &thread_id);
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Idle);

    runtime
        .submit(ThreadOp::Shutdown)
        .await
        .expect("submit shutdown");
    runtime.wait_until_terminated().await;
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
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
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![final_turn.clone()])),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(
        ThreadRuntimeOptions::new(thread_id.clone(), config.clone())
            .with_agent_factory(agent_factory),
    )
    .expect("spawn runtime");

    let result = runtime
        .submit_and_wait(ThreadOp::UserInput {
            turn_id: turn_id.clone(),
            prompt: "continue".into(),
            turn_context: None,
        })
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
