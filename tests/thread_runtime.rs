use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, LlmRequestOptions, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntime, ThreadRuntimeOptions, ThreadRuntimeStatus,
};
use exagent::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
use exagent::session::TurnContextItem;
use exagent::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, LlmCompletion, ThreadId, TurnId,
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
    let rollout_paths = rollout_paths(&config.workspace_root, thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: config.workspace_root.clone(),
            initial_cwd: config.cwd.clone(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        })])
        .expect("write rollout session meta");
}

#[test]
fn thread_session_can_be_constructed_as_runtime_state_owner() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_construct");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("create thread session");

    assert_eq!(session.thread_id(), &thread_id);
}

#[test]
fn thread_without_rollout_meta_is_not_loaded_as_runtime_state() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_no_rollout_meta");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let result = ThreadSession::new(ThreadSessionOptions::new(
        thread_id,
        config,
        agent_factory(vec![]),
    ));

    assert!(result.is_err());
}

#[tokio::test]
async fn thread_runtime_starts_idle_and_accepts_shutdown_op() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_test");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
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
async fn thread_session_loads_rollout_session_meta() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_start");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let _session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("create thread session");

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");

    assert!(matches!(items.first(), Some(RolloutItem::ThreadMeta(_))));
}

#[tokio::test]
async fn thread_resume_reconstructs_context_from_rollout_without_snapshot() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_resume");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let turn_context = TurnContextItem {
        workspace_root: config.workspace_root.clone(),
        cwd: config.cwd.clone(),
        model: "mock".to_string(),
        policy_mode: exagent::policy::PolicyMode::Off,
        command_timeout_secs: 30,
        max_output_bytes: 1024,
        thinking_mode: None,
        current_utc_date: Some("2026-05-20".to_string()),
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: config.workspace_root.clone(),
                initial_cwd: config.cwd.clone(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            RolloutItem::TurnContext(turn_context),
            RolloutItem::ResponseItem(ConversationMessage::user("resume user")),
            RolloutItem::ResponseItem(ConversationMessage::assistant(
                Some("resume assistant".to_string()),
                vec![],
            )),
        ])
        .await
        .expect("write rollout");

    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("resume thread runtime");

    let live_view = runtime.live_view();
    assert_eq!(live_view.snapshot.conversation.len(), 2);
    assert_eq!(live_view.snapshot.conversation[0].content, "resume user");
    assert_eq!(
        live_view.snapshot.conversation[1].content,
        "resume assistant"
    );
    assert!(live_view.snapshot.reference_turn_context.is_some());
}

#[tokio::test]
async fn runtime_restore_uses_rollout_projection_for_compaction_metadata() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_compaction_restore");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: config.workspace_root.clone(),
                initial_cwd: config.cwd.clone(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            RolloutItem::ResponseItem(ConversationMessage::user("pre-compaction user")),
            RolloutItem::Compacted(exagent::state::rollout::CompactedItem {
                message: "compacted history".to_string(),
                replacement_history: Some(vec![ConversationMessage::assistant(
                    Some("summary history".to_string()),
                    vec![],
                )]),
            }),
        ])
        .await
        .expect("write rollout");

    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        agent_factory(vec![]),
    ))
    .expect("resume thread runtime");

    let live_view = runtime.live_view();
    assert_eq!(
        live_view
            .snapshot
            .latest_compaction
            .as_ref()
            .map(|compaction| compaction.summary.as_str()),
        Some("compacted history")
    );
    assert_eq!(live_view.snapshot.conversation.len(), 1);
    assert_eq!(
        live_view.snapshot.conversation[0].content,
        "summary history"
    );
}

#[tokio::test]
async fn thread_runtime_live_view_uses_loaded_session_state_not_disk_mutations() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_live_view");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let rollout_paths = exagent::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    exagent::transcript::append_json_line(
        &rollout_paths.rollout_path,
        &exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_disk_only"),
            thread_id: thread_id.clone(),
            turn_id: Some(TurnId::new("turn_disk_only")),
            kind: RuntimeEventKind::RuntimeError {
                message: "disk mutation after runtime load".into(),
            },
        }),
    )
    .expect("append disk-only event");

    let live_view = runtime.live_view();

    assert_eq!(live_view.thread_id, thread_id);
    assert!(live_view.events.is_empty());
    assert!(live_view.snapshot.conversation.is_empty());
}

