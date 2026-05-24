use anyhow::Result;
use async_trait::async_trait;
use exagent::app_server::protocol::{
    BoundaryCapability, BoundaryOp, BoundaryOpResponse, EventsReplayParams, IgnoredOverrideField,
    InitializeParams, RunParams, RuntimeEventKindFilter, ThreadItem, ThreadReadParams,
    ThreadResumeParams, ThreadStartParams, ThreadStatus, ThreadView, TurnContextOverrides,
    TurnInterruptParams, TurnStartParams, TurnStatus,
};
use exagent::app_server::{AppServerError, AppServerService};
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, MockLlm};
use exagent::policy::PolicyMode;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::session::{AgentRole, ApprovalId, SessionSnapshot};
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::Tool;
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, LlmCompletion, SessionId, TokenUsage,
    TokenUsageInfo, ToolCall, ToolResult, ToolStatus, TurnId,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::sync::Notify;

struct BlockingLlm {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Clone)]
struct CwdProbeTool {
    observed_cwd: Arc<Mutex<Option<PathBuf>>>,
}

#[async_trait]
impl Tool for CwdProbeTool {
    fn name(&self) -> &'static str {
        "cwd_probe"
    }

    fn description(&self) -> &'static str {
        "Record the active tool cwd"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        *self.observed_cwd.lock().unwrap() = Some(ctx.config.cwd.clone());
        ToolResult {
            tool_call_id: call.id,
            tool_name: call.name,
            status: ToolStatus::Success,
            content: ctx.config.cwd.display().to_string(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for BlockingLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> Result<LlmCompletion> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(AssistantTurn {
            text: Some("released turn".into()),
            tool_calls: vec![],
        }
        .into_completion())
    }
}

fn events_replay_params(thread_id: SessionId) -> EventsReplayParams {
    EventsReplayParams {
        thread_id,
        workspace_root: None,
        after_event_id: None,
        limit: None,
        include_snapshot: false,
        event_kinds: vec![],
    }
}

fn run_command_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);
    registry
}

fn read_thread_snapshot(thread: &ThreadView) -> SessionSnapshot {
    if thread.snapshot_path.exists() {
        return exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
    }

    let workspace_root = workspace_root_from_legacy_snapshot_path(&thread.snapshot_path);
    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace_root, &thread.id);
    let items =
        exagent::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
            .unwrap();
    exagent::state::rollout::snapshot_from_rollout_items(&thread.id, &items).unwrap()
}

fn assert_rollout_jsonl_is_valid(thread: &ThreadView) {
    let workspace_root = workspace_root_from_legacy_snapshot_path(&thread.snapshot_path);
    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace_root, &thread.id);
    let contents = std::fs::read_to_string(rollout_paths.rollout_path).unwrap();
    for line in contents.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
}

fn workspace_root_from_legacy_snapshot_path(snapshot_path: &std::path::Path) -> PathBuf {
    snapshot_path
        .parent()
        .and_then(|session_dir| session_dir.parent())
        .and_then(|sessions_dir| sessions_dir.parent())
        .and_then(|exagent_dir| exagent_dir.parent())
        .expect("legacy snapshot path should be under <workspace>/.exagent/sessions/<thread>")
        .to_path_buf()
}

