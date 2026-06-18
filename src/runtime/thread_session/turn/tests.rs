use super::goal_effects::changed_files_for_goal_report;
use super::turn_config::{agent_profile_context_for_turn, effective_agent_type_for_turn};

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};

use crate::agent::Agent;
use crate::config::{AgentConfig, ThinkingMode};
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::llm::{LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink, MockLlm};
use crate::registry::{ToolContext, ToolRegistry};
use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::runtime::agent_profile::AgentType;
use crate::runtime::memory::MemoryRuntime;
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::runtime::thread_session::{RuntimeInterrupt, ThreadSession, ThreadSessionOptions};
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ThreadLineage, ThreadSnapshot};
use crate::state::index_db::IndexDb;
use crate::state::rollout::{RolloutItem, RolloutStore};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{
    AssistantTurn, ConversationContentPart, ConversationMessage, ImageDetail, InputModality,
    LlmCompletion, MessageRole, ReasoningBlock, ThreadId, TokenUsage, ToolCall, ToolResult,
    ToolStatus, TurnId, UserInput,
};

#[test]
fn goal_report_changed_files_extracts_apply_patch_metadata() {
    let result = ToolResult {
        tool_call_id: "call_1".into(),
        tool_name: "apply_patch".into(),
        status: ToolStatus::Success,
        content: "applied".into(),
        meta: Some(json!({
            "changed_files": ["src/runtime/goal/runtime.rs", "src/runtime/goal/runtime.rs"]
        })),
        parts: Vec::new(),
    };

    assert_eq!(
        changed_files_for_goal_report("apply_patch", &result),
        vec!["src/runtime/goal/runtime.rs".to_string()]
    );
}

#[test]
fn goal_report_changed_files_extracts_write_file_path_metadata_only_for_write_tool() {
    let result = ToolResult {
        tool_call_id: "call_1".into(),
        tool_name: "write_file".into(),
        status: ToolStatus::Success,
        content: "wrote".into(),
        meta: Some(json!({
            "normalized_path": "/workspace/src/new.rs",
            "requested_path": "src/new.rs",
            "path": "/workspace/src/new.rs"
        })),
        parts: Vec::new(),
    };

    assert_eq!(
        changed_files_for_goal_report("write_file", &result),
        vec!["/workspace/src/new.rs".to_string()]
    );
    assert!(changed_files_for_goal_report("read_file", &result).is_empty());
}

#[test]
fn goal_report_changed_files_ignores_failed_tool_results() {
    let result = ToolResult {
        tool_call_id: "call_1".into(),
        tool_name: "write_file".into(),
        status: ToolStatus::Error,
        content: "failed".into(),
        meta: Some(json!({ "normalized_path": "/workspace/src/new.rs" })),
        parts: Vec::new(),
    };

    assert!(changed_files_for_goal_report("write_file", &result).is_empty());
}

fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
    let snapshot = ThreadSnapshot::new_thread(
        thread_id.clone(),
        config.workspace_root.clone(),
        config.cwd.clone(),
    );
    let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
    crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[crate::state::rollout::RolloutItem::ThreadMeta(
            crate::state::rollout::thread_meta_from_snapshot(&snapshot),
        )])
        .expect("write rollout session meta");
}

fn append_rollout_items(config: &AgentConfig, thread_id: &ThreadId, items: &[RolloutItem]) {
    let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(items)
        .expect("append rollout items");
}

fn read_rollout_items(config: &AgentConfig, thread_id: &ThreadId) -> Vec<RolloutItem> {
    let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
    RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("read rollout items")
}

#[tokio::test]
async fn memory_context_refresh_is_best_effort_when_project_resolution_fails() {
    let dir = tempdir().unwrap();
    let db_dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_memory_context_best_effort");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let memory_runtime = MemoryRuntime::new(
        IndexDb::open(db_dir.path().join("index.sqlite"))
            .await
            .unwrap(),
    );
    let agent_factory: AgentFactory = Arc::new(|config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(
        ThreadSessionOptions::new(thread_id.clone(), config.clone(), agent_factory)
            .with_memory_runtime(Some(memory_runtime)),
    )
    .expect("create thread session");
    let missing_workspace = dir.path().join("missing-workspace");
    let snapshot =
        ThreadSnapshot::new_thread(thread_id, missing_workspace.clone(), missing_workspace);
    session.context_manager.upsert_ephemeral_internal_context(
        "00_memory_recall",
        ConversationMessage::injected_user_context(
            "00_memory_recall",
            "stale memory context must be cleared on recall failure",
        ),
    );

    session
        .ensure_frozen_memory_context(&snapshot)
        .await
        .expect("frozen memory injection is best-effort");
    session
        .refresh_dynamic_memory_context(&snapshot, "remember durable routing policy")
        .await
        .expect("dynamic memory injection is best-effort");

    let prompt = session
        .context_manager
        .for_prompt(&config.model.capabilities.input_modalities);
    assert!(!prompt.iter().any(|message| {
        message
            .content
            .contains("stale memory context must be cleared on recall failure")
    }));
}

#[test]
fn effective_agent_type_uses_planner_for_plan_mode_root_turn() {
    let dir = tempdir().unwrap();
    let snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("thread_plan_effective_root"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );

    assert_eq!(
        effective_agent_type_for_turn(&snapshot, TurnMode::Plan),
        AgentType::Planner
    );
}

