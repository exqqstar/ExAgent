use exagent::config::AgentConfig;
use exagent::policy::PolicyMode;
use exagent::runtime::{
    ConfigManager, ManagedThreadStatus, RuntimeController, RuntimeEngine, RuntimeExecution,
    RuntimeOp, RuntimeOpExecutor, ThreadManager, ThreadStartRequest, TurnContextRequest,
    TurnStartRequest, UserInput,
};
use exagent::session::AgentRole;
use exagent::types::SessionId;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{timeout, Duration};

#[test]
fn config_manager_builds_turn_context_from_validated_request() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();

    let manager = ConfigManager::new(AgentConfig {
        model: "base-model".into(),
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        policy_mode: PolicyMode::Off,
        ..AgentConfig::default()
    });

    let context = manager
        .build_turn_context(TurnContextRequest {
            cwd: Some("nested".into()),
            model: Some("turn-model".into()),
            agent_role: Some(AgentRole::Implementation),
            instructions: vec!["prefer small runtime steps".into()],
            ..TurnContextRequest::default()
        })
        .unwrap();

    let expected_root = std::fs::canonicalize(dir.path()).unwrap();
    let expected_cwd = std::fs::canonicalize(&nested).unwrap();

    assert_eq!(context.model, "turn-model");
    assert_eq!(context.workspace_root, expected_root);
    assert_eq!(context.cwd, expected_cwd);
    assert_eq!(context.policy_mode, PolicyMode::Off);
    assert_eq!(context.agent_role, AgentRole::Implementation);
    assert_eq!(context.instructions, vec!["prefer small runtime steps"]);
}

#[test]
fn config_manager_rejects_cwd_outside_workspace() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let manager = ConfigManager::new(AgentConfig {
        workspace_root: workspace.path().to_path_buf(),
        cwd: workspace.path().to_path_buf(),
        ..AgentConfig::default()
    });

    let err = manager
        .build_turn_context(TurnContextRequest {
            cwd: Some(outside.path().display().to_string()),
            ..TurnContextRequest::default()
        })
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));
}

#[test]
fn config_manager_defaults_cwd_to_request_workspace_root_when_workspace_is_overridden() {
    let base = tempdir().unwrap();
    let requested = tempdir().unwrap();
    let manager = ConfigManager::new(AgentConfig {
        workspace_root: base.path().to_path_buf(),
        cwd: base.path().to_path_buf(),
        ..AgentConfig::default()
    });

    let context = manager
        .build_turn_context(TurnContextRequest {
            workspace_root: Some(requested.path().display().to_string()),
            ..TurnContextRequest::default()
        })
        .unwrap();

    let expected = std::fs::canonicalize(requested.path()).unwrap();
    assert_eq!(context.workspace_root, expected);
    assert_eq!(context.cwd, expected);
}

#[tokio::test]
async fn thread_manager_reuses_live_thread_and_queues_ops_in_order() {
    let manager = ThreadManager::default();
    let session_id = SessionId::new("session_1");

    let first = manager.get_or_start(session_id.clone()).await;
    let second = manager.get_or_start(session_id.clone()).await;

    assert!(first.same_thread(&second));
    assert_eq!(manager.live_thread_count().await, 1);

    first
        .submit(RuntimeOp::SetThreadName {
            name: "first".into(),
        })
        .await
        .unwrap();
    second.submit(RuntimeOp::Shutdown).await.unwrap();

    assert_eq!(
        first.next_op().await.unwrap(),
        RuntimeOp::SetThreadName {
            name: "first".into()
        }
    );
    assert_eq!(first.next_op().await.unwrap(), RuntimeOp::Shutdown);
}

#[tokio::test]
async fn thread_manager_tracks_status_and_rejects_ops_for_archived_threads() {
    let manager = ThreadManager::default();
    let session_id = SessionId::new("session_1");
    let handle = manager.get_or_start(session_id.clone()).await;

    assert_eq!(handle.status().await, ManagedThreadStatus::Idle);

    handle.set_status(ManagedThreadStatus::Running).await;
    let same_handle = manager.get_or_start(session_id).await;
    assert_eq!(same_handle.status().await, ManagedThreadStatus::Running);

    same_handle.set_status(ManagedThreadStatus::Archived).await;
    let err = handle.submit(RuntimeOp::Shutdown).await.unwrap_err();

    assert!(err.to_string().contains("archived thread"));
}

#[tokio::test]
async fn thread_manager_does_not_return_archived_threads_as_live() {
    let manager = ThreadManager::default();
    let session_id = SessionId::new("session_1");
    let handle = manager.get_or_start(session_id.clone()).await;

    assert!(manager.get_live(&session_id).await.is_some());

    handle.set_status(ManagedThreadStatus::Archived).await;

    assert!(manager.get_live(&session_id).await.is_none());
}

