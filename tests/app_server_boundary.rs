use anyhow::Result;
use async_trait::async_trait;
use exagent::app_server::protocol::{
    BoundaryCapability, BoundaryOp, BoundaryOpResponse, EventsReplayParams, IgnoredOverrideField,
    InitializeParams, RunParams, ThreadReadParams, ThreadResumeParams, ThreadSpawnChildParams,
    ThreadStartParams, ThreadStatus, TurnContextOverrides, TurnInterruptParams, TurnStartParams,
    TurnStatus,
};
use exagent::app_server::{AppServerError, AppServerService};
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::{LlmClient, MockLlm};
use exagent::policy::PolicyMode;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::session::{AgentRole, ApprovalStatus, SessionSnapshot};
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::Tool;
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, SessionId, ToolCall, ToolResult, ToolStatus,
    TurnId,
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
    ) -> Result<AssistantTurn> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(AssistantTurn {
            text: Some("released turn".into()),
            tool_calls: vec![],
        })
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

fn assert_events_jsonl_is_valid(events_path: &std::path::Path) {
    let contents = std::fs::read_to_string(events_path).unwrap();
    for line in contents.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
}

#[tokio::test]
async fn initialize_boundary_advertises_v1_protocol_surface() {
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
        "appserver-runtime-boundary-v1"
    );
    assert_eq!(
        initialized.supported_ops,
        vec![
            BoundaryCapability::Initialize,
            BoundaryCapability::ThreadStart,
            BoundaryCapability::ThreadResume,
            BoundaryCapability::ThreadSpawnChild,
            BoundaryCapability::ThreadRead,
            BoundaryCapability::TurnStart,
            BoundaryCapability::TurnInterrupt,
            BoundaryCapability::EventsReplay,
        ]
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
            BoundaryCapability::ThreadSpawnChild,
            serde_json::to_value(BoundaryOp::ThreadSpawnChild(ThreadSpawnChildParams {
                parent_thread_id: SessionId::new("session_parent"),
                agent_role: AgentRole::Spec,
                prompt: "draft".into(),
                workspace_root: None,
                cwd: None,
                spawned_by_turn_id: None,
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
            turn_context: None,
        })
        .await
        .unwrap();

    assert_eq!(turn.thread_id, thread.thread_id);
    assert_eq!(turn.turn_id, TurnId::new("turn_1"));
    assert_eq!(turn.output.text.as_deref(), Some("thread turn complete"));

    let replay = service
        .events_replay(events_replay_params(thread.thread_id.clone()))
        .unwrap();
    assert_eq!(replay.thread_id, thread.thread_id);
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
    assert_eq!(replay.events[0].turn_id, Some(turn.turn_id.clone()));
    assert_eq!(replay.events[1].turn_id, Some(turn.turn_id.clone()));
    assert_eq!(replay.events[2].turn_id, Some(turn.turn_id));
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "run from new cwd".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
            }),
        })
        .await
        .unwrap();

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "probe cwd".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
            }),
        })
        .await
        .unwrap();

    assert_eq!(
        *observed_cwd.lock().unwrap(),
        Some(std::fs::canonicalize(turn_cwd).unwrap())
    );
    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
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
            thread_id: thread.thread_id.clone(),
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

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
    assert!(snapshot.conversation.is_empty());
    let replay = service
        .events_replay(events_replay_params(thread.thread_id))
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
    assert_eq!(child.ignored_overrides, vec![IgnoredOverrideField::Cwd]);

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
        .events_replay(events_replay_params(child.parent_thread_id.clone()))
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
        .events_replay(events_replay_params(SessionId::new(
            thread.thread_id.as_str(),
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
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: true,
            event_kinds: vec![],
        })
        .unwrap();

    let snapshot = replay.snapshot.expect("snapshot should be included");
    assert_eq!(snapshot.thread_id, thread.thread_id);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(dir.path()).unwrap());
    assert_eq!(snapshot.latest_compaction, None);
    assert_eq!(snapshot.open_exec_session_count, 0);
    assert_eq!(snapshot.conversation_message_count, 0);
    assert_eq!(snapshot.pending_approval_count, 0);
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
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();

    assert_eq!(read.thread_id, thread.thread_id);
    assert_eq!(read.status, ThreadStatus::Idle);
    assert_eq!(read.active_turn, None);
    assert_eq!(read.latest_turn, None);
    assert_eq!(read.snapshot_path, thread.snapshot_path);
    assert_eq!(read.events_path, thread.events_path);
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
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
            cwd: Some("ignored-cwd".into()),
        })
        .unwrap();

    assert_eq!(resumed.thread.thread_id, thread.thread_id);
    assert_eq!(resumed.thread.status, ThreadStatus::Idle);
    assert_eq!(resumed.ignored_overrides, vec![IgnoredOverrideField::Cwd]);

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
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
            session_id: Some(thread.thread_id.clone()),
        })
        .await
        .unwrap();

    assert_eq!(output.text.as_deref(), Some("legacy resume complete"));
    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
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
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::ThreadRead(read) = response else {
        panic!("expected thread read response");
    };
    assert_eq!(read.thread_id, thread.thread_id);
    assert_eq!(read.status, ThreadStatus::Idle);
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
    assert!(started.snapshot_path.exists());
    assert!(started.events_path.ends_with("events.jsonl"));
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
            thread_id: thread.thread_id.clone(),
            prompt: "continue through op".into(),
            workspace_root: None,
            turn_context: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::TurnStarted(turn) = response else {
        panic!("expected turn response");
    };
    assert_eq!(turn.thread_id, thread.thread_id);
    assert_eq!(turn.output.text.as_deref(), Some("op turn complete"));
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "produce replayable events".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let response = service
        .submit_boundary_op(BoundaryOp::EventsReplay(events_replay_params(
            thread.thread_id.clone(),
        )))
        .await
        .unwrap();

    let BoundaryOpResponse::EventsReplayed(replay) = response else {
        panic!("expected events replay response");
    };
    assert_eq!(replay.thread_id, thread.thread_id);
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "make cursor events".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread_id,
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
    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "this will fail".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("MockLlm is out of scripted turns"));

    let replay = service
        .events_replay(events_replay_params(thread.thread_id))
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
    let first_thread_id = thread.thread_id.clone();
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
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read_running.status, ThreadStatus::Running);
    assert_eq!(
        read_running.active_turn.as_ref().map(|turn| &turn.status),
        Some(&TurnStatus::Running)
    );

    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "must be rejected".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::ThreadBusy(thread_id)) if thread_id == &thread.thread_id
    ));

    release.notify_one();
    let completed = first_turn.await.unwrap().unwrap();
    assert_eq!(completed.output.text.as_deref(), Some("released turn"));

    let read_idle = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread_id,
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read_idle.status, ThreadStatus::Idle);
    assert_eq!(
        read_idle.latest_turn.as_ref().map(|turn| &turn.status),
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
    let first_thread_id = thread.thread_id.clone();
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
            thread_id: thread.thread_id.clone(),
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
        Some(AppServerError::ThreadBusy(thread_id)) if thread_id == &thread.thread_id
    ));

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert_ne!(snapshot.cwd, std::fs::canonicalize(rejected_cwd).unwrap());

    release.notify_one();
    first_turn.await.unwrap().unwrap();
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
    let first_thread_id = thread.thread_id.clone();
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
    let response = service
        .submit_boundary_op(BoundaryOp::TurnInterrupt(TurnInterruptParams {
            thread_id: thread.thread_id.clone(),
            turn_id: None,
            workspace_root: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::TurnInterrupted(interrupted) = response else {
        panic!("expected turn interrupted response");
    };
    assert_eq!(interrupted.thread_id, thread.thread_id);
    assert_eq!(
        interrupted
            .interrupted_turn
            .as_ref()
            .map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay_after_response = service
        .events_replay(events_replay_params(thread.thread_id.clone()))
        .unwrap();
    assert!(matches!(
        replay_after_response.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
    assert_events_jsonl_is_valid(thread.events_path.as_ref());

    let err = first_turn.await.unwrap().unwrap_err();
    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::TurnInterrupted { thread_id, turn_id })
            if thread_id == &thread.thread_id && turn_id == &TurnId::new("turn_1")
    ));

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.status, ThreadStatus::Idle);
    assert_eq!(
        read.latest_turn.as_ref().map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay = service
        .events_replay(events_replay_params(thread.thread_id))
        .unwrap();
    assert!(matches!(
        replay.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
}

#[tokio::test]
async fn thread_read_reports_waiting_approval_when_snapshot_has_pending_approval() {
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "request approval".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.status, ThreadStatus::WaitingApproval);
    assert_eq!(
        read.latest_turn.as_ref().map(|turn| &turn.status),
        Some(&TurnStatus::Completed)
    );

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
    assert!(snapshot
        .pending_approvals
        .iter()
        .any(|approval| approval.status == ApprovalStatus::Pending));

    let replay = service
        .events_replay(events_replay_params(thread.thread_id))
        .unwrap();
    assert!(replay
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::ApprovalRequested { .. })));
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
    service
        .turn_start(TurnStartParams {
            thread_id: thread.thread_id.clone(),
            prompt: "request approval".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();
    let waiting = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(waiting.status, ThreadStatus::WaitingApproval);
    let latest_turn_id = waiting
        .latest_turn
        .as_ref()
        .map(|turn| turn.turn_id.clone())
        .expect("waiting approval should have latest turn");

    let interrupted = service
        .turn_interrupt(TurnInterruptParams {
            thread_id: thread.thread_id.clone(),
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

    let snapshot: SessionSnapshot =
        exagent::transcript::read_json(thread.snapshot_path.as_ref()).unwrap();
    assert!(snapshot.pending_approvals.is_empty());

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread_id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_eq!(read.status, ThreadStatus::Idle);
    assert_eq!(
        read.latest_turn.as_ref().map(|turn| &turn.status),
        Some(&TurnStatus::Interrupted)
    );

    let replay = service
        .events_replay(events_replay_params(thread.thread_id))
        .unwrap();
    assert!(matches!(
        replay.events.last().map(|event| &event.kind),
        Some(RuntimeEventKind::TurnInterrupted)
    ));
}