#[test]
fn effective_agent_type_preserves_lineage_for_default_subagent_turn() {
    let dir = tempdir().unwrap();
    let mut snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("thread_explorer_child"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    snapshot.lineage = Some(ThreadLineage {
        parent_thread_id: ThreadId::new("parent"),
        root_thread_id: ThreadId::new("parent"),
        depth: 1,
        agent_path: "explore-auth".into(),
        agent_type: Some(AgentType::Explorer),
        agent_role: None,
        agent_nickname: None,
        forked_from_id: None,
    });

    assert_eq!(
        effective_agent_type_for_turn(&snapshot, TurnMode::Default),
        AgentType::Explorer
    );
}

#[test]
fn effective_agent_type_plan_mode_overrides_worker_lineage_for_one_turn() {
    let dir = tempdir().unwrap();
    let mut snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("thread_worker_child"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    snapshot.lineage = Some(ThreadLineage {
        parent_thread_id: ThreadId::new("parent"),
        root_thread_id: ThreadId::new("parent"),
        depth: 1,
        agent_path: "worker-child".into(),
        agent_type: Some(AgentType::Worker),
        agent_role: Some("implementation worker".into()),
        agent_nickname: None,
        forked_from_id: None,
    });

    assert_eq!(
        effective_agent_type_for_turn(&snapshot, TurnMode::Plan),
        AgentType::Planner
    );
    assert_eq!(
        snapshot.lineage.as_ref().unwrap().agent_type,
        Some(AgentType::Worker)
    );
}

#[test]
fn profile_context_none_for_default_root_turn() {
    let dir = tempdir().unwrap();
    let snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("thread_default_root_profile"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );

    assert!(agent_profile_context_for_turn(&snapshot, TurnMode::Default).is_none());
}

#[test]
fn profile_context_uses_planner_for_plan_mode_root_turn() {
    let dir = tempdir().unwrap();
    let snapshot = ThreadSnapshot::new_thread(
        ThreadId::new("thread_plan_root_profile"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );

    let context =
        agent_profile_context_for_turn(&snapshot, TurnMode::Plan).expect("planner profile context");

    assert_eq!(context.agent_type, Some(AgentType::Planner));
    assert_eq!(context.agent_role, None);
    assert!(context
        .instructions
        .as_deref()
        .unwrap()
        .contains("planner agent"));
    assert!(context
        .response_guidance
        .as_deref()
        .unwrap()
        .contains("ordered implementation steps"));
}

struct RecordingLlm {
    turns: AsyncMutex<VecDeque<AssistantTurn>>,
    prompt_lens: Arc<Mutex<Vec<usize>>>,
    prompt_contents: Option<Arc<Mutex<Vec<Vec<String>>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedModelLlmCall {
    built_model: String,
    options: LlmRequestOptions,
}

struct RecordingModelLlm {
    built_model: String,
    calls: Arc<Mutex<Vec<ObservedModelLlmCall>>>,
}

struct StreamingScriptedLlm {
    events: Vec<LlmStreamEvent>,
    completion: LlmCompletion,
}

impl RecordingLlm {
    fn new(turns: Vec<AssistantTurn>, prompt_lens: Arc<Mutex<Vec<usize>>>) -> Self {
        Self {
            turns: AsyncMutex::new(turns.into()),
            prompt_lens,
            prompt_contents: None,
        }
    }

    fn with_prompt_contents(
        turns: Vec<AssistantTurn>,
        prompt_lens: Arc<Mutex<Vec<usize>>>,
        prompt_contents: Arc<Mutex<Vec<Vec<String>>>>,
    ) -> Self {
        Self {
            turns: AsyncMutex::new(turns.into()),
            prompt_lens,
            prompt_contents: Some(prompt_contents),
        }
    }
}

#[async_trait]
impl LlmClient for StreamingScriptedLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        Ok(self.completion.clone())
    }

    async fn stream(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmCompletion> {
        for event in self.events.clone() {
            sink.event(event).await?;
        }
        sink.event(LlmStreamEvent::Completed(self.completion.clone()))
            .await?;
        Ok(self.completion.clone())
    }
}

#[async_trait]
impl LlmClient for RecordingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.prompt_lens.lock().unwrap().push(messages.len());
        if let Some(prompt_contents) = &self.prompt_contents {
            prompt_contents.lock().unwrap().push(
                messages
                    .iter()
                    .map(|message| message.content.clone())
                    .collect(),
            );
        }
        self.turns
            .lock()
            .await
            .pop_front()
            .map(AssistantTurn::into_completion)
            .ok_or_else(|| anyhow::anyhow!("RecordingLlm is out of scripted turns"))
    }
}

#[async_trait]
impl LlmClient for RecordingModelLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.calls.lock().unwrap().push(ObservedModelLlmCall {
            built_model: self.built_model.clone(),
            options: options.clone(),
        });
        Ok(AssistantTurn {
            text: Some(format!("{} done", self.built_model)),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

enum ScriptedLlmStep {
    Completion(LlmCompletion),
    Error(&'static str),
}

struct ScriptedLlm {
    steps: AsyncMutex<VecDeque<ScriptedLlmStep>>,
    prompt_contents: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.prompt_contents.lock().unwrap().push(
            messages
                .iter()
                .map(|message| message.content.clone())
                .collect(),
        );
        match self.steps.lock().await.pop_front() {
            Some(ScriptedLlmStep::Completion(completion)) => Ok(completion),
            Some(ScriptedLlmStep::Error(message)) => Err(anyhow::anyhow!(message)),
            None => Err(anyhow::anyhow!("ScriptedLlm is out of scripted steps")),
        }
    }
}

fn assistant_completion(text: impl Into<String>) -> LlmCompletion {
    AssistantTurn {
        text: Some(text.into()),
        tool_calls: vec![],
        reasoning: vec![],
    }
    .into_completion()
}

struct MetadataTool;

struct BlockingTool {
    started: Arc<Notify>,
}

#[async_trait]
impl ToolHandler for MetadataTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "metadata_tool",
            "Return content with runtime metadata",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        ToolOutcome::from_result(ToolResult {
            tool_call_id: call.id,
            tool_name: call.name,
            status: ToolStatus::Success,
            content: "model-visible summary".to_string(),
            meta: Some(serde_json::json!({
                "stdout": "runtime stdout that must not enter prompt",
                "stderr": "runtime stderr that must not enter prompt",
                "exec_session_id": "exec_secret",
            })),
            parts: vec![ConversationContentPart::LocalImage {
                path: PathBuf::from("/tmp/tool-result.png"),
                detail: Some(ImageDetail::High),
            }],
        })
    }
}

#[async_trait]
impl ToolHandler for BlockingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "blocking_tool",
            "Block until the test interrupts the turn",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
        self.started.notify_one();
        std::future::pending::<()>().await;
        ToolOutcome::success(
            invocation.call.id,
            invocation.call.name,
            crate::tools::ToolModelOutput::text("unreachable"),
        )
    }
}

