use anyhow::Result;
use async_trait::async_trait;
use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionStatus, BoundaryCapability, BoundaryOp,
    BoundaryOpResponse, EventsReplayParams, IgnoredOverrideField, InitializeParams, RunParams,
    RuntimeEventKindFilter, ThreadItem, ThreadReadParams, ThreadResumeParams, ThreadStartParams,
    ThreadStatus, ThreadView, TurnContextOverrides, TurnInterruptParams, TurnStartParams,
    TurnStatus,
};
use exagent::app_server::{AppServerError, AppServerService};
use exagent::config::{AgentConfig, PermissionProfile, ThinkingMode};
use exagent::events::{ExecOutputStream, RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, LlmRequestOptions, MockLlm};
use exagent::policy::PolicyMode;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::resolved::{ModelRef, ResolvedCredential, ResolvedModelConfig};
use exagent::session::{ApprovalId, ThreadSnapshot};
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, LlmCompletion, ReasoningBlock, ReasoningSignature,
    ThreadId, TokenUsage, TokenUsageInfo, ToolCall, ToolResult, ToolStatus, TurnId,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::sync::Notify;

struct BlockingLlm {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

struct RecordingOptionsLlm {
    observed_thinking_modes: Arc<Mutex<Vec<Option<ThinkingMode>>>>,
}

#[derive(Clone)]
struct CwdProbeTool {
    observed_cwd: Arc<Mutex<Option<PathBuf>>>,
}

#[async_trait]
impl ToolHandler for CwdProbeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "cwd_probe",
            "Record the active tool cwd",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        *self.observed_cwd.lock().unwrap() = Some(ctx.config.cwd.clone());
        ToolOutcome::from_result(ToolResult {
            tool_call_id: call.id,
            tool_name: call.name,
            status: ToolStatus::Success,
            content: ctx.config.cwd.display().to_string(),
            meta: None,
        })
    }
}