async fn wait_for_turn_event(
    service: &AppServerService,
    thread_id: &SessionId,
    turn_id: &TurnId,
    predicate: impl Fn(&RuntimeEventKind) -> bool,
) {
    for _ in 0..200 {
        let replay = service
            .events_replay(events_replay_params(thread_id.clone()))
            .unwrap();
        if replay
            .events
            .iter()
            .filter(|event| event.turn_id.as_ref() == Some(turn_id))
            .any(|event| predicate(&event.kind))
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for turn event");
}

async fn wait_for_turn_completed(
    service: &AppServerService,
    thread_id: &SessionId,
    turn_id: &TurnId,
) {
    wait_for_turn_event(service, thread_id, turn_id, |kind| {
        matches!(kind, RuntimeEventKind::TurnCompleted)
    })
    .await;
}

async fn wait_for_runtime_error(
    service: &AppServerService,
    thread_id: &SessionId,
    turn_id: &TurnId,
) {
    wait_for_turn_event(service, thread_id, turn_id, |kind| {
        matches!(kind, RuntimeEventKind::RuntimeError { .. })
    })
    .await;
}

async fn wait_for_thread_event(
    service: &AppServerService,
    thread_id: &SessionId,
    predicate: impl Fn(&RuntimeEventKind) -> bool,
) {
    for _ in 0..200 {
        let replay = service
            .events_replay(events_replay_params(thread_id.clone()))
            .unwrap();
        if replay.events.iter().any(|event| predicate(&event.kind)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for thread event");
}

#[tokio::test]
async fn initialize_boundary_advertises_v2_protocol_surface() {
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );

    let response = service
        .submit_boundary_op(BoundaryOp::Initialize(InitializeParams {}))
        .await
        .unwrap();

    let BoundaryOpResponse::Initialized(initialized) = response else {
        panic!("expected initialize response");
    };
    assert_eq!(
        initialized.protocol_version,
        "appserver-runtime-boundary-v2"
    );
    assert_eq!(
        initialized.supported_ops,
        vec![
            BoundaryCapability::Initialize,
            BoundaryCapability::ThreadStart,
            BoundaryCapability::ThreadResume,
            BoundaryCapability::ThreadRead,
            BoundaryCapability::TurnStart,
            BoundaryCapability::TurnInterrupt,
            BoundaryCapability::EventsReplay,
        ]
    );
    assert_eq!(
        initialized.supported_streams,
        vec![BoundaryCapability::EventsSubscribe]
    );
}

#[test]
fn boundary_capabilities_match_boundary_op_type_names() {
    let cases = vec![
        (
            BoundaryCapability::Initialize,
            serde_json::to_value(BoundaryOp::Initialize(InitializeParams {})).unwrap(),
        ),
        (
            BoundaryCapability::ThreadStart,
            serde_json::to_value(BoundaryOp::ThreadStart(ThreadStartParams {
                workspace_root: None,
                cwd: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::ThreadResume,
            serde_json::to_value(BoundaryOp::ThreadResume(ThreadResumeParams {
                thread_id: SessionId::new("session_123"),
                workspace_root: None,
                cwd: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::ThreadRead,
            serde_json::to_value(BoundaryOp::ThreadRead(ThreadReadParams {
                thread_id: SessionId::new("session_123"),
                workspace_root: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::TurnStart,
            serde_json::to_value(BoundaryOp::TurnStart(TurnStartParams {
                thread_id: SessionId::new("session_123"),
                prompt: "continue".into(),
                workspace_root: None,
                turn_context: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::TurnInterrupt,
            serde_json::to_value(BoundaryOp::TurnInterrupt(TurnInterruptParams {
                thread_id: SessionId::new("session_123"),
                turn_id: None,
                workspace_root: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::EventsReplay,
            serde_json::to_value(BoundaryOp::EventsReplay(events_replay_params(
                SessionId::new("session_123"),
            )))
            .unwrap(),
        ),
    ];

    for (capability, op) in cases {
        assert_eq!(
            op["type"],
            serde_json::to_value(capability).unwrap(),
            "capability and boundary op type must stay aligned"
        );
    }
}

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

    let snapshot = read_thread_snapshot(&response.thread);
    assert_eq!(snapshot.session_id, response.thread.id);
    assert_eq!(snapshot.parent_session_id, None);
    assert_eq!(snapshot.root_session_id, response.thread.id);
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
            thread_id: thread.thread.id.clone(),
            prompt: "continue work".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    assert_eq!(turn.thread_id, thread.thread.id);
    assert_eq!(turn.turn.id, TurnId::new("turn_1"));
    assert_eq!(turn.turn.status, TurnStatus::InProgress);

    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let replay = service
        .events_replay(events_replay_params(thread.thread.id.clone()))
        .unwrap();
    assert_eq!(replay.thread_id, thread.thread.id);
    assert_eq!(replay.events.len(), 3);
    assert!(matches!(
        &replay.events[0].kind,
        RuntimeEventKind::TurnStarted
    ));
    assert!(matches!(
        &replay.events[1].kind,
        RuntimeEventKind::AssistantTurn { turn } if turn.text.as_deref() == Some("thread turn complete")
    ));
    assert!(matches!(
        &replay.events[2].kind,
        RuntimeEventKind::TurnCompleted
    ));
    assert_eq!(replay.events[0].turn_id, Some(turn.turn.id.clone()));
    assert_eq!(replay.events[1].turn_id, Some(turn.turn.id.clone()));
    assert_eq!(replay.events[2].turn_id, Some(turn.turn.id));
}

#[tokio::test]
async fn thread_read_reconstructs_turn_view_from_replayed_events() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("need tool".into()),
                tool_calls: vec![ToolCall {
                    id: "call_cwd_probe".into(),
                    name: "cwd_probe".into(),
                    arguments: serde_json::json!({}),
                }],
            },
            AssistantTurn {
                text: Some("tool done".into()),
                tool_calls: vec![],
            },
        ])),
        || {
            let mut registry = ToolRegistry::new();
            registry.register(CwdProbeTool {
                observed_cwd: Arc::new(Mutex::new(None)),
            });
            registry
        },
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run a tool".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let replay = service
        .events_replay(events_replay_params(thread.thread.id.clone()))
        .unwrap();
    assert_eq!(replay.events.len(), 5);
    assert!(replay
        .events
        .iter()
        .all(|event| event.turn_id.as_ref() == Some(&turn.turn.id)));

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id,
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.thread.turns.len(), 1);
    let view = read.thread.turns.last().unwrap();
    assert_eq!(view.id, turn.turn.id);
    assert_eq!(view.status, TurnStatus::Completed);
    assert_eq!(view.items.len(), 3);
    assert!(matches!(
        &view.items[0],
        ThreadItem::AssistantMessage { text } if text.as_deref() == Some("need tool")
    ));
    assert!(matches!(
        &view.items[1],
        ThreadItem::ToolResult { name } if name == "cwd_probe"
    ));
    assert!(matches!(
        &view.items[2],
        ThreadItem::AssistantMessage { text } if text.as_deref() == Some("tool done")
    ));
}

#[tokio::test]
async fn events_subscribe_receives_live_turn_lifecycle_events() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("live event complete".into()),
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
    let mut events = service
        .events_subscribe(exagent::app_server::protocol::EventsSubscribeParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
        })
        .unwrap();

    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "stream live events".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let first = events.recv().await.unwrap();
    assert_eq!(first.turn_id.as_ref(), Some(&turn.turn.id));
    assert!(matches!(first.kind, RuntimeEventKind::TurnStarted));
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;
}

#[tokio::test]
async fn events_subscribe_receives_live_approval_requested_events() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("try risky command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky_live_subscribe".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                }],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let mut events = service
        .events_subscribe(exagent::app_server::protocol::EventsSubscribeParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
        })
        .unwrap();

    let _turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let approval_event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("live event channel open");
            if matches!(event.kind, RuntimeEventKind::ApprovalRequested { .. }) {
                return event;
            }
        }
    })
    .await
    .expect("approval request must be delivered through live subscribe");

    assert!(matches!(
        approval_event.kind,
        RuntimeEventKind::ApprovalRequested { .. }
    ));
    let read_at_approval = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(
        read_at_approval.thread.status,
        ThreadStatus::WaitingApproval
    );

    wait_for_thread_event(&service, &thread.thread.id, |kind| {
        matches!(kind, RuntimeEventKind::TurnCompleted)
    })
    .await;
    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert!(read.thread.turns.iter().any(|turn| {
        turn.items
            .iter()
            .any(|item| matches!(item, ThreadItem::ApprovalRequested { .. }))
    }));
}

#[test]
fn events_subscribe_rejects_missing_thread() {
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

    let err = service
        .events_subscribe(exagent::app_server::protocol::EventsSubscribeParams {
            thread_id: SessionId::new("missing-thread"),
            workspace_root: None,
            after_event_id: None,
        })
        .unwrap_err();
    assert!(err.to_string().contains("thread not found: missing-thread"));
}

#[tokio::test]
async fn turn_start_applies_validated_context_override_with_user_input() {
    let dir = tempdir().unwrap();
    let original_cwd = dir.path().join("original-cwd");
    let turn_cwd = dir.path().join("turn-cwd");
    std::fs::create_dir_all(&original_cwd).unwrap();
    std::fs::create_dir_all(&turn_cwd).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("turn context complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run from new cwd".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert!(snapshot
        .conversation
        .iter()
        .any(|message| message.content == "run from new cwd"));
}

#[tokio::test]
async fn turn_context_cwd_is_used_for_tools_without_becoming_thread_cwd() {
    let dir = tempdir().unwrap();
    let original_cwd = dir.path().join("original-cwd");
    let turn_cwd = dir.path().join("turn-cwd");
    std::fs::create_dir_all(&original_cwd).unwrap();
    std::fs::create_dir_all(&turn_cwd).unwrap();
    let observed_cwd = Arc::new(Mutex::new(None));
    let observed_for_registry = observed_cwd.clone();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_cwd_probe".into(),
                    name: "cwd_probe".into(),
                    arguments: serde_json::json!({}),
                }],
            },
            AssistantTurn {
                text: Some("cwd probed".into()),
                tool_calls: vec![],
            },
        ])),
        move || {
            let mut registry = ToolRegistry::new();
            registry.register(CwdProbeTool {
                observed_cwd: observed_for_registry.clone(),
            });
            registry
        },
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "probe cwd".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    assert_eq!(
        *observed_cwd.lock().unwrap(),
        Some(std::fs::canonicalize(turn_cwd).unwrap())
    );
    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
}