#[tokio::test]
async fn thread_session_handles_user_input_and_records_turn_lifecycle() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let final_turn = AssistantTurn {
        text: Some("session turn complete".into()),
        tool_calls: vec![],
        reasoning: vec![],
    };
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![final_turn.clone()])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input(turn_id.clone(), "continue".into(), None, None)
        .await
        .expect("run turn");

    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        panic!("expected user input result");
    };
    assert_eq!(final_turn.text.as_deref(), Some("session turn complete"));

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let replay = live_view.events;
    assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
    assert!(matches!(
        replay[1].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(replay[2].kind, RuntimeEventKind::TurnCompleted));
    assert_eq!(replay[0].turn_id.as_ref(), Some(&turn_id));
    assert_eq!(replay[2].turn_id.as_ref(), Some(&turn_id));

    let snapshot = live_view.snapshot;
    assert!(snapshot.reference_turn_context.is_some());
    assert!(snapshot.conversation[0]
        .content
        .contains("Runtime context:"));
    assert!(snapshot.conversation[1]
        .content
        .contains("Environment context:"));
}

#[tokio::test]
async fn image_input_on_text_only_model_is_rejected_before_turn_start_recording() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_rejects_image_text_only");
    let turn_id = TurnId::new("turn_image_rejected");
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    config.model.capabilities.input_modalities = vec![InputModality::Text];
    write_rollout_meta(&config, &thread_id);
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("should not run".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input_parts(
            turn_id.clone(),
            vec![UserInput::LocalImage {
                path: dir.path().join("screen.png"),
                detail: Some(ImageDetail::High),
            }],
            None,
            None,
        )
        .await;
    let err = match result {
        Ok(_) => panic!("expected image input rejection"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("does not support image input"));
    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert!(live_view
        .events
        .iter()
        .all(|event| !matches!(event.kind, RuntimeEventKind::TurnStarted)));
    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().all(|item| !matches!(
        item,
        RolloutItem::ResponseItem(response)
            if response.turn_id == turn_id && response.message.role == MessageRole::User
    )));
}

#[tokio::test]
async fn missing_local_image_input_is_rejected_before_turn_start_recording() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_rejects_missing_image");
    let turn_id = TurnId::new("turn_missing_image_rejected");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("should not run".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input_parts(
            turn_id.clone(),
            vec![UserInput::LocalImage {
                path: dir.path().join("missing.png"),
                detail: Some(ImageDetail::High),
            }],
            None,
            None,
        )
        .await;
    let err = match result {
        Ok(_) => panic!("expected missing image rejection"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("could not read image"));
    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert!(live_view
        .events
        .iter()
        .all(|event| !matches!(event.kind, RuntimeEventKind::TurnStarted)));
    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().all(|item| !matches!(
        item,
        RolloutItem::ResponseItem(response)
            if response.turn_id == turn_id && response.message.role == MessageRole::User
    )));
}

#[tokio::test]
async fn thread_session_records_reasoning_event_before_assistant_turn() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_records_reasoning_event");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let final_turn = AssistantTurn {
        text: Some("final answer".into()),
        tool_calls: vec![],
        reasoning: vec![ReasoningBlock {
            text: "provider reasoning".to_string(),
            signature: None,
            redacted: false,
        }],
    };
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![final_turn.clone()])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id.clone(), "think first".into(), None, None)
        .await
        .expect("run turn");

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let kinds = live_view
        .events
        .iter()
        .map(|event| &event.kind)
        .collect::<Vec<_>>();

    assert!(matches!(kinds[0], RuntimeEventKind::TurnStarted));
    assert!(matches!(
        kinds[1],
        RuntimeEventKind::Reasoning { summary, content }
            if summary.is_empty() && content == &vec!["provider reasoning".to_string()]
    ));
    assert!(matches!(kinds[2], RuntimeEventKind::AssistantTurn { .. }));
    assert!(matches!(kinds[3], RuntimeEventKind::TurnCompleted));

    let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items =
        RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("read rollout");
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(RuntimeEvent {
            kind: RuntimeEventKind::Reasoning { content, .. },
            ..
        }) if content == &vec!["provider reasoning".to_string()]
    )));
}