#[async_trait]
impl LlmClient for BlockingLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(AssistantTurn {
            text: Some("released turn".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

#[async_trait]
impl LlmClient for RecordingOptionsLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.observed_thinking_modes
            .lock()
            .unwrap()
            .push(options.thinking_mode);
        Ok(AssistantTurn {
            text: Some("recorded options".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

fn events_replay_params(thread_id: ThreadId) -> EventsReplayParams {
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

fn read_thread_snapshot(workspace_root: &std::path::Path, thread: &ThreadView) -> ThreadSnapshot {
    let workspace_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace_root, &thread.id);
    let items =
        exagent::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
            .unwrap();
    exagent::state::rollout::snapshot_from_rollout_items(&thread.id, &items).unwrap()
}

fn assert_rollout_jsonl_is_valid(workspace_root: &std::path::Path, thread: &ThreadView) {
    let workspace_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace_root, &thread.id);
    let contents = std::fs::read_to_string(rollout_paths.rollout_path).unwrap();
    for line in contents.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
}

async fn wait_for_turn_event(
    service: &AppServerService,
    thread_id: &ThreadId,
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
    thread_id: &ThreadId,
    turn_id: &TurnId,
) {
    wait_for_turn_event(service, thread_id, turn_id, |kind| {
        matches!(kind, RuntimeEventKind::TurnCompleted)
    })
    .await;
}

async fn wait_for_runtime_error(
    service: &AppServerService,
    thread_id: &ThreadId,
    turn_id: &TurnId,
) {
    wait_for_turn_event(service, thread_id, turn_id, |kind| {
        matches!(kind, RuntimeEventKind::RuntimeError { .. })
    })
    .await;
}

async fn wait_for_thread_event(
    service: &AppServerService,
    thread_id: &ThreadId,
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
            BoundaryCapability::ThreadGoal,
            BoundaryCapability::AgentTree,
            BoundaryCapability::TurnStart,
            BoundaryCapability::TurnInterrupt,
            BoundaryCapability::ApprovalDecision,
            BoundaryCapability::EventsReplay,
        ]
    );
    assert_eq!(
        initialized.supported_streams,
        vec![BoundaryCapability::EventsSubscribe]
    );
    assert_eq!(
        initialized.supported_permission_profiles,
        vec![PermissionProfile::FullAccess]
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
                permission_profile: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::ThreadResume,
            serde_json::to_value(BoundaryOp::ThreadResume(ThreadResumeParams {
                thread_id: ThreadId::new("session_123"),
                workspace_root: None,
                cwd: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::ThreadRead,
            serde_json::to_value(BoundaryOp::ThreadRead(ThreadReadParams {
                thread_id: ThreadId::new("session_123"),
                workspace_root: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::TurnStart,
            serde_json::to_value(BoundaryOp::TurnStart(TurnStartParams {
                thread_id: ThreadId::new("session_123"),
                prompt: "continue".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::TurnInterrupt,
            serde_json::to_value(BoundaryOp::TurnInterrupt(TurnInterruptParams {
                thread_id: ThreadId::new("session_123"),
                turn_id: None,
                workspace_root: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::ApprovalDecision,
            serde_json::to_value(BoundaryOp::ApprovalDecision(ApprovalDecisionParams {
                thread_id: ThreadId::new("session_123"),
                turn_id: None,
                approval_id: ApprovalId::new("approval_123"),
                decision: ApprovalDecisionStatus::Denied,
                note: None,
                workspace_root: None,
            }))
            .unwrap(),
        ),
        (
            BoundaryCapability::EventsReplay,
            serde_json::to_value(BoundaryOp::EventsReplay(events_replay_params(
                ThreadId::new("session_123"),
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
            permission_profile: None,
        })
        .unwrap();

    let snapshot = read_thread_snapshot(dir.path(), &response.thread);
    assert_eq!(snapshot.thread_id, response.thread.id);
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
            permission_profile: None,
        })
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));
}

#[tokio::test]
async fn thread_start_rejects_managed_permission_profile_until_supported() {
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
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: Some(PermissionProfile::Managed),
        })
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("unsupported permission profile: managed"));
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "continue work".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
async fn events_replay_redacts_assistant_reasoning_metadata() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("visible answer".into()),
            tool_calls: vec![ToolCall {
                id: "call_visible".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
                thought_signature: Some(serde_json::json!("hidden-tool-signature")),
            }],
            reasoning: vec![ReasoningBlock {
                text: "hidden reasoning".into(),
                signature: Some(ReasoningSignature::OpenAiField {
                    field: "reasoning_content".into(),
                }),
                redacted: false,
            }],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "produce private metadata".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_event(&service, &thread.thread.id, &turn.turn.id, |kind| {
        matches!(kind, RuntimeEventKind::AssistantTurn { .. })
    })
    .await;

    let replay = service
        .events_replay(events_replay_params(thread.thread.id))
        .unwrap();
    let assistant_turn = replay
        .events
        .iter()
        .find_map(|event| match &event.kind {
            RuntimeEventKind::AssistantTurn { turn } => Some(turn),
            _ => None,
        })
        .expect("assistant turn event");

    assert_eq!(assistant_turn.text.as_deref(), Some("visible answer"));
    assert!(assistant_turn.reasoning.is_empty());
    assert_eq!(assistant_turn.tool_calls.len(), 1);
    assert_eq!(assistant_turn.tool_calls[0].id, "call_visible");
    assert_eq!(assistant_turn.tool_calls[0].name, "read_file");
    assert_eq!(
        assistant_turn.tool_calls[0].arguments,
        serde_json::json!({"path": "Cargo.toml"})
    );
    assert_eq!(assistant_turn.tool_calls[0].thought_signature, None);
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("tool done".into()),
                tool_calls: vec![],
                reasoning: vec![],
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
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run a tool".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let replay = service
        .events_replay(events_replay_params(thread.thread.id.clone()))
        .unwrap();
    assert!(replay.events.len() >= 5);
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
    assert_eq!(view.items.len(), 5);
    assert!(matches!(
        &view.items[0],
        ThreadItem::UserMessage { text, .. } if text == "run a tool"
    ));
    assert!(matches!(
        &view.items[1],
        ThreadItem::AssistantMessage { text, .. } if text.as_deref() == Some("need tool")
    ));
    assert!(matches!(
        &view.items[2],
        ThreadItem::ToolInvocation {
            invocation_id,
            tool_call_id,
            tool_name,
            status,
            mutating,
            reason,
            message,
            ..
        } if invocation_id == "inv_call_cwd_probe"
            && tool_call_id.as_deref() == Some("call_cwd_probe")
            && tool_name.as_deref() == Some("cwd_probe")
            && status == "completed"
            && *mutating == Some(false)
            && reason.is_none()
            && message.is_none()
    ));
    assert!(matches!(
        &view.items[3],
        ThreadItem::ToolResult { name, .. } if name == "cwd_probe"
    ));
    assert!(matches!(
        &view.items[4],
        ThreadItem::AssistantMessage { text, .. } if text.as_deref() == Some("tool done")
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
async fn events_subscribe_redacts_live_assistant_reasoning_metadata() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("live visible answer".into()),
            tool_calls: vec![ToolCall {
                id: "call_live_visible".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "README.md"}),
                thought_signature: Some(serde_json::json!("hidden-live-tool-signature")),
            }],
            reasoning: vec![ReasoningBlock {
                text: "hidden live reasoning".into(),
                signature: Some(ReasoningSignature::GeminiThoughtSignature(
                    "hidden-live-reasoning-signature".into(),
                )),
                redacted: false,
            }],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            prompt: "stream private metadata".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    for _ in 0..20 {
        let event = events.recv().await.unwrap();
        if event.turn_id.as_ref() != Some(&turn.turn.id) {
            continue;
        }
        let RuntimeEventKind::AssistantTurn { turn } = event.kind else {
            continue;
        };

        assert_eq!(turn.text.as_deref(), Some("live visible answer"));
        assert!(turn.reasoning.is_empty());
        assert_eq!(turn.tool_calls[0].id, "call_live_visible");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(
            turn.tool_calls[0].arguments,
            serde_json::json!({"path": "README.md"})
        );
        assert_eq!(turn.tool_calls[0].thought_signature, None);
        return;
    }

    panic!("timed out waiting for assistant turn event");
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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

    let approval_event_payload =
        serde_json::to_value(&approval_event.kind).expect("serialize approval event kind");
    assert_eq!(approval_event_payload["permission_profile"], "full_access");
    assert_eq!(approval_event_payload["filesystem_sandbox"], "none");
    assert_eq!(approval_event_payload["network_sandbox"], "none");
    assert_eq!(approval_event_payload["env_isolation"], "none");
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
    let approval_item = read
        .thread
        .turns
        .iter()
        .flat_map(|turn| turn.items.iter())
        .find(|item| matches!(item, ThreadItem::ApprovalRequested { .. }))
        .expect("thread read projects approval request item");
    let approval_item_payload =
        serde_json::to_value(approval_item).expect("serialize approval thread item");
    assert_eq!(approval_item_payload["permission_profile"], "full_access");
    assert_eq!(approval_item_payload["filesystem_sandbox"], "none");
    assert_eq!(approval_item_payload["network_sandbox"], "none");
    assert_eq!(approval_item_payload["env_isolation"], "none");
}

#[tokio::test]
async fn events_subscribe_receives_live_exec_output_without_persisting_it() {
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
                    id: "call_live_exec_output".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "sleep 0.2; printf 'live-delta\\n'",
                        "persistent": true
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("command started".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            prompt: "run persistent command".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let output_event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("live event channel open");
            if matches!(event.kind, RuntimeEventKind::ExecOutput { .. }) {
                return event;
            }
        }
    })
    .await
    .expect("exec output must be delivered through live subscribe");

    assert_eq!(output_event.turn_id.as_ref(), Some(&turn.turn.id));
    match &output_event.kind {
        RuntimeEventKind::ExecOutput {
            stream,
            chunk,
            sequence,
            ..
        } => {
            assert_eq!(*stream, ExecOutputStream::Stdout);
            assert!(chunk.contains("live-delta"));
            assert!(*sequence > 0);
        }
        other => panic!("expected exec output event, got {other:?}"),
    }

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![RuntimeEventKindFilter::ExecOutput],
        })
        .unwrap();
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ExecOutput { chunk, .. } if chunk.contains("live-delta")
    )));
    let invocation_replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![RuntimeEventKindFilter::ToolInvocationOutputDelta],
        })
        .unwrap();
    assert!(invocation_replay.events.is_empty());

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert!(read
        .thread
        .turns
        .iter()
        .flat_map(|turn| turn.items.iter())
        .any(
            |item| matches!(item, ThreadItem::ExecOutput { text, .. } if text.contains("live-delta"))
        ));

    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread.thread.id);
    let rollout_items =
        exagent::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
            .unwrap();
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
            kind: RuntimeEventKind::ExecOutput { .. },
            ..
        })
    )));
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
            kind: RuntimeEventKind::ToolInvocationOutputDelta { .. },
            ..
        })
    )));
}