#[tokio::test]
async fn turn_start_rejects_invalid_context_override_before_accepting_input() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("must not run".into()),
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
    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "must not be accepted".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some(outside.path().to_string_lossy().to_string()),
            }),
        })
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));

    let snapshot = read_thread_snapshot(&thread.thread);
    assert!(snapshot.conversation.is_empty());
    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    assert!(replay.events.is_empty());
}

#[tokio::test]
async fn legacy_run_compatibility_uses_thread_and_turn_lifecycle() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("compat run complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let output = service
        .run(RunParams {
            prompt: "run through compatibility wrapper".into(),
            workspace_root: None,
            cwd: None,
            session_id: None,
        })
        .await
        .unwrap();

    let replay = service
        .events_replay(events_replay_params(output.session_id))
        .unwrap();

    assert_eq!(replay.events.len(), 3);
    assert!(matches!(
        &replay.events[0].kind,
        RuntimeEventKind::TurnStarted
    ));
    assert!(matches!(
        &replay.events[1].kind,
        RuntimeEventKind::AssistantTurn { turn } if turn.text.as_deref() == Some("compat run complete")
    ));
    assert!(matches!(
        &replay.events[2].kind,
        RuntimeEventKind::TurnCompleted
    ));
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
        .events_replay(events_replay_params(SessionId::new(
            thread.thread.id.as_str(),
        )))
        .unwrap();

    assert_eq!(replay.events, vec![]);
}