#[tokio::test]
async fn thread_session_streams_reasoning_and_assistant_deltas() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_streaming_reasoning");
    let turn_id = TurnId::new("turn_streaming_reasoning");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let completion = LlmCompletion {
        turn: AssistantTurn {
            text: Some("hello world".to_string()),
            tool_calls: vec![],
            reasoning: vec![ReasoningBlock {
                text: "think first".to_string(),
                signature: None,
                redacted: false,
            }],
        },
        token_usage: None,
    };
    let agent_factory: AgentFactory = Arc::new({
        let completion = completion.clone();
        move |config| {
            Ok(Agent::new(
                config,
                Box::new(StreamingScriptedLlm {
                    events: vec![
                        LlmStreamEvent::ReasoningDelta("think ".to_string()),
                        LlmStreamEvent::ReasoningDelta("first".to_string()),
                        LlmStreamEvent::AssistantTextDelta("hello ".to_string()),
                        LlmStreamEvent::AssistantTextDelta("world".to_string()),
                    ],
                    completion: completion.clone(),
                }),
                ToolRegistry::new(),
            ))
        }
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id.clone(), "stream please".into(), None, None)
        .await
        .expect("run turn");

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let kinds = live_view
        .events
        .iter()
        .map(|event| &event.kind)
        .collect::<Vec<_>>();

    assert!(matches!(kinds[0], RuntimeEventKind::TurnStarted));
    assert!(matches!(
        kinds[1],
        RuntimeEventKind::ReasoningDelta { delta } if delta == "think "
    ));
    assert!(matches!(
        kinds[2],
        RuntimeEventKind::ReasoningDelta { delta } if delta == "first"
    ));
    assert!(matches!(
        kinds[3],
        RuntimeEventKind::AssistantTextDelta { delta } if delta == "hello "
    ));
    assert!(matches!(
        kinds[4],
        RuntimeEventKind::AssistantTextDelta { delta } if delta == "world"
    ));
    assert!(matches!(kinds[5], RuntimeEventKind::Reasoning { .. }));
    assert!(matches!(kinds[6], RuntimeEventKind::AssistantTurn { .. }));
    assert!(matches!(kinds[7], RuntimeEventKind::TurnCompleted));

    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response)
            if response.turn_id == turn_id
                && response.message.content == "hello world"
    )));
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(RuntimeEvent {
            kind: RuntimeEventKind::ReasoningDelta { .. }
                | RuntimeEventKind::AssistantTextDelta { .. },
            ..
        })
    )));
}