#[tokio::test]
async fn events_subscribe_receives_tool_invocation_lifecycle_events() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("run command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_lifecycle".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "printf 'done'" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("finished".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            prompt: "run lifecycle command".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    let mut saw_started = false;
    let mut saw_completed = false;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !saw_started || !saw_completed {
            let event = events.recv().await.expect("live event channel open");
            match event.kind {
                RuntimeEventKind::ToolInvocationStarted {
                    invocation_id,
                    tool_call_id,
                    tool_name,
                    mutating,
                } => {
                    assert_eq!(event.turn_id.as_ref(), Some(&turn.turn.id));
                    assert_eq!(invocation_id, "inv_call_lifecycle");
                    assert_eq!(tool_call_id, "call_lifecycle");
                    assert_eq!(tool_name, "run_command");
                    assert!(mutating);
                    saw_started = true;
                }
                RuntimeEventKind::ToolInvocationCompleted {
                    invocation_id,
                    tool_call_id,
                    tool_name,
                    status,
                } => {
                    assert_eq!(event.turn_id.as_ref(), Some(&turn.turn.id));
                    assert_eq!(invocation_id, "inv_call_lifecycle");
                    assert_eq!(tool_call_id, "call_lifecycle");
                    assert_eq!(tool_name, "run_command");
                    assert_eq!(status, ToolStatus::Success);
                    saw_completed = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("tool lifecycle events must be delivered through live subscribe");

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![
                RuntimeEventKindFilter::ToolInvocationStarted,
                RuntimeEventKindFilter::ToolInvocationCompleted,
            ],
        })
        .unwrap();
    assert_eq!(replay.events.len(), 2);
}

#[tokio::test]
async fn events_subscribe_receives_tool_invocation_waiting_approval() {
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
                    id: "call_waiting_approval_lifecycle".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            prompt: "request approval lifecycle".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    let waiting_event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("live event channel open");
            if matches!(
                event.kind,
                RuntimeEventKind::ToolInvocationWaitingApproval { .. }
            ) {
                return event;
            }
        }
    })
    .await
    .expect("waiting approval lifecycle event must be delivered");

    match &waiting_event.kind {
        RuntimeEventKind::ToolInvocationWaitingApproval {
            invocation_id,
            approval_id,
            reason,
        } => {
            assert_eq!(invocation_id, "inv_call_waiting_approval_lifecycle");
            assert!(approval_id.as_str().starts_with("approval_"));
            assert!(!reason.is_empty());
        }
        other => panic!("expected waiting approval lifecycle event, got {other:?}"),
    }

    let replay = service
        .events_replay(EventsReplayParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
            after_event_id: None,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![RuntimeEventKindFilter::ToolInvocationWaitingApproval],
        })
        .unwrap();
    assert_eq!(replay.events.len(), 1);
}