#[test]
fn events_replay_can_include_latest_snapshot_for_ui_reconstruction() {
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
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: true,
            event_kinds: vec![],
        })
        .unwrap();

    let snapshot = replay.snapshot.expect("snapshot should be included");
    assert_eq!(snapshot.thread_id, thread.thread.id);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(dir.path()).unwrap());
    assert_eq!(snapshot.latest_compaction, None);
    assert_eq!(snapshot.open_exec_session_count, 0);
    assert_eq!(snapshot.conversation_message_count, 0);
    assert_eq!(snapshot.pending_approval_count, 0);
}

#[tokio::test]
async fn events_replay_snapshot_includes_latest_compaction_after_auto_compact() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            model_context_window: Some(1_000),
            auto_compact_token_limit: Some(1),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("first done".into()),
                tool_calls: vec![],
            },
            AssistantTurn {
                text: Some("summary after first".into()),
                tool_calls: vec![],
            },
            AssistantTurn {
                text: Some("second done".into()),
                tool_calls: vec![],
            },
        ])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let first_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "first prompt".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &first_turn.turn.id).await;
    let second_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "second prompt".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &second_turn.turn.id).await;

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id,
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: true,
            event_kinds: vec![],
        })
        .unwrap();
    let snapshot = replay.snapshot.expect("snapshot should be included");

    assert_eq!(
        snapshot
            .latest_compaction
            .as_ref()
            .map(|summary| summary.summary.as_str()),
        Some("summary after first")
    );
}

#[tokio::test]
async fn events_replay_snapshot_counts_live_open_exec_sessions_from_overlay_only() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("start persistent command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_start_persistent".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "printf 'ready\\n'; sleep 30",
                        "persistent": true
                    }),
                }],
            },
            AssistantTurn {
                text: Some("persistent command started".into()),
                tool_calls: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "open persistent command".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let live_replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: true,
            event_kinds: vec![],
        })
        .unwrap();
    assert_eq!(
        live_replay
            .snapshot
            .as_ref()
            .expect("live snapshot")
            .open_exec_session_count,
        1
    );

    let cold_service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        run_command_registry,
    );
    let cold_replay = cold_service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id,
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: true,
            event_kinds: vec![],
        })
        .unwrap();

    assert_eq!(
        cold_replay
            .snapshot
            .expect("cold snapshot")
            .open_exec_session_count,
        0
    );
}