#[tokio::test]
async fn thread_session_reuses_live_agent_and_snapshot_across_turns() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_live_state");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let factory_call_counter = factory_calls.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        factory_call_counter.fetch_add(1, Ordering::SeqCst);
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![
                AssistantTurn {
                    text: Some("first turn complete".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                AssistantTurn {
                    text: Some("second turn complete".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
            ])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let first = session
        .handle_user_input(TurnId::new("turn_1"), "first input".into(), None, None)
        .await
        .expect("first turn");
    let second = session
        .handle_user_input(TurnId::new("turn_2"), "second input".into(), None, None)
        .await
        .expect("second turn");

    let ThreadOpResult::UserInput {
        final_turn: first_final_turn,
        ..
    } = first
    else {
        panic!("expected first user input result");
    };
    let ThreadOpResult::UserInput {
        final_turn: second_final_turn,
        ..
    } = second
    else {
        panic!("expected second user input result");
    };
    assert_eq!(
        first_final_turn.text.as_deref(),
        Some("first turn complete")
    );
    assert_eq!(
        second_final_turn.text.as_deref(),
        Some("second turn complete")
    );
    assert_eq!(factory_calls.load(Ordering::SeqCst), 1);

    let snapshot =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle())
            .snapshot;
    let contents = snapshot
        .conversation
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(contents.len(), 6);
    assert!(contents[0].contains("Runtime context:"));
    assert!(contents[1].contains("Environment context:"));
    assert_eq!(
        &contents[2..],
        &[
            "first input",
            "first turn complete",
            "second input",
            "second turn complete"
        ]
    );
}

#[tokio::test]
async fn thread_session_restores_base_agent_after_turn_model_override() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_turn_model_override_restores_base");
    let base_model = ResolvedModelConfig::from_provider_profile(
        "openai",
        "gpt-4.1",
        None,
        ResolvedCredential::None,
        None,
    );
    let override_model = ResolvedModelConfig::from_provider_profile(
        "openai",
        "gpt-5",
        None,
        ResolvedCredential::None,
        None,
    );
    let config = AgentConfig {
        model: base_model.clone(),
        thinking_mode: Some(ThinkingMode::Low),
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let built_models = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let built_models_for_factory = built_models.clone();
    let calls_for_factory = calls.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let built_model = config.model.identity.model_id.clone();
        built_models_for_factory
            .lock()
            .unwrap()
            .push(built_model.clone());
        Ok(Agent::new(
            config,
            Box::new(RecordingModelLlm {
                built_model,
                calls: calls_for_factory.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(
            TurnId::new("turn_1"),
            "run with override model".into(),
            Some(ThreadTurnContext {
                cwd: None,
                resolved_model: Some(override_model.clone()),
                thinking_mode: None,
                clear_thinking_mode: false,
                turn_mode: crate::runtime::turn_mode::TurnMode::Default,
            }),
            None,
        )
        .await
        .expect("first turn");
    session
        .handle_user_input(
            TurnId::new("turn_2"),
            "run with base model".into(),
            None,
            None,
        )
        .await
        .expect("second turn");

    assert_eq!(
        built_models.lock().unwrap().clone(),
        vec![
            "gpt-4.1".to_string(),
            "gpt-5".to_string(),
            "gpt-4.1".to_string()
        ]
    );

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].built_model, "gpt-5");
    assert_eq!(calls[0].options.model.as_deref(), Some("gpt-5"));
    assert_eq!(
        calls[0].options.reasoning_capabilities.as_ref(),
        Some(&override_model.capabilities.reasoning)
    );
    assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::Low));

    assert_eq!(calls[1].built_model, "gpt-4.1");
    assert_eq!(calls[1].options.model, None);
    assert_eq!(calls[1].options.reasoning_capabilities, None);
    assert_eq!(calls[1].options.thinking_mode, Some(ThinkingMode::Low));
}

#[tokio::test]
async fn plan_mode_uses_planner_default_thinking_mode_for_root_turn() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_plan_mode_uses_planner_thinking");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        thinking_mode: None,
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_factory = calls.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let built_model = config.model.identity.model_id.clone();
        Ok(Agent::new(
            config,
            Box::new(RecordingModelLlm {
                built_model,
                calls: calls_for_factory.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(
            TurnId::new("turn_plan_default"),
            "plan with default thinking".into(),
            Some(ThreadTurnContext {
                cwd: None,
                resolved_model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
                turn_mode: TurnMode::Plan,
            }),
            None,
        )
        .await
        .expect("plan turn");

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::Medium));
}

#[tokio::test]
async fn plan_mode_explicit_thinking_override_wins_over_planner_default() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_plan_mode_thinking_override");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        thinking_mode: None,
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_factory = calls.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let built_model = config.model.identity.model_id.clone();
        Ok(Agent::new(
            config,
            Box::new(RecordingModelLlm {
                built_model,
                calls: calls_for_factory.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(
            TurnId::new("turn_plan_high"),
            "plan with high thinking".into(),
            Some(ThreadTurnContext {
                cwd: None,
                resolved_model: None,
                thinking_mode: Some(ThinkingMode::High),
                clear_thinking_mode: false,
                turn_mode: TurnMode::Plan,
            }),
            None,
        )
        .await
        .expect("plan turn");

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::High));
}

#[tokio::test]
async fn thread_session_next_sampling_uses_committed_history() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_committed_history");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::new(
                vec![
                    AssistantTurn {
                        text: None,
                        tool_calls: vec![ToolCall {
                            id: "call_missing".into(),
                            name: "missing_tool".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        }],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                ],
                prompt_lens_for_llm.clone(),
            )),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "use a tool".into(), None, None)
        .await
        .expect("run turn");

    assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 5]);
}

#[tokio::test]
async fn thread_session_records_model_only_tool_message_in_prompt_context() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_model_only_tool_message");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let prompt_contents_for_llm = prompt_contents.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let mut registry = ToolRegistry::new();
        registry.register(MetadataTool);
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::with_prompt_contents(
                vec![
                    AssistantTurn {
                        text: None,
                        tool_calls: vec![ToolCall {
                            id: "call_metadata".into(),
                            name: "metadata_tool".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        }],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                ],
                prompt_lens_for_llm.clone(),
                prompt_contents_for_llm.clone(),
            )),
            registry,
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "use metadata tool".into(), None, None)
        .await
        .expect("run turn");

    let prompts = prompt_contents.lock().unwrap();
    assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 5]);
    let second_prompt = prompts.last().expect("second prompt after tool");
    assert!(second_prompt
        .iter()
        .any(|content| content == "model-visible summary"));
    assert!(!second_prompt
        .iter()
        .any(|content| content.contains("runtime stdout")));
    assert!(!second_prompt
        .iter()
        .any(|content| content.contains("\"meta\"")));
    assert!(!second_prompt
        .iter()
        .any(|content| content.contains("exec_secret")));

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let tool_message = live_view
        .snapshot
        .conversation
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .expect("tool message in snapshot conversation");
    assert_eq!(
        tool_message.parts,
        vec![ConversationContentPart::LocalImage {
            path: PathBuf::from("/tmp/tool-result.png"),
            detail: Some(ImageDetail::High),
        }]
    );
}