#[tokio::test]
async fn thread_read_updates_waiting_tool_invocation_after_approval_decision() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("try risky command".into()),
            tool_calls: vec![ToolCall {
                id: "call_waiting_approval_read".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                thought_signature: None,
            }],
            reasoning: vec![],
        }])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval lifecycle".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    let approval_id = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let replay = service
                .events_replay(events_replay_params(thread.thread.id.clone()))
                .unwrap();
            if let Some(id) = replay.events.iter().find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested { approval_id, .. } => {
                    Some(approval_id.clone())
                }
                _ => None,
            }) {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("approval request must be recorded");

    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.thread.id.clone(),
            turn_id: Some(turn.turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    let invocation_status = read
        .thread
        .turns
        .iter()
        .flat_map(|turn| turn.items.iter())
        .find_map(|item| match item {
            ThreadItem::ToolInvocation {
                invocation_id,
                status,
                ..
            } if invocation_id == "inv_call_waiting_approval_read" => Some(status),
            _ => None,
        })
        .expect("thread read projects original tool invocation");

    assert_eq!(invocation_status, "denied");
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
            thread_id: ThreadId::new("missing-thread"),
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run from new cwd".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
                model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("cwd probed".into()),
                tool_calls: vec![],
                reasoning: vec![],
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
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "probe cwd".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
                model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    assert_eq!(
        *observed_cwd.lock().unwrap(),
        Some(std::fs::canonicalize(turn_cwd).unwrap())
    );
    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
}

#[tokio::test]
async fn turn_context_thinking_mode_reaches_llm_without_mutating_later_turns() {
    let dir = tempdir().unwrap();
    let observed_thinking_modes = Arc::new(Mutex::new(Vec::new()));
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            thinking_mode: Some(ThinkingMode::Low),
            ..AgentConfig::default()
        },
        Box::new(RecordingOptionsLlm {
            observed_thinking_modes: observed_thinking_modes.clone(),
        }),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let first = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run with high thinking".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: None,
                model: None,
                thinking_mode: Some(ThinkingMode::High),
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &first.turn.id).await;

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
    assert_eq!(
        snapshot
            .reference_turn_context
            .as_ref()
            .and_then(|context| context.thinking_mode),
        Some(ThinkingMode::High)
    );

    let second = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run with default thinking".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &second.turn.id).await;

    assert_eq!(
        observed_thinking_modes.lock().unwrap().as_slice(),
        [Some(ThinkingMode::High), Some(ThinkingMode::Low)]
    );
}