#[test]
fn thread_read_reports_new_thread_as_idle_without_latest_turn() {
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
    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();

    assert_eq!(read.thread.id, thread.thread.id);
    assert_eq!(read.thread.status, ThreadStatus::Idle);
    assert_eq!(read.thread.active_turn, None);
    assert_eq!(read.thread.turns.last(), None);
    assert_eq!(read.thread.snapshot_path, thread.thread.snapshot_path);
    assert_eq!(read.thread.events_path, thread.thread.events_path);
}

#[test]
fn thread_read_prefers_loaded_runtime_view_over_disk_events() {
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
    exagent::transcript::append_json_line(
        &thread.thread.events_path,
        &RuntimeEvent {
            event_id: EventId::new("evt_disk_only"),
            session_id: thread.thread.id.clone(),
            turn_id: Some(TurnId::new("turn_disk_only")),
            kind: RuntimeEventKind::RuntimeError {
                message: "disk-only event after runtime load".into(),
            },
        },
    )
    .unwrap();

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();

    assert_eq!(read.thread.id, thread.thread.id);
    assert_eq!(read.thread.status, ThreadStatus::Idle);
    assert_eq!(read.thread.turns, vec![]);
}

#[test]
fn thread_resume_reads_persisted_thread_context_and_reports_ignored_cwd_override() {
    let dir = tempdir().unwrap();
    let original_cwd = dir.path().join("original-cwd");
    let ignored_cwd = dir.path().join("ignored-cwd");
    std::fs::create_dir_all(&original_cwd).unwrap();
    std::fs::create_dir_all(&ignored_cwd).unwrap();
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
            cwd: Some("original-cwd".into()),
        })
        .unwrap();
    let resumed = service
        .thread_resume(ThreadResumeParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            cwd: Some("ignored-cwd".into()),
        })
        .unwrap();

    assert_eq!(resumed.thread.id, thread.thread.id);
    assert_eq!(resumed.thread.status, ThreadStatus::Idle);
    assert_eq!(resumed.ignored_overrides, vec![IgnoredOverrideField::Cwd]);

    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert_ne!(snapshot.cwd, ignored_cwd);
}

#[tokio::test]
async fn legacy_run_resume_ignores_cwd_override_and_keeps_thread_snapshot_cwd() {
    let dir = tempdir().unwrap();
    let original_cwd = dir.path().join("original-cwd");
    let ignored_cwd = dir.path().join("ignored-cwd");
    std::fs::create_dir_all(&original_cwd).unwrap();
    std::fs::create_dir_all(&ignored_cwd).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("legacy resume complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
        })
        .unwrap();
    let output = service
        .run(RunParams {
            prompt: "resume through legacy run".into(),
            workspace_root: None,
            cwd: Some("ignored-cwd".into()),
            session_id: Some(thread.thread.id.clone()),
        })
        .await
        .unwrap();

    assert_eq!(output.text.as_deref(), Some("legacy resume complete"));
    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert_ne!(snapshot.cwd, std::fs::canonicalize(ignored_cwd).unwrap());
}

#[tokio::test]
async fn submit_boundary_op_dispatches_thread_read() {
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
    let response = service
        .submit_boundary_op(BoundaryOp::ThreadRead(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::ThreadRead(read) = response else {
        panic!("expected thread read response");
    };
    assert_eq!(read.thread.id, thread.thread.id);
    assert_eq!(read.thread.status, ThreadStatus::Idle);
}

#[tokio::test]
async fn submit_boundary_op_dispatches_thread_start() {
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

    let response = service
        .submit_boundary_op(BoundaryOp::ThreadStart(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::ThreadStarted(started) = response else {
        panic!("expected thread start response");
    };
    assert!(!started.thread.snapshot_path.exists());
    assert_rollout_jsonl_is_valid(&started.thread);
    assert!(started.thread.events_path.ends_with("events.jsonl"));
}

#[test]
fn queued_thread_op_serializes_user_input_without_runtime_dependencies() {
    let value = serde_json::to_value(exagent::app_server::protocol::QueuedThreadOp::UserInput {
        prompt: "hello".into(),
    })
    .unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "type": "user_input",
            "prompt": "hello"
        })
    );
}

#[test]
fn events_replay_rejects_missing_thread() {
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

    let err = service
        .events_replay(events_replay_params(SessionId::new("missing-thread")))
        .unwrap_err();

    assert!(err.to_string().contains("thread not found: missing-thread"));
}

#[tokio::test]
async fn submit_boundary_op_dispatches_turn_start() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("op turn complete".into()),
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
    let response = service
        .submit_boundary_op(BoundaryOp::TurnStart(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "continue through op".into(),
            workspace_root: None,
            turn_context: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::TurnStarted(turn) = response else {
        panic!("expected turn response");
    };
    assert_eq!(turn.thread_id, thread.thread.id);
    assert_eq!(turn.turn.status, TurnStatus::InProgress);
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;
}

#[tokio::test]
async fn submit_boundary_op_dispatches_events_replay_as_first_class_op() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("evented turn".into()),
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
            thread_id: thread.thread.id.clone(),
            prompt: "produce replayable events".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let response = service
        .submit_boundary_op(BoundaryOp::EventsReplay(events_replay_params(
            thread.thread.id.clone(),
        )))
        .await
        .unwrap();

    let BoundaryOpResponse::EventsReplayed(replay) = response else {
        panic!("expected events replay response");
    };
    assert_eq!(replay.thread_id, thread.thread.id);
    assert_eq!(replay.events.len(), 3);
}

#[tokio::test]
async fn events_replay_supports_after_event_id_and_limit_cursor_fields() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("cursor turn".into()),
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
            thread_id: thread.thread.id.clone(),
            prompt: "make cursor events".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id,
            workspace_root: None,
            after_event_id: Some(EventId::new("evt_1")),
            limit: Some(1),
            include_snapshot: false,
            event_kinds: vec![],
        })
        .unwrap();

    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].event_id, EventId::new("evt_2"));
}