#[tokio::test]
async fn thread_handle_execution_lock_serializes_same_thread_work() {
    let manager = ThreadManager::default();
    let handle = manager.get_or_start(SessionId::new("session_1")).await;
    let first_guard = handle.lock_execution().await;
    let marker = Arc::new(Mutex::new(Vec::new()));

    let competing_handle = handle.clone();
    let competing_marker = marker.clone();
    let task = tokio::spawn(async move {
        let _second_guard = competing_handle.lock_execution().await;
        competing_marker.lock().await.push("second");
    });

    tokio::task::yield_now().await;
    assert!(marker.lock().await.is_empty());

    drop(first_guard);
    task.await.unwrap();
    assert_eq!(*marker.lock().await, vec!["second"]);
}

#[tokio::test]
async fn runtime_controller_starts_thread_and_queues_user_input_op() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let controller = RuntimeController::new(AgentConfig {
        model: "base-model".into(),
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        policy_mode: PolicyMode::Off,
        ..AgentConfig::default()
    });

    let thread = controller
        .start_thread(ThreadStartRequest {
            context: TurnContextRequest {
                cwd: Some("nested".into()),
                model: Some("thread-model".into()),
                agent_role: Some(AgentRole::Implementation),
                instructions: vec!["use the runtime queue".into()],
                ..TurnContextRequest::default()
            },
        })
        .await
        .unwrap();

    let turn = controller
        .start_turn(TurnStartRequest {
            session_id: thread.session_id.clone(),
            input: vec![UserInput {
                content: "continue the work".into(),
            }],
            context: TurnContextRequest::default(),
        })
        .await
        .unwrap();

    assert_eq!(turn.status, "queued");

    let handle = controller.thread_handle(&thread.session_id).await.unwrap();
    let op = handle.next_op().await.unwrap();

    let RuntimeOp::UserInput {
        turn_id,
        input,
        context,
    } = op
    else {
        panic!("expected user input op");
    };

    assert_eq!(turn_id, turn.turn_id);
    assert_eq!(input[0].content, "continue the work");
    assert_eq!(context.model, "thread-model");
    assert_eq!(context.cwd, std::fs::canonicalize(&nested).unwrap());
    assert_eq!(context.agent_role, AgentRole::Implementation);
    assert_eq!(context.instructions, vec!["use the runtime queue"]);
}

#[tokio::test]
async fn runtime_controller_rejects_turns_for_unknown_threads() {
    let dir = tempdir().unwrap();
    let controller = RuntimeController::new(AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    });
    let session_id = SessionId::new("missing_session");

    let err = controller
        .start_turn(TurnStartRequest {
            session_id: session_id.clone(),
            input: vec![UserInput {
                content: "continue".into(),
            }],
            context: TurnContextRequest::default(),
        })
        .await
        .unwrap_err();

    assert!(err.to_string().contains("thread not found"));
    assert!(controller.thread_handle(&session_id).await.is_none());
}

#[tokio::test]
async fn runtime_controller_merges_partial_turn_context_with_thread_defaults() {
    let dir = tempdir().unwrap();
    let controller = RuntimeController::new(AgentConfig {
        model: "base-model".into(),
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        policy_mode: PolicyMode::Off,
        ..AgentConfig::default()
    });
    let thread = controller
        .start_thread(ThreadStartRequest {
            context: TurnContextRequest {
                model: Some("thread-model".into()),
                agent_role: Some(AgentRole::Implementation),
                instructions: vec!["preserve this".into()],
                ..TurnContextRequest::default()
            },
        })
        .await
        .unwrap();

    controller
        .start_turn(TurnStartRequest {
            session_id: thread.session_id.clone(),
            input: vec![UserInput {
                content: "continue".into(),
            }],
            context: TurnContextRequest {
                model: Some("turn-model".into()),
                ..TurnContextRequest::default()
            },
        })
        .await
        .unwrap();

    let handle = controller.thread_handle(&thread.session_id).await.unwrap();
    let RuntimeOp::UserInput { context, .. } = handle.next_op().await.unwrap() else {
        panic!("expected user input op");
    };

    assert_eq!(context.model, "turn-model");
    assert_eq!(context.agent_role, AgentRole::Implementation);
    assert_eq!(context.instructions, vec!["preserve this"]);
}

#[tokio::test]
async fn runtime_controller_rejects_empty_turn_input() {
    let dir = tempdir().unwrap();
    let controller = RuntimeController::new(AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    });
    let thread = controller
        .start_thread(ThreadStartRequest::default())
        .await
        .unwrap();

    let err = controller
        .start_turn(TurnStartRequest {
            session_id: thread.session_id,
            input: vec![],
            context: TurnContextRequest::default(),
        })
        .await
        .unwrap_err();

    assert!(err.to_string().contains("turn input cannot be empty"));
}