#[tokio::test]
async fn turn_context_clear_thinking_mode_suppresses_default_for_one_turn() {
    let dir = tempdir().unwrap();
    let observed_thinking_modes = Arc::new(Mutex::new(Vec::new()));
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            thinking_mode: Some(ThinkingMode::High),
            ..AgentConfig::default()
        },
        Box::new(RecordingOptionsLlm {
            observed_thinking_modes: observed_thinking_modes.clone(),
        }),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let clear_params: TurnStartParams = serde_json::from_value(serde_json::json!({
        "thread_id": thread.thread.id,
        "prompt": "run without thinking default",
        "workspace_root": null,
        "turn_context": {
            "clear_thinking_mode": true
        }
    }))
    .unwrap();
    let first = service.turn_start(clear_params).await.unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &first.turn.id).await;
    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
    assert_eq!(
        snapshot
            .reference_turn_context
            .as_ref()
            .and_then(|context| context.thinking_mode),
        None
    );

    let second = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run with default thinking".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &second.turn.id).await;
    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
    assert_eq!(
        snapshot
            .reference_turn_context
            .as_ref()
            .and_then(|context| context.thinking_mode),
        Some(ThinkingMode::High)
    );

    assert_eq!(
        observed_thinking_modes.lock().unwrap().as_slice(),
        [None, Some(ThinkingMode::High)]
    );
}