#[tokio::test]
async fn failed_turn_start_records_runtime_error_for_replay() {
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
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "this will fail".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    assert_eq!(turn.turn.status, TurnStatus::InProgress);
    wait_for_runtime_error(&service, &thread.thread.id, &turn.turn.id).await;

    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    assert_eq!(replay.events.len(), 2);
    assert!(matches!(
        &replay.events[0].kind,
        RuntimeEventKind::TurnStarted
    ));
    assert!(matches!(
        &replay.events[1].kind,
        RuntimeEventKind::RuntimeError { message }
            if message.contains("MockLlm is out of scripted turns")
    ));
    assert_eq!(replay.events[0].turn_id, replay.events[1].turn_id);
}

#[tokio::test]
async fn thread_rejects_second_turn_while_first_turn_is_running() {
    let dir = tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(BlockingLlm {
            started: started.clone(),
            release: release.clone(),
        }),
        ToolRegistry::new,
    ));

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold the turn open".into(),
                workspace_root: None,
                turn_context: None,
            })
            .await
    });

    started.notified().await;

    let read_running = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read_running.thread.status, ThreadStatus::Running);
    assert_eq!(
        read_running
            .thread
            .active_turn
            .as_ref()
            .map(|turn| &turn.status),
        Some(&TurnStatus::InProgress)
    );

    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "must be rejected".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::ThreadBusy(thread_id)) if thread_id == &thread.thread.id
    ));

    release.notify_one();
    let started_turn = first_turn.await.unwrap().unwrap();
    assert_eq!(started_turn.turn.status, TurnStatus::InProgress);
    wait_for_turn_completed(&service, &thread.thread.id, &started_turn.turn.id).await;

    let read_idle = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id,
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read_idle.thread.status, ThreadStatus::Idle);
    assert_eq!(
        read_idle.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::Completed)
    );
}

#[tokio::test]
async fn rejected_concurrent_turn_context_does_not_mutate_thread_snapshot() {
    let dir = tempdir().unwrap();
    let original_cwd = dir.path().join("original-cwd");
    let rejected_cwd = dir.path().join("rejected-cwd");
    std::fs::create_dir_all(&original_cwd).unwrap();
    std::fs::create_dir_all(&rejected_cwd).unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(BlockingLlm {
            started: started.clone(),
            release: release.clone(),
        }),
        ToolRegistry::new,
    ));

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold the turn open".into(),
                workspace_root: None,
                turn_context: None,
            })
            .await
    });

    started.notified().await;

    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "must be rejected without context mutation".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some("rejected-cwd".into()),
            }),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::ThreadBusy(thread_id)) if thread_id == &thread.thread.id
    ));

    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert_ne!(snapshot.cwd, std::fs::canonicalize(rejected_cwd).unwrap());

    release.notify_one();
    let started_turn = first_turn.await.unwrap().unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &started_turn.turn.id).await;
}