#[tokio::test]
async fn thread_session_interrupt_records_model_visible_tool_result() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_interrupt_tool_context");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let prompt_contents_for_llm = prompt_contents.clone();
    let tool_started = Arc::new(Notify::new());
    let tool_started_for_registry = tool_started.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let mut registry = ToolRegistry::new();
        registry.register(BlockingTool {
            started: tool_started_for_registry.clone(),
        });
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::with_prompt_contents(
                vec![
                    AssistantTurn {
                        text: None,
                        tool_calls: vec![ToolCall {
                            id: "call_blocking".into(),
                            name: "blocking_tool".into(),
                            arguments: serde_json::json!({}),
                            thought_signature: None,
                        }],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("after interrupt".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                ],
                prompt_lens_for_llm.clone(),
                prompt_contents_for_llm.clone(),
            )),
            registry,
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");
    let (interrupt_tx, interrupt_rx) = oneshot::channel();
    let interrupted = Arc::new(Notify::new());
    let interrupted_for_waiter = interrupted.clone();
    let tool_started_for_waiter = tool_started.clone();

    let run_turn = session.handle_user_input(
        turn_id.clone(),
        "call blocking tool".into(),
        None,
        Some(RuntimeInterrupt {
            interrupt_rx,
            interrupted,
        }),
    );
    let send_interrupt = async move {
        tool_started_for_waiter.notified().await;
        interrupt_tx.send(()).expect("send interrupt");
        interrupted_for_waiter.notified().await;
    };
    let (result, _) = tokio::join!(run_turn, send_interrupt);
    assert!(result.is_err());

    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response)
            if response.turn_id == turn_id
                && response.message.tool_call_id.as_deref() == Some("call_blocking")
                && response.message.content == "interrupted"
    )));

    session
        .handle_user_input(TurnId::new("turn_2"), "continue".into(), None, None)
        .await
        .expect("second turn after interrupted tool");

    let prompts = prompt_contents.lock().unwrap();
    assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 6]);
    let second_prompt = prompts.last().expect("second prompt after interrupt");
    assert!(second_prompt.iter().any(|content| content == "interrupted"));
}

#[tokio::test]
async fn thread_session_continues_until_assistant_turn_has_no_tool_calls() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_no_legacy_max_turns");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        let mut turns = Vec::new();
        for index in 0..13 {
            turns.push(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: format!("call_{index}"),
                    name: "missing_tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                }],
                reasoning: vec![],
            });
        }
        turns.push(AssistantTurn {
            text: Some("done after tools".into()),
            tool_calls: vec![],
            reasoning: vec![],
        });
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::new(turns, prompt_lens_for_llm.clone())),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input(turn_id, "keep going".into(), None, None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        panic!("expected user input result");
    };

    assert_eq!(final_turn.text.as_deref(), Some("done after tools"));
    assert_eq!(prompt_lens.lock().unwrap().len(), 14);
}

#[tokio::test]
async fn thread_session_sampling_prompt_includes_runtime_context() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_prompt_context");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().join("subdir"),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let prompt_contents_for_llm = prompt_contents.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::with_prompt_contents(
                vec![AssistantTurn {
                    text: Some("context received".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }],
                prompt_lens_for_llm.clone(),
                prompt_contents_for_llm.clone(),
            )),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "inspect context".into(), None, None)
        .await
        .expect("run turn");

    let prompts = prompt_contents.lock().unwrap();
    assert_eq!(*prompt_lens.lock().unwrap(), vec![3]);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0][0].contains("Runtime context:"));
    assert!(prompts[0][1].contains("Environment context:"));
    assert_eq!(prompts[0][2], "inspect context");
}

/// Verifies the F2 streaming contract at the ThreadSession boundary:
/// assistant/tool events are recorded one step at a time, and the final
/// snapshot contains the conversation items that produced those events.
#[tokio::test]
async fn thread_session_streams_events_paired_with_snapshot() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_streaming_capture");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![
                AssistantTurn {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "call_x".into(),
                        name: "missing_tool".into(),
                        arguments: serde_json::json!({}),
                        thought_signature: None,
                    }],
                    reasoning: vec![],
                },
                AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
            ])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input(turn_id.clone(), "hi".into(), None, None)
        .await
        .expect("session should complete the two-step turn");
    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        panic!("expected user input result");
    };

    assert_eq!(final_turn.text.as_deref(), Some("done"));
    let replay =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle()).events;
    assert_eq!(replay.len(), 7, "expected tool lifecycle plus turn events");
    assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
    assert!(matches!(
        replay[1].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(
        replay[2].kind,
        RuntimeEventKind::ToolInvocationStarted { .. }
    ));
    assert!(matches!(
        replay[3].kind,
        RuntimeEventKind::ToolInvocationFailed { .. }
    ));
    assert!(matches!(
        replay[4].kind,
        RuntimeEventKind::ToolResult { .. }
    ));
    assert!(matches!(
        replay[5].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(replay[6].kind, RuntimeEventKind::TurnCompleted));

    let snapshot =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle())
            .snapshot;
    assert_eq!(snapshot.conversation.len(), 6);
    assert!(snapshot.conversation[0]
        .content
        .contains("Runtime context:"));
    assert!(snapshot.conversation[1]
        .content
        .contains("Environment context:"));
    assert_eq!(snapshot.conversation[2].content, "hi");
    assert_eq!(snapshot.conversation[5].content, "done");
}

#[tokio::test]
async fn thread_session_pre_turn_compaction_skips_when_under_budget() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_pre_turn_compaction_skip");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        auto_compact_token_limit: Some(100_000),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    append_rollout_items(
        &config,
        &thread_id,
        &[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ],
    );
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "new user".into(), None, None)
        .await
        .expect("run turn");

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert!(!live_view
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
    assert!(!read_rollout_items(&config, &thread_id)
        .iter()
        .any(|item| matches!(item, RolloutItem::Compacted(_))));
}