#[tokio::test]
async fn turn_context_model_reaches_turn_context_without_mutating_later_turns() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let service = AppServerService::with_llm(
        AgentConfig {
            model: ResolvedModelConfig::from_provider_profile(
                "openai",
                "base-model",
                None,
                ResolvedCredential::None,
                None,
            ),
            workspace_root: workspace.clone(),
            cwd: workspace.clone(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("first done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("second done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: Some(workspace.display().to_string()),
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let first = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run with override model".into(),
            input: vec![],
            workspace_root: Some(workspace.display().to_string()),
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: None,
                model: Some(ModelRef::new("openai", "override-model")),
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &first.turn.id).await;

    let second = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "run with default model".into(),
            input: vec![],
            workspace_root: Some(workspace.display().to_string()),
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &second.turn.id).await;

    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace, &thread.thread.id);
    let contexts: Vec<_> =
        exagent::state::rollout::RolloutStore::read_items_blocking(&rollout_paths.rollout_path)
            .unwrap()
            .into_iter()
            .filter_map(|item| match item {
                exagent::state::rollout::RolloutItem::TurnContext(context) => Some(context.model),
                _ => None,
            })
            .collect();

    assert_eq!(
        contexts,
        vec![
            ModelRef::new("openai", "override-model"),
            ModelRef::new("openai", "base-model")
        ]
    );
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "must not be accepted".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: Some(outside.path().to_string_lossy().to_string()),
                model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("cwd must stay within workspace_root"));

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let output = service
        .run(RunParams {
            prompt: "run through compatibility wrapper".into(),
            workspace_root: None,
            cwd: None,
            thread_id: None,
            thinking_mode: None,
            permission_profile: None,
        })
        .await
        .unwrap();

    let replay = service
        .events_replay(events_replay_params(output.thread_id))
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
            permission_profile: None,
        })
        .unwrap();
    let replay = service
        .events_replay(events_replay_params(ThreadId::new(
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
            permission_profile: None,
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

#[test]
fn replay_snapshot_view_deserializes_missing_permission_profile_as_full_access() {
    let snapshot: exagent::app_server::protocol::ReplaySnapshotView =
        serde_json::from_value(serde_json::json!({
            "thread_id": "thread_old_replay_profile",
            "cwd": "/tmp",
            "open_exec_session_count": 0,
            "conversation_message_count": 0,
            "pending_approval_count": 0
        }))
        .expect("deserialize replay snapshot view");

    assert_eq!(snapshot.permission_profile, PermissionProfile::FullAccess);
}

#[tokio::test]
async fn events_replay_snapshot_includes_latest_compaction_after_auto_compact() {
    let dir = tempdir().unwrap();
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        auto_compact_token_limit: Some(1),
        ..AgentConfig::default()
    };
    config.model.capabilities.context_window = Some(1_000);
    let service = AppServerService::with_llm(
        config,
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("first done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("summary after first".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("second done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let first_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "first prompt".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &first_turn.turn.id).await;
    let second_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "second prompt".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("persistent command started".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "open persistent command".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
            permission_profile: None,
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
            permission_profile: None,
        })
        .unwrap();
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread.thread.id);
    exagent::transcript::append_json_line(
        &rollout_paths.rollout_path,
        &exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_disk_only"),
            thread_id: thread.thread.id.clone(),
            turn_id: Some(TurnId::new("turn_disk_only")),
            kind: RuntimeEventKind::RuntimeError {
                message: "disk-only event after runtime load".into(),
            },
        }),
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
            permission_profile: None,
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

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("original-cwd".into()),
            permission_profile: None,
        })
        .unwrap();
    let output = service
        .run(RunParams {
            prompt: "resume through legacy run".into(),
            workspace_root: None,
            cwd: Some("ignored-cwd".into()),
            thread_id: Some(thread.thread.id.clone()),
            thinking_mode: None,
            permission_profile: None,
        })
        .await
        .unwrap();

    assert_eq!(output.text.as_deref(), Some("legacy resume complete"));
    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
    assert_eq!(snapshot.cwd, std::fs::canonicalize(original_cwd).unwrap());
    assert_ne!(snapshot.cwd, std::fs::canonicalize(ignored_cwd).unwrap());
}