#[tokio::test]
async fn turn_interrupt_aborts_active_turn_and_records_interrupted_event() {
    let dir = tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(BlockingLlm {
            started: started.clone(),
            release,
        }),
        ToolRegistry::new,
    ));

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold until interrupted".into(),
                workspace_root: None,
                turn_context: None,
            })
            .await
    });

    started.notified().await;
    let started_turn = first_turn.await.unwrap().unwrap();
    assert_eq!(started_turn.turn.status, TurnStatus::InProgress);
    let response = service
        .submit_boundary_op(BoundaryOp::TurnInterrupt(TurnInterruptParams {
            thread_id: thread.thread.id.clone(),
            turn_id: None,
            workspace_root: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::TurnInterrupted(interrupted) = response else {
        panic!("expected turn interrupted response");
    };
    assert_eq!(interrupted.thread_id, thread.thread.id);
    assert_eq!(
        interrupted
            .interrupted_turn
            .as_ref()
            .map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay_after_response = service
        .events_replay(events_replay_params(thread.thread.id.clone()))
        .unwrap();
    assert!(matches!(
        replay_after_response.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
    assert_rollout_jsonl_is_valid(&thread.thread);

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.thread.status, ThreadStatus::Idle);
    assert_eq!(
        read.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    assert!(matches!(
        replay.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
}

#[tokio::test]
async fn turn_interrupt_aborts_pre_turn_auto_compaction() {
    let dir = tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            auto_compact_token_limit: Some(1),
            ..AgentConfig::default()
        },
        Box::new(BlockingLlm {
            started: started.clone(),
            release,
        }),
        ToolRegistry::new,
    ));
    let thread_id = SessionId::new("thread_interrupt_pre_turn_compaction");
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::SessionMeta(
                exagent::state::rollout::SessionMeta {
                    thread_id: thread_id.clone(),
                    root_thread_id: thread_id.clone(),
                    parent_thread_id: None,
                    spawned_by_turn_id: None,
                    agent_role: AgentRole::Primary,
                    workspace_root: snapshot.workspace_root,
                    initial_cwd: snapshot.cwd,
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                },
            ),
            exagent::state::rollout::RolloutItem::ResponseItem(ConversationMessage::user(
                "old user",
            )),
            exagent::state::rollout::RolloutItem::ResponseItem(ConversationMessage::assistant(
                Some("old assistant".to_string()),
                vec![],
            )),
        ])
        .expect("write rollout");

    let first_service = service.clone();
    let first_thread_id = thread_id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "new user".into(),
                workspace_root: None,
                turn_context: None,
            })
            .await
    });

    started.notified().await;
    let started_turn = first_turn.await.unwrap().unwrap();
    let interrupted = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.turn_interrupt(TurnInterruptParams {
            thread_id: thread_id.clone(),
            turn_id: Some(started_turn.turn.id.clone()),
            workspace_root: None,
        }),
    )
    .await
    .expect("interrupt should not wait for compaction release")
    .expect("interrupt pre-turn compaction");

    assert_eq!(
        interrupted
            .interrupted_turn
            .as_ref()
            .map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );
    let replay = service
        .events_replay(events_replay_params(thread_id))
        .expect("replay events");
    assert!(matches!(
        replay.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
}

#[tokio::test]
async fn thread_read_reports_waiting_approval_when_runtime_overlay_has_pending_approval() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("try risky command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                }],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let _turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_thread_event(&service, &thread.thread.id, |kind| {
        matches!(kind, RuntimeEventKind::ApprovalRequested { .. })
    })
    .await;

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.thread.status, ThreadStatus::WaitingApproval);
    assert_eq!(
        read.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::Completed)
    );

    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    assert!(replay
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::ApprovalRequested { .. })));
}

#[test]
fn cold_thread_read_does_not_restore_historical_approval_as_waiting() {
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
    let thread_id = SessionId::new("thread_cold_historical_approval");
    let turn_id = TurnId::new("turn_1");
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::SessionMeta(
                exagent::state::rollout::SessionMeta {
                    thread_id: thread_id.clone(),
                    root_thread_id: thread_id.clone(),
                    parent_thread_id: None,
                    spawned_by_turn_id: None,
                    agent_role: AgentRole::Primary,
                    workspace_root: snapshot.workspace_root.clone(),
                    initial_cwd: snapshot.cwd.clone(),
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                },
            ),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                session_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_2"),
                session_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ApprovalRequested {
                    approval_id: ApprovalId::new("approval_1"),
                    tool_name: "run_command".to_string(),
                    reason: "approval required".to_string(),
                },
            }),
        ])
        .expect("write rollout");

    let read = service
        .thread_read(ThreadReadParams {
            thread_id,
            workspace_root: None,
        })
        .expect("read cold thread");

    assert_eq!(read.thread.status, ThreadStatus::Idle);
    assert_eq!(read.thread.active_turn, None);
    assert_ne!(
        read.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::WaitingApproval),
        "historical approval requests must not be projected as current live waiting state"
    );
}