#[tokio::test]
async fn thread_session_pre_turn_compaction_runs_before_new_user_message() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_pre_turn_compaction_before_user");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        auto_compact_token_limit: Some(1),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    append_rollout_items(
        &config,
        &thread_id,
        &[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ],
    );
    let prompt_lens = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_lens_for_llm = prompt_lens.clone();
    let prompt_contents_for_llm = prompt_contents.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingLlm::with_prompt_contents(
                vec![
                    AssistantTurn {
                        text: Some("summary from compaction".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                ],
                prompt_lens_for_llm.clone(),
                prompt_contents_for_llm.clone(),
            )),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");
    session.context_manager.upsert_ephemeral_internal_context(
        "01_project_docs",
        ConversationMessage::injected_user_context(
            "01_project_docs",
            "prompt-only project docs must not be compacted",
        ),
    );

    session
        .handle_user_input(turn_id.clone(), "new user".into(), None, None)
        .await
        .expect("run turn");

    let prompts = prompt_contents.lock().unwrap();
    assert_eq!(*prompt_lens.lock().unwrap(), vec![2, 4]);
    assert!(prompts[0].join("\n").contains("old user"));
    assert!(prompts[0].join("\n").contains("old assistant"));
    assert!(!prompts[0].join("\n").contains("new user"));
    assert!(!prompts[0]
        .join("\n")
        .contains("prompt-only project docs must not be compacted"));
    assert!(prompts[1].join("\n").contains("summary from compaction"));
    assert!(prompts[1].join("\n").contains("new user"));
    assert!(!prompts[1].join("\n").contains("old user"));

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let contents = live_view
        .snapshot
        .conversation
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    assert!(contents[0].contains("summary from compaction"));
    assert_eq!(contents[3], "new user");
    assert_eq!(contents[4], "done");
    assert!(live_view
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
    assert!(live_view
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::TokenCount { .. })));

    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items
        .iter()
        .any(|item| matches!(item, RolloutItem::Compacted(compacted)
                if compacted.message == "summary from compaction")));
}

#[tokio::test]
async fn thread_session_pre_turn_compaction_replays_replacement_history() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_pre_turn_compaction_replay");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        auto_compact_token_limit: Some(1),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    append_rollout_items(
        &config,
        &thread_id,
        &[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ],
    );
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![
                AssistantTurn {
                    text: Some("summary from compaction".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
            ])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");
    session
        .handle_user_input(turn_id, "new user".into(), None, None)
        .await
        .expect("run turn");
    drop(session);

    let resumed = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        }),
    ))
    .expect("resume thread session");
    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &resumed.live_state_handle());
    let contents = live_view
        .snapshot
        .conversation
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert!(contents[0].contains("summary from compaction"));
    assert!(contents.contains(&"new user"));
    assert!(contents.contains(&"done"));
    assert!(!contents.contains(&"old user"));
    assert!(!contents.contains(&"old assistant"));
}

#[tokio::test]
async fn thread_session_manual_compaction_writes_checkpoint_without_user_turn() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_manual_compaction");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    append_rollout_items(
        &config,
        &thread_id,
        &[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ],
    );
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("manual summary".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_manual_compaction()
        .await
        .expect("manual compaction should succeed");

    assert!(matches!(result, ThreadOpResult::Ack));
    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert_eq!(
        live_view
            .snapshot
            .latest_compaction
            .as_ref()
            .map(|summary| summary.summary.as_str()),
        Some("manual summary")
    );
    assert!(live_view.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::CompactionWritten { summary }
            if event.turn_id.is_none() && summary.summary == "manual summary"
    )));
    assert!(live_view.events.iter().any(|event| matches!(
        event,
        RuntimeEvent {
            turn_id: None,
            kind: RuntimeEventKind::TokenCount { .. },
            ..
        }
    )));

    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::Compacted(compacted) if compacted.message == "manual summary"
    )));
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(RuntimeEvent {
            turn_id: None,
            kind: RuntimeEventKind::TokenCount { .. },
            ..
        })
    )));
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response) if response.turn_id.as_str() == "manual_compaction"
    )));
    assert!(!live_view.events.iter().any(|event| event
        .turn_id
        .as_ref()
        .is_some_and(|turn_id| { turn_id.as_str() == "manual_compaction" })));
}