#[tokio::test]
async fn legacy_run_resume_rejects_unsupported_permission_profile() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("must not run".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();

    let err = service
        .run(RunParams {
            prompt: "resume with unsupported profile".into(),
            workspace_root: None,
            cwd: None,
            thread_id: Some(thread.thread.id),
            thinking_mode: None,
            permission_profile: Some(PermissionProfile::Managed),
        })
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("unsupported permission profile: managed"));
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
            permission_profile: None,
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
            permission_profile: None,
        }))
        .await
        .unwrap();

    let BoundaryOpResponse::ThreadStarted(started) = response else {
        panic!("expected thread start response");
    };
    assert_rollout_jsonl_is_valid(dir.path(), &started.thread);
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
        .events_replay(events_replay_params(ThreadId::new("missing-thread")))
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let response = service
        .submit_boundary_op(BoundaryOp::TurnStart(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "continue through op".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
            tool_calls: vec![ToolCall {
                id: "call_evented".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "src/lib.rs"}),
                thought_signature: Some(serde_json::json!("hidden-boundary-op-tool-signature")),
            }],
            reasoning: vec![ReasoningBlock {
                text: "hidden boundary op reasoning".into(),
                signature: Some(ReasoningSignature::AnthropicSignature(
                    "hidden-boundary-op-signature".into(),
                )),
                redacted: false,
            }],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "produce replayable events".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();
    wait_for_turn_event(&service, &thread.thread.id, &turn.turn.id, |kind| {
        matches!(kind, RuntimeEventKind::AssistantTurn { .. })
    })
    .await;

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
    assert!(replay.events.len() >= 3);
    let assistant_turn = replay
        .events
        .iter()
        .find_map(|event| match &event.kind {
            RuntimeEventKind::AssistantTurn { turn } => Some(turn),
            _ => None,
        })
        .expect("assistant turn event");
    assert_eq!(assistant_turn.text.as_deref(), Some("evented turn"));
    assert!(assistant_turn.reasoning.is_empty());
    assert_eq!(assistant_turn.tool_calls[0].id, "call_evented");
    assert_eq!(assistant_turn.tool_calls[0].name, "read_file");
    assert_eq!(
        assistant_turn.tool_calls[0].arguments,
        serde_json::json!({"path": "src/lib.rs"})
    );
    assert_eq!(assistant_turn.tool_calls[0].thought_signature, None);
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
            reasoning: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "make cursor events".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
            permission_profile: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "this will fail".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
            permission_profile: None,
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold the turn open".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
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
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
            permission_profile: None,
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold the turn open".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
    });

    started.notified().await;

    let err = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "must be rejected without context mutation".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: Some("rejected-cwd".into()),
                model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<AppServerError>(),
        Some(AppServerError::ThreadBusy(thread_id)) if thread_id == &thread.thread.id
    ));

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
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
            permission_profile: None,
        })
        .unwrap();

    let first_service = service.clone();
    let first_thread_id = thread.thread.id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "hold until interrupted".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
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
    assert_rollout_jsonl_is_valid(dir.path(), &thread.thread);

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
async fn turn_interrupt_records_active_tool_invocation_cancelled() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("run long command".into()),
            tool_calls: vec![ToolCall {
                id: "call_interrupt_tool".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "sleep 30" }),
                thought_signature: None,
            }],
            reasoning: vec![],
        }])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
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
            prompt: "interrupt active tool".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("live event channel open");
            if matches!(
                event.kind,
                RuntimeEventKind::ToolInvocationStarted {
                    ref tool_call_id,
                    ..
                } if tool_call_id == "call_interrupt_tool"
            ) {
                return;
            }
        }
    })
    .await
    .expect("tool invocation must start before interrupt");

    let interrupted = service
        .turn_interrupt(TurnInterruptParams {
            thread_id: thread.thread.id.clone(),
            turn_id: Some(turn.turn.id.clone()),
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

    let replay = service
        .events_replay(events_replay_params(thread.thread.id.clone()))
        .unwrap();
    let cancelled_index = replay
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ToolInvocationCancelled {
                    invocation_id,
                    tool_call_id,
                    tool_name,
                    reason,
                } if invocation_id == "inv_call_interrupt_tool"
                    && tool_call_id == "call_interrupt_tool"
                    && tool_name == "run_command"
                    && reason == "interrupted"
            )
        })
        .expect("active tool invocation should record cancellation");
    let interrupted_index = replay
        .events
        .iter()
        .position(|event| matches!(event.kind, RuntimeEventKind::TurnInterrupted))
        .expect("turn should record interruption");
    assert!(cancelled_index < interrupted_index);
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
    let thread_id = ThreadId::new("thread_interrupt_pre_turn_compaction");
    let snapshot = ThreadSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::ThreadMeta(exagent::state::rollout::ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: snapshot.workspace_root,
                initial_cwd: snapshot.cwd,
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            exagent::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("old user"),
            ),
            exagent::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ])
        .expect("write rollout");

    let first_service = service.clone();
    let first_thread_id = thread_id.clone();
    let first_turn = tokio::spawn(async move {
        first_service
            .turn_start(TurnStartParams {
                thread_id: first_thread_id,
                prompt: "new user".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let _turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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
    let thread_id = ThreadId::new("thread_cold_historical_approval");
    let turn_id = TurnId::new("turn_1");
    let snapshot = ThreadSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::ThreadMeta(exagent::state::rollout::ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: snapshot.workspace_root.clone(),
                initial_cwd: snapshot.cwd.clone(),
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ApprovalRequested {
                    approval_id: ApprovalId::new("approval_1"),
                    tool_name: "run_command".to_string(),
                    reason: "approval required".to_string(),
                    permission_profile: PermissionProfile::FullAccess,
                    filesystem_sandbox: exagent::config::default_boundary_none(),
                    network_sandbox: exagent::config::default_boundary_none(),
                    env_isolation: exagent::config::default_boundary_none(),
                    command: None,
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_3"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ApprovalDecision {
                    approval_id: ApprovalId::new("approval_1"),
                    status: exagent::session::ApprovalStatus::Denied,
                    note: Some("already decided".to_string()),
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
fn cold_thread_read_restores_pending_approval_overlay_from_events() {
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
    let thread_id = ThreadId::new("thread_cold_pending_approval");
    let turn_id = TurnId::new("turn_1");
    let snapshot = ThreadSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[
            exagent::state::rollout::RolloutItem::ThreadMeta(exagent::state::rollout::ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: snapshot.workspace_root.clone(),
                initial_cwd: snapshot.cwd.clone(),
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ToolInvocationStarted {
                    invocation_id: "inv_call_risky".to_string(),
                    tool_call_id: "call_risky".to_string(),
                    tool_name: "run_command".to_string(),
                    mutating: true,
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_3"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ToolInvocationWaitingApproval {
                    invocation_id: "inv_call_risky".to_string(),
                    approval_id: ApprovalId::new("approval_1"),
                    reason: "approval required".to_string(),
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_4"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ApprovalRequested {
                    approval_id: ApprovalId::new("approval_1"),
                    tool_name: "run_command".to_string(),
                    reason: "approval required".to_string(),
                    permission_profile: PermissionProfile::FullAccess,
                    filesystem_sandbox: exagent::config::default_boundary_none(),
                    network_sandbox: exagent::config::default_boundary_none(),
                    env_isolation: exagent::config::default_boundary_none(),
                    command: None,
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_5"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            }),
        ])
        .expect("write rollout");

    let read = service
        .thread_read(ThreadReadParams {
            thread_id,
            workspace_root: None,
        })
        .expect("read cold thread");

    assert_eq!(read.thread.status, ThreadStatus::WaitingApproval);
    let latest_turn = read.thread.turns.last().expect("turn exists");
    let invocation = latest_turn
        .items
        .iter()
        .find_map(|item| match item {
            ThreadItem::ToolInvocation {
                invocation_id,
                approval_id,
                status,
                ..
            } if invocation_id == "inv_call_risky" => Some((approval_id, status)),
            _ => None,
        })
        .expect("tool invocation item exists");
    assert_eq!(invocation.0.as_ref(), Some(&ApprovalId::new("approval_1")));
    assert_eq!(invocation.1, "waiting_approval");
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
    let thread_id = ThreadId::new("thread_token_count_replay");
    let turn_id = TurnId::new("turn_1");
    let snapshot = ThreadSnapshot::new_thread(
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
            exagent::state::rollout::RolloutItem::ThreadMeta(exagent::state::rollout::ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: snapshot.workspace_root,
                initial_cwd: snapshot.cwd,
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TokenCount {
                    info: Some(token_info.clone()),
                },
            }),
            exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_3"),
                thread_id: thread_id.clone(),
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
    let thread_id = ThreadId::new("thread_cold_interrupt_rollout_only");
    let snapshot = ThreadSnapshot::new_thread(
        thread_id.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let rollout_paths = exagent::state::rollout::rollout_paths(dir.path(), &thread_id);
    let store = exagent::state::rollout::RolloutStore::new(rollout_paths.rollout_path);
    store
        .append_items_blocking(&[exagent::state::rollout::RolloutItem::ThreadMeta(
            exagent::state::rollout::ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: snapshot.workspace_root,
                initial_cwd: snapshot.cwd,
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
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
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap();
    let _turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "request approval".into(),
            input: vec![],
            workspace_root: None,
            turn_mode: Default::default(),
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

    let snapshot = read_thread_snapshot(dir.path(), &thread.thread);
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