#[test]
fn token_count_events_are_replayable_without_thread_view_items() {
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
    let thread_id = SessionId::new("thread_token_count_replay");
    let turn_id = TurnId::new("turn_1");
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let token_info = TokenUsageInfo {
        total_token_usage: TokenUsage {
            total_tokens: 100,
            ..TokenUsage::default()
        },
        last_token_usage: TokenUsage {
            total_tokens: 25,
            ..TokenUsage::default()
        },
        model_context_window: Some(1_000),
    };
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::SessionMeta(
                exagent::state::rollout::SessionMeta {
                    thread_id: thread_id.clone(),
                    root_thread_id: thread_id.clone(),
                    parent_thread_id: None,
                    spawned_by_turn_id: None,
                    agent_role: AgentRole::Primary,
                    workspace_root: snapshot.workspace_root,
                    initial_cwd: snapshot.cwd,
                    created_at: "2026-05-20T00:00:00Z".to_string(),
                },
            ),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                session_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_2"),
                session_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TokenCount {
                    info: Some(token_info.clone()),
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_3"),
                session_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            }),
        ])
        .expect("write rollout");

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread_id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![RuntimeEventKindFilter::TokenCount],
        })
        .expect("replay token count events");

    assert_eq!(replay.events.len(), 1);
    assert_eq!(
        replay.events[0].kind,
        RuntimeEventKind::TokenCount {
            info: Some(token_info)
        }
    );

    let read = service
        .thread_read(ThreadReadParams {
            thread_id,
            workspace_root: None,
        })
        .expect("read cold thread");

    assert_eq!(
        read.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::Completed)
    );
    assert!(read
        .thread
        .turns
        .last()
        .expect("turn view")
        .items
        .is_empty());
}

#[tokio::test]
async fn cold_rollout_thread_interrupt_rejects_instead_of_not_found() {
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
    let thread_id = SessionId::new("thread_cold_interrupt_rollout_only");
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[exagent::state::rollout::RolloutItem::SessionMeta(
            exagent::state::rollout::SessionMeta {
                thread_id: thread_id.clone(),
                root_thread_id: thread_id.clone(),
                parent_thread_id: None,
                spawned_by_turn_id: None,
                agent_role: AgentRole::Primary,
                workspace_root: snapshot.workspace_root,
                initial_cwd: snapshot.cwd,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            },
        )])
        .expect("write rollout");

    let err = service
        .turn_interrupt(TurnInterruptParams {
            thread_id: thread_id.clone(),
            turn_id: None,
            workspace_root: None,
        })
        .await
        .expect_err("cold rollout thread should not be interruptible");

    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::TurnRejected { thread_id: rejected, reason })
            if rejected == &thread_id && reason == "thread has no active turn"
    ));
}

#[tokio::test]
async fn turn_interrupt_clears_waiting_approval_and_records_interrupted_event() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("try risky command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                }],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let _turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_thread_event(&service, &thread.thread.id, |kind| {
        matches!(kind, RuntimeEventKind::ApprovalRequested { .. })
    })
    .await;
    let waiting = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(waiting.thread.status, ThreadStatus::WaitingApproval);
    let latest_turn_id = waiting
        .thread
        .turns
        .last()
        .map(|turn| turn.id.clone())
        .expect("waiting approval should have latest turn");

    let interrupted = service
        .turn_interrupt(TurnInterruptParams {
            thread_id: thread.thread.id.clone(),
            turn_id: Some(latest_turn_id.clone()),
            workspace_root: None,
        })
        .await
        .unwrap();
    assert_eq!(
        interrupted
            .interrupted_turn
            .as_ref()
            .map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );
    assert_eq!(
        interrupted
            .interrupted_turn
            .as_ref()
            .map(|turn| &turn.turn_id),
        Some(&latest_turn_id)
    );

    let snapshot = read_thread_snapshot(&thread.thread);
    assert!(snapshot.pending_approvals.is_empty());

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.thread.status, ThreadStatus::Idle);
    assert_eq!(
        read.thread.turns.last().map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    assert!(matches!(
        replay.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
}