#[tokio::test]
async fn thread_session_manual_compaction_failure_records_unscoped_runtime_error() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_manual_compaction_failure");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    append_rollout_items(
        &config,
        &thread_id,
        &[
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::user("old user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_0"),
                ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
            ),
        ],
    );
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    let error = match session.handle_manual_compaction().await {
        Ok(_) => panic!("empty compaction summary should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("empty compaction summary"));
    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert_eq!(live_view.status, ThreadRuntimeStatus::Idle);
    assert!(live_view.events.iter().any(|event| matches!(
        event,
        RuntimeEvent {
            turn_id: None,
            kind: RuntimeEventKind::RuntimeError { message },
            ..
        } if message.contains("empty compaction summary")
    )));
    assert!(read_rollout_items(&config, &thread_id)
        .iter()
        .any(|item| matches!(
            item,
            RolloutItem::EventMsg(RuntimeEvent {
                turn_id: None,
                kind: RuntimeEventKind::RuntimeError { message },
                ..
            }) if message.contains("empty compaction summary")
        )));
}

#[tokio::test]
async fn thread_session_records_token_usage_after_assistant_response() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_records_token_usage");
    let turn_id = TurnId::new("turn_1");
    let usage = TokenUsage {
        input_tokens: 40,
        cached_input_tokens: 5,
        output_tokens: 10,
        reasoning_output_tokens: 2,
        total_tokens: 52,
    };
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    config.model.capabilities.context_window = Some(1_000);
    write_rollout_meta(&config, &thread_id);
    let usage_for_llm = usage.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new_completions(vec![LlmCompletion {
                turn: AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                token_usage: Some(usage_for_llm.clone()),
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "count tokens".into(), None, None)
        .await
        .expect("run turn");

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    let token_info = live_view.events.iter().find_map(|event| match &event.kind {
        RuntimeEventKind::TokenCount { info } => info.clone(),
        _ => None,
    });
    let token_info = token_info.expect("token count event");
    assert_eq!(token_info.last_token_usage, usage);
    assert_eq!(token_info.total_token_usage, usage);
    assert_eq!(token_info.model_context_window, Some(1_000));

    let rollout_items = read_rollout_items(&config, &thread_id);
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event)
            if matches!(event.kind, RuntimeEventKind::TokenCount { info: Some(_) })
    )));
}

#[tokio::test]
async fn thread_session_does_not_emit_token_count_without_model_usage() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_no_bogus_token_usage");
    let turn_id = TurnId::new("turn_1");
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    config.model.capabilities.context_window = Some(1_000);
    write_rollout_meta(&config, &thread_id);
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("done".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(turn_id, "count tokens".into(), None, None)
        .await
        .expect("run turn");

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert!(!live_view
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::TokenCount { .. })));
}

#[tokio::test]
async fn context_window_error_compacts_and_retries_once() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_context_window_retry");
    let turn_id = TurnId::new("turn_1");
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "prompt-only project docs must not be compacted",
    )
    .expect("write project docs");
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    config.model.capabilities.context_window = Some(1_000);
    write_rollout_meta(&config, &thread_id);
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents_for_llm = prompt_contents.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(ScriptedLlm {
                steps: AsyncMutex::new(
                    vec![
                        ScriptedLlmStep::Error("context_length_exceeded: too many tokens"),
                        ScriptedLlmStep::Completion(assistant_completion(
                            "summary after context error",
                        )),
                        ScriptedLlmStep::Completion(assistant_completion("done after retry")),
                    ]
                    .into(),
                ),
                prompt_contents: prompt_contents_for_llm.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory,
    ))
    .expect("create thread session");

    let result = session
        .handle_user_input(turn_id.clone(), "new user".into(), None, None)
        .await
        .expect("retry should recover");
    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        panic!("expected user input result");
    };

    assert_eq!(final_turn.text.as_deref(), Some("done after retry"));
    let prompts = prompt_contents.lock().unwrap();
    assert_eq!(prompts.len(), 3);
    assert!(prompts[0].join("\n").contains("new user"));
    assert!(prompts[0]
        .join("\n")
        .contains("prompt-only project docs must not be compacted"));
    assert!(prompts[1].join("\n").contains("new user"));
    assert!(!prompts[1]
        .join("\n")
        .contains("prompt-only project docs must not be compacted"));
    assert!(prompts[2]
        .join("\n")
        .contains("summary after context error"));
    assert!(prompts[2].join("\n").contains("new user"));
    assert!(prompts[2]
        .iter()
        .any(|message| message.contains("Runtime context:")));
    assert!(prompts[2]
        .iter()
        .any(|message| message.contains("Environment context:")));
    let runtime_context_index = prompts[2]
        .iter()
        .position(|message| message.contains("Runtime context:"))
        .expect("runtime context in retry prompt");
    let user_index = prompts[2]
        .iter()
        .position(|message| message == "new user")
        .expect("current user in retry prompt");
    assert!(runtime_context_index < user_index);

    let live_view =
        ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
    assert!(live_view.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::TokenCount {
            info: Some(info)
        } if info.last_token_usage.total_tokens == 1_000
    )));
    assert!(live_view
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
}

#[tokio::test]
async fn context_window_error_retry_does_not_loop() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_context_window_retry_once");
    let turn_id = TurnId::new("turn_1");
    let mut config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    config.model.capabilities.context_window = Some(1_000);
    write_rollout_meta(&config, &thread_id);
    let prompt_contents = Arc::new(Mutex::new(Vec::new()));
    let prompt_contents_for_llm = prompt_contents.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(ScriptedLlm {
                steps: AsyncMutex::new(
                    vec![
                        ScriptedLlmStep::Error("context_length_exceeded: too many tokens"),
                        ScriptedLlmStep::Completion(assistant_completion(
                            "summary after context error",
                        )),
                        ScriptedLlmStep::Error("maximum context length is 1000 tokens"),
                    ]
                    .into(),
                ),
                prompt_contents: prompt_contents_for_llm.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session =
        ThreadSession::new(ThreadSessionOptions::new(thread_id, config, agent_factory))
            .expect("create thread session");

    let error = match session
        .handle_user_input(turn_id, "new user".into(), None, None)
        .await
    {
        Ok(_) => panic!("retry should fail once"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("maximum context length"));
    assert_eq!(prompt_contents.lock().unwrap().len(), 3);
}