#[tokio::test]
async fn runtime_engine_consumes_next_queued_op_and_restores_idle_status() {
    let manager = ThreadManager::default();
    let handle = manager.get_or_start(SessionId::new("session_1")).await;
    let context = ConfigManager::new(AgentConfig::default())
        .build_turn_context(TurnContextRequest::default())
        .unwrap();
    let turn_id = exagent::types::TurnId::new("turn_1");
    handle
        .submit(RuntimeOp::UserInput {
            turn_id: turn_id.clone(),
            input: vec![UserInput {
                content: "run this".into(),
            }],
            context,
        })
        .await
        .unwrap();

    let executor = RecordingExecutor::default();
    let seen = executor.seen.clone();
    let engine = RuntimeEngine::new(executor);

    let result = engine.run_next(&handle).await.unwrap().unwrap();

    assert_eq!(handle.status().await, ManagedThreadStatus::Idle);
    assert_eq!(result.session_id, SessionId::new("session_1"));
    assert_eq!(result.turn_id, Some(turn_id.clone()));
    assert_eq!(result.status, "completed");
    assert!(matches!(
        &seen.lock().await[0],
        RuntimeOp::UserInput {
            turn_id: recorded_turn_id,
            ..
        } if *recorded_turn_id == turn_id
    ));
}

#[tokio::test]
async fn runtime_engine_returns_none_when_no_op_is_queued() {
    let manager = ThreadManager::default();
    let handle = manager.get_or_start(SessionId::new("session_1")).await;
    let engine = RuntimeEngine::new(RecordingExecutor::default());

    let result = timeout(Duration::from_millis(50), engine.run_next(&handle))
        .await
        .expect("run_next should not block when no op is queued")
        .unwrap();

    assert_eq!(result, None);
    assert_eq!(handle.status().await, ManagedThreadStatus::Idle);
}

#[tokio::test]
async fn runtime_engine_serializes_concurrent_runs_for_same_thread() {
    let manager = ThreadManager::default();
    let handle = manager.get_or_start(SessionId::new("session_1")).await;
    let context = ConfigManager::new(AgentConfig::default())
        .build_turn_context(TurnContextRequest::default())
        .unwrap();
    handle
        .submit(RuntimeOp::UserInput {
            turn_id: exagent::types::TurnId::new("turn_1"),
            input: vec![UserInput {
                content: "first".into(),
            }],
            context: context.clone(),
        })
        .await
        .unwrap();
    handle
        .submit(RuntimeOp::UserInput {
            turn_id: exagent::types::TurnId::new("turn_2"),
            input: vec![UserInput {
                content: "second".into(),
            }],
            context,
        })
        .await
        .unwrap();

    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let engine = Arc::new(RuntimeEngine::new(BlockingExecutor {
        started_tx,
        release: release.clone(),
    }));

    let first_task = {
        let engine = engine.clone();
        let handle = handle.clone();
        tokio::spawn(async move { engine.run_next(&handle).await })
    };
    let second_task = {
        let engine = engine.clone();
        let handle = handle.clone();
        tokio::spawn(async move { engine.run_next(&handle).await })
    };

    assert_eq!(started_rx.recv().await.unwrap(), "first");
    assert!(
        timeout(Duration::from_millis(50), started_rx.recv())
            .await
            .is_err(),
        "second op started while the first op was still executing"
    );

    release.notify_one();
    first_task.await.unwrap().unwrap().unwrap();

    assert_eq!(
        timeout(Duration::from_millis(50), started_rx.recv())
            .await
            .unwrap()
            .unwrap(),
        "second"
    );
    release.notify_one();
    second_task.await.unwrap().unwrap().unwrap();
}

#[derive(Clone, Default)]
struct RecordingExecutor {
    seen: Arc<Mutex<Vec<RuntimeOp>>>,
}

#[async_trait::async_trait]
impl RuntimeOpExecutor for RecordingExecutor {
    async fn execute_op(
        &self,
        session_id: &SessionId,
        op: RuntimeOp,
    ) -> anyhow::Result<RuntimeExecution> {
        let turn_id = match &op {
            RuntimeOp::UserInput { turn_id, .. } => Some(turn_id.clone()),
            _ => None,
        };
        self.seen.lock().await.push(op);
        Ok(RuntimeExecution {
            session_id: session_id.clone(),
            turn_id,
            status: "completed".into(),
        })
    }
}

#[derive(Clone)]
struct BlockingExecutor {
    started_tx: mpsc::UnboundedSender<String>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl RuntimeOpExecutor for BlockingExecutor {
    async fn execute_op(
        &self,
        session_id: &SessionId,
        op: RuntimeOp,
    ) -> anyhow::Result<RuntimeExecution> {
        let (turn_id, label) = match &op {
            RuntimeOp::UserInput { turn_id, input, .. } => (
                Some(turn_id.clone()),
                input
                    .first()
                    .map(|item| item.content.clone())
                    .unwrap_or_default(),
            ),
            _ => (None, String::new()),
        };
        self.started_tx.send(label).unwrap();
        self.release.notified().await;

        Ok(RuntimeExecution {
            session_id: session_id.clone(),
            turn_id,
            status: "completed".into(),
        })
    }
}