#[tokio::test]
async fn thread_runtime_runs_user_input_through_agent_and_records_turn_lifecycle() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
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

    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        panic!("expected user input result");
    };
    assert_eq!(final_turn.text.as_deref(), Some("runtime turn complete"));

    let live_view = runtime.live_view();
    assert!(matches!(
        live_view.events[0].kind,
        RuntimeEventKind::TurnStarted
    ));
    assert!(matches!(
        live_view.events[1].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(
        live_view.events[2].kind,
        RuntimeEventKind::TurnCompleted
    ));
    assert_eq!(live_view.events[0].turn_id.as_ref(), Some(&turn_id));
    assert_eq!(live_view.events[2].turn_id.as_ref(), Some(&turn_id));

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event) if matches!(event.kind, RuntimeEventKind::TurnStarted)
    )));
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event) if matches!(event.kind, RuntimeEventKind::TurnCompleted)
    )));
}

#[tokio::test]
async fn thread_turn_records_rollout_items_and_context_history() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![AssistantTurn {
            text: Some("rollout assistant".into()),
            tool_calls: vec![],
        }]),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait(turn_id, "rollout user".into(), None)
        .await
        .expect("run turn");

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");

    assert!(items
        .iter()
        .any(|item| matches!(item, RolloutItem::TurnContext(_))));
    assert!(items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(message) if message.content == "rollout user"
    )));
    assert!(items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(message) if message.content == "rollout assistant"
    )));

    let live_view = runtime.live_view();
    assert_eq!(live_view.snapshot.conversation.len(), 4);
    assert!(live_view.snapshot.conversation[0].injected);
    assert!(live_view.snapshot.conversation[1].injected);
    assert_eq!(live_view.snapshot.conversation[2].content, "rollout user");
    assert_eq!(
        live_view.snapshot.conversation[3].content,
        "rollout assistant"
    );
}

#[tokio::test]
async fn rollout_thread_turn_does_not_write_snapshot_or_events_files() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_no_legacy_files");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: config.workspace_root.clone(),
            initial_cwd: config.cwd.clone(),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        })])
        .await
        .expect("write rollout meta");
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![AssistantTurn {
            text: Some("no legacy writes".into()),
            tool_calls: vec![],
        }]),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait(turn_id, "continue".into(), None)
        .await
        .expect("run turn");

    assert!(rollout_paths.rollout_path.exists());
    let sessions_dir = config.workspace_root.join(".exagent").join("sessions");
    assert!(!sessions_dir.exists());
}

#[tokio::test]
async fn thread_runtime_live_view_tracks_snapshot_after_turn_without_disk_read() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_live_snapshot");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
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

    let live_view = runtime.live_view();

    assert_eq!(live_view.snapshot.conversation.len(), 4);
    assert!(live_view.snapshot.reference_turn_context.is_some());
    assert!(live_view.snapshot.conversation[0]
        .content
        .contains("Runtime context:"));
    assert!(live_view.snapshot.conversation[1]
        .content
        .contains("Environment context:"));
    assert_eq!(live_view.snapshot.conversation[2].content, "continue");
    assert_eq!(
        live_view.snapshot.conversation[3].content,
        "live snapshot complete"
    );
}

#[tokio::test]
async fn thread_runtime_next_turn_id_uses_live_state_not_disk() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_next_turn_id");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![AssistantTurn {
            text: Some("first turn complete".into()),
            tool_calls: vec![],
        }]),
    ))
    .expect("spawn runtime");

    assert_eq!(runtime.next_turn_id(), TurnId::new("turn_1"));
    runtime
        .submit_user_input_and_wait(TurnId::new("turn_1"), "continue".into(), None)
        .await
        .expect("run turn");

    assert_eq!(runtime.next_turn_id(), TurnId::new("turn_2"));
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
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        panic!("simulated llm panic to verify StoppedGuard");
    }
}

#[tokio::test]
async fn thread_runtime_marks_stopped_when_loop_handler_panics() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_panic_guard");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

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
