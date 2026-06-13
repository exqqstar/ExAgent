use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast;

use crate::app_server::override_policy::ensure_supported_permission_profile;
use crate::app_server::protocol::{
    AgentRunResponse, AgentTreeParams, AgentTreeResponse, ApprovalDecisionParams,
    ApprovalDecisionResponse, ApprovalsListParams, ApprovalsListResponse, BoundaryCapability,
    BoundaryOp, BoundaryOpResponse, CheckpointRestoreParams, CheckpointRestoreResponse,
    EventsReplayParams, EventsReplayResponse, EventsSubscribeParams, InitializeParams,
    InitializeResponse, RunParams, SubmitUserInputParams, SubmitUserInputResponse,
    ThreadCompactParams, ThreadCompactResponse, ThreadForkParams, ThreadForkResponse,
    ThreadReadParams, ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse,
    ThreadStartParams, ThreadStartResponse, TurnContextOverrides, TurnInterruptParams,
    TurnInterruptResponse, TurnStartParams, TurnStartResponse, BOUNDARY_PROTOCOL_VERSION,
};
use crate::app_server::request_processors::{
    agent_processor, approvals_processor, checkpoint_processor, compaction_processor,
    events_processor, fork_processor, goal_processor, thread_processor, turn_processor,
};
use crate::app_server::services::AppServerServices;
use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::llm::LlmClient;
#[cfg(test)]
use crate::mcp::client::McpClientFactory;
use crate::model::factory::LlmClientFactory;
use crate::registry::ToolRegistry;
use crate::resolver::{EnvModelResolver, ModelResolver};
#[cfg(test)]
use crate::runtime::agent_profile::AgentType;
#[cfg(test)]
use crate::runtime::subagent::InterAgentCommunication;
#[cfg(test)]
use crate::runtime::turn_mode::TurnMode;
#[cfg(test)]
use crate::state::spawn_edges::{SpawnEdgeStatus, ThreadSpawnEdgeStore};
#[cfg(test)]
use crate::types::{AssistantTurn, ThreadId};

pub struct ThreadManager {
    services: Arc<AppServerServices>,
}

impl Default for ThreadManager {
    fn default() -> Self {
        Self::from_env(AgentConfig::default())
    }
}

impl ThreadManager {
    pub fn from_env(base_config: AgentConfig) -> Self {
        Self::with_model_resolver(base_config, Arc::new(EnvModelResolver))
    }

    pub fn with_model_resolver(
        base_config: AgentConfig,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self {
        Self {
            services: Arc::new(AppServerServices::with_model_resolver(
                base_config,
                model_resolver,
            )),
        }
    }

    pub fn with_model_resolver_and_goal_store(
        base_config: AgentConfig,
        model_resolver: Arc<dyn ModelResolver>,
        goal_store: crate::index_db::IndexDb,
    ) -> Self {
        Self {
            services: Arc::new(
                AppServerServices::with_model_resolver(base_config, model_resolver)
                    .with_goal_store(goal_store),
            ),
        }
    }

    pub fn with_llm_factory_model_resolver_and_goal_store(
        base_config: AgentConfig,
        llm_factory: Arc<dyn LlmClientFactory>,
        model_resolver: Arc<dyn ModelResolver>,
        goal_store: crate::index_db::IndexDb,
    ) -> Self {
        Self {
            services: Arc::new(
                AppServerServices::with_llm_factory_and_model_resolver(
                    base_config,
                    llm_factory,
                    model_resolver,
                )
                .with_goal_store(goal_store),
            ),
        }
    }

    pub fn with_llm<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self::with_llm_and_model_resolver(
            base_config,
            llm,
            registry_factory,
            Arc::new(EnvModelResolver),
        )
    }

    pub fn with_llm_and_model_resolver<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            services: Arc::new(AppServerServices::with_llm_and_model_resolver(
                base_config,
                llm,
                registry_factory,
                model_resolver,
            )),
        }
    }

    #[cfg(test)]
    fn with_llm_and_mcp_client_factory_for_tests<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
        mcp_client_factory: Arc<dyn McpClientFactory>,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            services: Arc::new(
                AppServerServices::with_llm_and_mcp_client_factory_for_tests(
                    base_config,
                    llm,
                    registry_factory,
                    mcp_client_factory,
                ),
            ),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        let workspace_root = params.workspace_root.clone();
        let thinking_mode = params.thinking_mode;
        let permission_profile = params.permission_profile;
        let thread_id = match params.thread_id {
            Some(thread_id) => {
                if let Some(permission_profile) = permission_profile {
                    ensure_supported_permission_profile(permission_profile)?;
                }
                self.thread_resume(ThreadResumeParams {
                    thread_id,
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                })?
                .thread
                .id
            }
            None => {
                self.thread_start(ThreadStartParams {
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                    permission_profile,
                })?
                .thread
                .id
            }
        };

        self.turn_start_and_wait(TurnStartParams {
            thread_id,
            prompt: params.prompt,
            input: vec![],
            workspace_root,
            turn_mode: Default::default(),
            turn_context: thinking_mode.map(|thinking_mode| TurnContextOverrides {
                cwd: None,
                model: None,
                thinking_mode: Some(thinking_mode),
                clear_thinking_mode: false,
            }),
        })
        .await
    }

    pub fn initialize(&self, _params: InitializeParams) -> InitializeResponse {
        InitializeResponse {
            protocol_version: BOUNDARY_PROTOCOL_VERSION.to_string(),
            supported_ops: vec![
                BoundaryCapability::Initialize,
                BoundaryCapability::ThreadStart,
                BoundaryCapability::ThreadResume,
                BoundaryCapability::ThreadRead,
                BoundaryCapability::ThreadFork,
                BoundaryCapability::ThreadCompact,
                BoundaryCapability::ThreadGoal,
                BoundaryCapability::AgentTree,
                BoundaryCapability::ApprovalsList,
                BoundaryCapability::CheckpointRestore,
                BoundaryCapability::TurnStart,
                BoundaryCapability::TurnInterrupt,
                BoundaryCapability::ApprovalDecision,
                BoundaryCapability::SubmitUserInput,
                BoundaryCapability::EventsReplay,
            ],
            supported_streams: vec![BoundaryCapability::EventsSubscribe],
            supported_permission_profiles: crate::config::PermissionProfile::supported_profiles(),
        }
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        thread_processor::thread_start(self.services.as_ref(), params)
    }

    pub fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse> {
        thread_processor::thread_read(self.services.as_ref(), params)
    }

    pub async fn thread_fork(&self, params: ThreadForkParams) -> Result<ThreadForkResponse> {
        fork_processor::thread_fork(self.services.as_ref(), params).await
    }

    pub async fn thread_compact(
        &self,
        params: ThreadCompactParams,
    ) -> Result<ThreadCompactResponse> {
        compaction_processor::thread_compact(self.services.as_ref(), params).await
    }

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        thread_processor::thread_resume(self.services.as_ref(), params)
    }

    pub async fn agent_tree(&self, params: AgentTreeParams) -> Result<AgentTreeResponse> {
        agent_processor::agent_tree(self.services.as_ref(), params).await
    }

    pub async fn approvals_list(
        &self,
        params: ApprovalsListParams,
    ) -> Result<ApprovalsListResponse> {
        approvals_processor::approvals_list(self.services.as_ref(), params).await
    }

    pub async fn checkpoint_restore(
        &self,
        params: CheckpointRestoreParams,
    ) -> Result<CheckpointRestoreResponse> {
        checkpoint_processor::checkpoint_restore(self.services.as_ref(), params).await
    }

    pub async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        turn_processor::turn_start(self.services.as_ref(), params).await
    }

    async fn turn_start_and_wait(&self, params: TurnStartParams) -> Result<AgentRunResponse> {
        turn_processor::turn_start_and_wait(self.services.as_ref(), params).await
    }

    pub async fn turn_interrupt(
        &self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResponse> {
        turn_processor::turn_interrupt(self.services.as_ref(), params).await
    }

    pub async fn approval_decision(
        &self,
        params: ApprovalDecisionParams,
    ) -> Result<ApprovalDecisionResponse> {
        turn_processor::approval_decision(self.services.as_ref(), params).await
    }

    pub async fn submit_user_input(
        &self,
        params: SubmitUserInputParams,
    ) -> Result<SubmitUserInputResponse> {
        turn_processor::submit_user_input(self.services.as_ref(), params).await
    }

    async fn turn_start_direct(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        turn_processor::turn_start_direct(self.services.as_ref(), params).await
    }

    #[cfg(test)]
    async fn run_turn_through_runtime(
        &self,
        params: TurnStartParams,
    ) -> Result<(crate::types::ThreadId, std::path::PathBuf, AssistantTurn)> {
        turn_processor::run_turn_through_runtime(self.services.as_ref(), params).await
    }

    pub async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        match op {
            BoundaryOp::Initialize(params) => {
                Ok(BoundaryOpResponse::Initialized(self.initialize(params)))
            }
            BoundaryOp::ThreadStart(params) => self
                .thread_start(params)
                .map(BoundaryOpResponse::ThreadStarted),
            BoundaryOp::ThreadRead(params) => {
                self.thread_read(params).map(BoundaryOpResponse::ThreadRead)
            }
            BoundaryOp::ThreadFork(params) => self
                .thread_fork(params)
                .await
                .map(BoundaryOpResponse::ThreadFork),
            BoundaryOp::ThreadCompact(params) => self
                .thread_compact(params)
                .await
                .map(BoundaryOpResponse::ThreadCompacted),
            BoundaryOp::ThreadResume(params) => self
                .thread_resume(params)
                .map(BoundaryOpResponse::ThreadResumed),
            BoundaryOp::ThreadGoalSet(params) => {
                goal_processor::thread_goal_set(self.services.as_ref(), params)
                    .await
                    .map(BoundaryOpResponse::ThreadGoalSet)
            }
            BoundaryOp::ThreadGoalGet(params) => {
                goal_processor::thread_goal_get(self.services.as_ref(), params)
                    .await
                    .map(BoundaryOpResponse::ThreadGoalGet)
            }
            BoundaryOp::ThreadGoalClear(params) => {
                goal_processor::thread_goal_clear(self.services.as_ref(), params)
                    .await
                    .map(BoundaryOpResponse::ThreadGoalCleared)
            }
            BoundaryOp::AgentTree(params) => self
                .agent_tree(params)
                .await
                .map(BoundaryOpResponse::AgentTree),
            BoundaryOp::ApprovalsList(params) => self
                .approvals_list(params)
                .await
                .map(BoundaryOpResponse::ApprovalsList),
            BoundaryOp::CheckpointRestore(params) => self
                .checkpoint_restore(params)
                .await
                .map(BoundaryOpResponse::CheckpointRestored),
            BoundaryOp::TurnStart(params) => self
                .turn_start_direct(params)
                .await
                .map(BoundaryOpResponse::TurnStarted),
            BoundaryOp::TurnInterrupt(params) => self
                .turn_interrupt(params)
                .await
                .map(BoundaryOpResponse::TurnInterrupted),
            BoundaryOp::ApprovalDecision(params) => self
                .approval_decision(params)
                .await
                .map(BoundaryOpResponse::ApprovalDecisionSubmitted),
            BoundaryOp::SubmitUserInput(params) => self
                .submit_user_input(params)
                .await
                .map(BoundaryOpResponse::UserInputSubmitted),
            BoundaryOp::EventsReplay(params) => self
                .events_replay(params)
                .map(BoundaryOpResponse::EventsReplayed),
        }
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        events_processor::events_replay(self.services.as_ref(), params)
    }

    pub fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>> {
        events_processor::events_subscribe(self.services.as_ref(), params)
    }
}

#[cfg(test)]
fn agent_run_response(thread_id: ThreadId, final_turn: AssistantTurn) -> AgentRunResponse {
    turn_processor::agent_run_response(thread_id, final_turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::app_server::protocol::{
        AgentTreeParams, ApprovalDecisionStatus, ApprovalsListParams, BoundaryOpResponse,
        ThreadGoalSetParams, ThreadGoalStatus,
    };
    use crate::config::ThinkingMode;
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::index_db::{IndexDb, ProjectUpsert};
    use crate::llm::{LlmRequestOptions, MockLlm};
    use crate::mcp::client::{McpCallOutput, McpClient, McpClientFactory, McpToolDefinition};
    use crate::mcp::config::McpServerConfig;
    use crate::policy::{PendingCommandApproval, PolicyMode};
    use crate::resolved::{ModelRef, ResolvedCredential, ResolvedModelConfig};
    use crate::resolver::ModelResolver;
    use crate::session::{ApprovalId, ApprovalStatus, ThreadLineage, ThreadSnapshot, ThreadSource};
    use crate::state::rollout::{
        rollout_paths, thread_meta_from_snapshot, RolloutItem, RolloutStore,
    };
    use crate::state::spawn_edges::ThreadSpawnEdge;
    use crate::tools::ToolSpec;
    use crate::types::{
        ConversationMessage, EventId, LlmCompletion, ThreadId, TokenUsage, TokenUsageInfo,
        ToolCall, ToolStatus, TurnId,
    };

    struct RecordingToolsLlm {
        observed_tools: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct ForcedWritePlanModeLlm;

    struct ForcedRunPlanModeLlm;

    struct ForcedSpawnPlanModeLlm;

    struct DispatchSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct CloseSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct MessageSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct ForkSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct SpawnOverrideSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
        child_tools: Arc<Mutex<Vec<Vec<String>>>>,
        child_options: Arc<Mutex<Vec<LlmRequestOptions>>>,
    }

    struct SpawnPlannerSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
        child_tools: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct ResumeTreeSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct RecordingGoalContinuationLlm {
        prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct BusyFollowupSubagentLlm {
        child_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
        child_started: Arc<tokio::sync::Notify>,
        release_child: Arc<tokio::sync::Notify>,
    }

    struct CompletionForwardingSubagentLlm {
        parent_prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    struct StaticModelResolver {
        resolved: ResolvedModelConfig,
        requests: Arc<Mutex<Vec<ModelRef>>>,
    }

    #[derive(Clone, Default)]
    struct CountingMcpFactory {
        connected: Arc<AtomicUsize>,
        listed: Arc<AtomicUsize>,
    }

    struct CountingMcpClient {
        listed: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl crate::llm::LlmClient for RecordingToolsLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.observed_tools
                .lock()
                .unwrap()
                .push(tools.iter().map(|tool| tool.name.clone()).collect());
            Ok(AssistantTurn {
                text: Some("recorded tools".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for RecordingGoalContinuationLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.prompts.lock().unwrap().push(messages.to_vec());
            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for ForcedWritePlanModeLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if has_tool_after_last_user(messages, "denied by agent profile") {
                return Ok(AssistantTurn {
                    text: Some("write denied".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_write_in_plan".into(),
                    name: "write_file".into(),
                    arguments: json!({
                        "path": "plan-mode-should-not-write.txt",
                        "content": "not allowed"
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for ForcedRunPlanModeLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if has_tool_after_last_user(messages, "denied by agent profile") {
                return Ok(AssistantTurn {
                    text: Some("run denied".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_run_in_plan".into(),
                    name: "run_command".into(),
                    arguments: json!({
                        "command": "touch plan-mode-should-not-run.txt"
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for ForcedSpawnPlanModeLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if has_tool_after_last_user(messages, "denied by agent profile") {
                return Ok(AssistantTurn {
                    text: Some("spawn denied".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_spawn_worker_in_plan".into(),
                    name: "spawn_agent".into(),
                    arguments: json!({
                        "task_name": "bypass-worker",
                        "message": "write something",
                        "agent_type": "worker"
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for DispatchSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            let has_child_task = messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::User)
                    && message.content == "research child task"
            });
            if has_child_task {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                return Ok(AssistantTurn {
                    text: Some("child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let has_spawn_result = messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::Tool)
                    && message.content.contains("\"thread_id\"")
            });
            if has_spawn_result {
                return Ok(AssistantTurn {
                    text: Some("parent done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_spawn_research".into(),
                    name: "spawn_agent".into(),
                    arguments: json!({
                        "task_name": "research",
                        "message": "research child task",
                        "fork_turns": "none"
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for CloseSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            let has_child_task = messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::User)
                    && message.content == "research child task"
            });
            if has_child_task {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                return Ok(AssistantTurn {
                    text: Some("child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let has_close_result = messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::Tool)
                    && message.content.contains("\"closed_agents\"")
            });
            if has_close_result {
                return Ok(AssistantTurn {
                    text: Some("parent closed child".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let has_spawn_result = messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::Tool)
                    && message.content.contains("\"thread_id\"")
            });
            if has_spawn_result {
                return Ok(AssistantTurn {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "call_close_research".into(),
                        name: "close_agent".into(),
                        arguments: json!({
                            "agent_path": "/root/research"
                        }),
                        thought_signature: None,
                    }],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(AssistantTurn {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_spawn_research".into(),
                    name: "spawn_agent".into(),
                    arguments: json!({
                        "task_name": "research",
                        "message": "research child task",
                        "fork_turns": "none"
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for MessageSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if is_child_prompt(messages) {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                return Ok(AssistantTurn {
                    text: Some("child handled message".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let last_user = last_user_message(messages).unwrap_or_default();
            if last_user.contains("send resumed child") {
                if has_tool_after_last_user(messages, "\"interaction\":\"send_message\"")
                    || has_tool_after_last_user(messages, "Unknown tool: send_message")
                {
                    return Ok(AssistantTurn {
                        text: Some("parent sent resumed message".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(tool_turn(
                    "call_send_resumed_research",
                    "send_message",
                    json!({
                        "recipient_path": "/root/research",
                        "message": "send-only resumed research update"
                    }),
                ));
            }

            if last_user.contains("send child only") {
                if has_tool_after_last_user(messages, "\"interaction\":\"send_message\"")
                    || has_tool_after_last_user(messages, "Unknown tool: send_message")
                {
                    return Ok(AssistantTurn {
                        text: Some("parent sent message".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                if has_tool_after_last_user(messages, "\"thread_id\"") {
                    return Ok(tool_turn(
                        "call_send_research",
                        "send_message",
                        json!({
                            "recipient_path": "/root/research",
                            "message": "send-only research update"
                        }),
                    ));
                }
                return Ok(spawn_research_turn());
            }

            if last_user.contains("spawn child") {
                if has_tool_after_last_user(messages, "\"thread_id\"") {
                    return Ok(AssistantTurn {
                        text: Some("parent spawned child".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(spawn_research_turn());
            }

            if last_user.contains("follow up child") {
                if has_tool_after_last_user(messages, "\"interaction\":\"followup_task\"")
                    || has_tool_after_last_user(messages, "Unknown tool: followup_task")
                {
                    return Ok(AssistantTurn {
                        text: Some("parent followed up".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(tool_turn(
                    "call_followup_research",
                    "followup_task",
                    json!({
                        "recipient_path": "/root/research",
                        "message": "follow-up research update"
                    }),
                ));
            }

            Ok(AssistantTurn {
                text: Some("idle parent".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for ForkSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::User)
                    && message.content == "fork child task"
            }) {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                return Ok(AssistantTurn {
                    text: Some("fork child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let last_user = last_user_message(messages).unwrap_or_default();
            if last_user.contains("old parent fact") {
                return Ok(AssistantTurn {
                    text: Some("old parent answer".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if last_user.contains("new parent fact") {
                return Ok(AssistantTurn {
                    text: Some("new parent answer".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if last_user.contains("remember parent fact") {
                return Ok(AssistantTurn {
                    text: Some("remembered parent fact".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if last_user.contains("fork with all") {
                if has_tool_after_last_user(messages, "\"thread_id\"") {
                    return Ok(AssistantTurn {
                        text: Some("parent forked all".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                if has_tool_after_last_user(messages, "not supported") {
                    return Ok(AssistantTurn {
                        text: Some("parent fork failed".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(spawn_fork_turn("call_spawn_fork_all", "review", "all"));
            }
            if last_user.contains("fork last two") {
                if has_tool_after_last_user(messages, "\"thread_id\"") {
                    return Ok(AssistantTurn {
                        text: Some("parent forked last two".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                if has_tool_after_last_user(messages, "not supported") {
                    return Ok(AssistantTurn {
                        text: Some("parent fork failed".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(spawn_fork_turn("call_spawn_fork_last_two", "trimmed", "2"));
            }

            Ok(AssistantTurn {
                text: Some("idle fork parent".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for SpawnOverrideSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            tools: &[ToolSpec],
            options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::User)
                    && message.content == "override child task"
            }) {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                self.child_tools
                    .lock()
                    .unwrap()
                    .push(tools.iter().map(|tool| tool.name.clone()).collect());
                self.child_options.lock().unwrap().push(options.clone());
                return Ok(AssistantTurn {
                    text: Some("override child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            if has_tool_after_last_user(messages, "\"thread_id\"") {
                return Ok(AssistantTurn {
                    text: Some("parent spawned override".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if has_tool_after_last_user(messages, "invalid arguments") {
                return Ok(AssistantTurn {
                    text: Some("parent override failed".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(tool_turn(
                "call_spawn_override",
                "spawn_agent",
                json!({
                    "task_name": "override",
                    "message": "override child task",
                    "fork_turns": "none",
                    "model": {
                        "provider_id": "openai",
                        "model_id": "gpt-subagent"
                    },
                    "thinking_mode": "high",
                    "agent_type": "reviewer",
                    "agent_role": "reviewer"
                }),
            ))
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for SpawnPlannerSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if messages.iter().any(|message| {
                matches!(message.role, crate::types::MessageRole::User)
                    && message.content == "planner child task"
            }) {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                self.child_tools
                    .lock()
                    .unwrap()
                    .push(tools.iter().map(|tool| tool.name.clone()).collect());
                return Ok(AssistantTurn {
                    text: Some("planner child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            if has_tool_after_last_user(messages, "\"thread_id\"") {
                return Ok(AssistantTurn {
                    text: Some("parent spawned planner".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            Ok(tool_turn(
                "call_spawn_planner",
                "spawn_agent",
                json!({
                    "task_name": "planner-child",
                    "message": "planner child task",
                    "agent_type": "planner",
                    "fork_turns": "none"
                }),
            ))
        }
    }

    #[async_trait]
    impl ModelResolver for StaticModelResolver {
        async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig> {
            self.requests.lock().unwrap().push(model_ref.clone());
            Ok(self.resolved.clone())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for ResumeTreeSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if is_child_prompt(messages) {
                self.child_prompts.lock().unwrap().push(messages.to_vec());
                return Ok(AssistantTurn {
                    text: Some("child done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            let last_user = last_user_message(messages).unwrap_or_default();
            if last_user.contains("spawn child") {
                if has_tool_after_last_user(messages, "\"thread_id\"") {
                    return Ok(AssistantTurn {
                        text: Some("parent spawned child".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(spawn_research_turn());
            }

            if last_user.contains("list agents after resume") {
                if has_tool_after_last_user(messages, "\"agents\"") {
                    let saw_research = has_tool_after_last_user(messages, "/root/research");
                    return Ok(AssistantTurn {
                        text: Some(if saw_research {
                            "listed /root/research".into()
                        } else {
                            "listed missing child".into()
                        }),
                        tool_calls: vec![],
                        reasoning: vec![],
                    }
                    .into_completion());
                }
                return Ok(tool_turn(
                    "call_list_agents",
                    "list_agents",
                    serde_json::json!({}),
                ));
            }

            Ok(AssistantTurn {
                text: Some("idle resume parent".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for BusyFollowupSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if is_child_prompt(messages) {
                let prompt_count = {
                    let mut child_prompts = self.child_prompts.lock().unwrap();
                    child_prompts.push(messages.to_vec());
                    child_prompts.len()
                };
                if prompt_count == 1 {
                    self.child_started.notify_waiters();
                    self.release_child.notified().await;
                }
                return Ok(AssistantTurn {
                    text: Some("busy child released".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            if has_tool_after_last_user(messages, "\"interaction\":\"followup_task\"")
                || has_tool_after_last_user(messages, "Unknown tool: followup_task")
            {
                return Ok(AssistantTurn {
                    text: Some("parent busy followup queued".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if has_tool_after_last_user(messages, "\"thread_id\"") {
                return Ok(tool_turn(
                    "call_followup_busy_research",
                    "followup_task",
                    json!({
                        "recipient_path": "/root/research",
                        "message": "busy follow-up research update"
                    }),
                ));
            }
            Ok(spawn_research_turn())
        }
    }

    #[async_trait]
    impl crate::llm::LlmClient for CompletionForwardingSubagentLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            if is_child_prompt(messages) {
                return Ok(AssistantTurn {
                    text: Some("child final answer".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }

            self.parent_prompts.lock().unwrap().push(messages.to_vec());
            if messages.iter().any(|message| {
                message.injected
                    && message.content.contains("subagent_turn_completed")
                    && message.content.contains("child final answer")
                    && message.content.contains("completed")
            }) {
                return Ok(AssistantTurn {
                    text: Some("parent saw child completion".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }
                .into_completion());
            }
            if has_tool_after_last_user(messages, "\"thread_id\"") {
                return Ok(tool_turn(
                    "call_wait_child_completion",
                    "wait_agent",
                    json!({ "timeout_ms": 5_000 }),
                ));
            }
            Ok(spawn_research_turn())
        }
    }

    fn is_child_prompt(messages: &[ConversationMessage]) -> bool {
        messages.iter().any(|message| {
            matches!(message.role, crate::types::MessageRole::User)
                && message.content == "research child task"
        })
    }

    fn last_user_message(messages: &[ConversationMessage]) -> Option<&str> {
        messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, crate::types::MessageRole::User))
            .map(|message| message.content.as_str())
    }

    fn has_tool_after_last_user(messages: &[ConversationMessage], pattern: &str) -> bool {
        let Some(last_user_index) = messages
            .iter()
            .rposition(|message| matches!(message.role, crate::types::MessageRole::User))
        else {
            return false;
        };
        messages[last_user_index + 1..].iter().any(|message| {
            matches!(message.role, crate::types::MessageRole::Tool)
                && message.content.contains(pattern)
        })
    }

    fn spawn_research_turn() -> LlmCompletion {
        tool_turn(
            "call_spawn_research",
            "spawn_agent",
            json!({
                "task_name": "research",
                "message": "research child task",
                "fork_turns": "none"
            }),
        )
    }

    fn spawn_fork_turn(id: &str, task_name: &str, fork_turns: &str) -> LlmCompletion {
        tool_turn(
            id,
            "spawn_agent",
            json!({
                "task_name": task_name,
                "message": "fork child task",
                "fork_turns": fork_turns
            }),
        )
    }

    fn tool_turn(id: &str, name: &str, arguments: serde_json::Value) -> LlmCompletion {
        AssistantTurn {
            text: None,
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
                thought_signature: None,
            }],
            reasoning: vec![],
        }
        .into_completion()
    }

    #[async_trait]
    impl McpClientFactory for CountingMcpFactory {
        async fn connect(
            &self,
            _server: &McpServerConfig,
            _default_cwd: &Path,
        ) -> Result<Arc<dyn McpClient>> {
            self.connected.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(CountingMcpClient {
                listed: self.listed.clone(),
            }))
        }
    }

    #[async_trait]
    impl McpClient for CountingMcpClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDefinition>> {
            self.listed.fetch_add(1, Ordering::SeqCst);
            Ok(vec![McpToolDefinition {
                name: "lookup".into(),
                description: "Lookup a record through an MCP-backed tool.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
            }])
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<McpCallOutput> {
            Ok(McpCallOutput {
                content: vec![json!({ "type": "text", "text": "found" })],
                is_error: false,
                meta: None,
            })
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    fn mcp_server(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            display_name: id.into(),
            command: "fake".into(),
            args: Vec::new(),
            env: HashMap::new(),
            working_directory: None,
        }
    }

    fn mcp_thread_manager(
        dir: &Path,
        supports_tools: bool,
        factory: CountingMcpFactory,
        observed_tools: Arc<Mutex<Vec<Vec<String>>>>,
    ) -> ThreadManager {
        let mut config = AgentConfig {
            workspace_root: dir.to_path_buf(),
            cwd: dir.to_path_buf(),
            mcp_servers: vec![mcp_server("records")],
            ..AgentConfig::default()
        };
        config.model.capabilities.supports_tools = supports_tools;
        ThreadManager::with_llm_and_mcp_client_factory_for_tests(
            config,
            Box::new(RecordingToolsLlm { observed_tools }),
            || ToolRegistry::new(),
            Arc::new(factory),
        )
    }

    #[test]
    fn thread_start_registers_loaded_runtime_and_thread_resume_reuses_it() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        assert!(manager
            .services
            .runtime_loader
            .runtime_for(&started.thread.id)
            .is_some());
        let started_runtime = manager
            .services
            .runtime_loader
            .runtime_for(&started.thread.id)
            .unwrap();

        let resumed = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
            })
            .expect("thread resume");

        assert_eq!(resumed.thread.id, started.thread.id);
        let resumed_runtime = manager
            .services
            .runtime_loader
            .runtime_for(&started.thread.id)
            .unwrap();
        assert!(Arc::ptr_eq(&started_runtime, &resumed_runtime));
    }

    #[test]
    fn thread_start_writes_rollout_without_legacy_snapshot_or_events() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let rollout_paths = crate::state::rollout::rollout_paths(dir.path(), &started.thread.id);
        assert!(rollout_paths.rollout_path.exists());
        let items =
            RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("read rollout");
        let Some(RolloutItem::ThreadMeta(meta)) = items.first() else {
            panic!("expected thread meta");
        };
        assert_eq!(
            meta.permission_profile,
            crate::config::PermissionProfile::FullAccess
        );
    }

    #[tokio::test]
    async fn boundary_goal_set_schedules_continuation_for_loaded_runtime() {
        let dir = tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .expect("open index db");
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager {
            services: Arc::new(
                AppServerServices::with_llm_and_model_resolver(
                    AgentConfig {
                        workspace_root: dir.path().to_path_buf(),
                        cwd: dir.path().to_path_buf(),
                        ..AgentConfig::default()
                    },
                    Box::new(RecordingGoalContinuationLlm {
                        prompts: prompts.clone(),
                    }),
                    || ToolRegistry::new(),
                    Arc::new(StaticModelResolver {
                        resolved: ResolvedModelConfig::from_provider_profile(
                            "openai",
                            "gpt-goal",
                            None,
                            ResolvedCredential::None,
                            None,
                        ),
                        requests: Arc::new(Mutex::new(Vec::new())),
                    }),
                )
                .with_goal_store(db.clone()),
            ),
        };
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        insert_index_thread(&db, dir.path(), &started.thread.id).await;

        let response = manager
            .submit_boundary_op(BoundaryOp::ThreadGoalSet(ThreadGoalSetParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                objective: Some("ship boundary continuation".into()),
                status: Some(ThreadGoalStatus::Active),
                token_budget: None,
            }))
            .await
            .expect("thread goal set");

        assert!(matches!(response, BoundaryOpResponse::ThreadGoalSet(_)));
        wait_for_child_prompt_count(&prompts, 1).await;
        let prompts = prompts.lock().unwrap();
        let continuation_prompt = &prompts[0];
        assert!(continuation_prompt.iter().any(|message| {
            message.injected
                && message
                    .content
                    .contains("Continue working on the active thread goal")
        }));
        assert!(continuation_prompt
            .iter()
            .any(|message| message.content.contains("ship boundary continuation")));
    }

    #[tokio::test]
    async fn boundary_goal_objective_update_records_runtime_context() {
        let dir = tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .expect("open index db");
        let manager = ThreadManager {
            services: Arc::new(
                AppServerServices::with_llm_and_model_resolver(
                    AgentConfig {
                        workspace_root: dir.path().to_path_buf(),
                        cwd: dir.path().to_path_buf(),
                        ..AgentConfig::default()
                    },
                    Box::new(MockLlm::new(vec![])),
                    || ToolRegistry::new(),
                    Arc::new(StaticModelResolver {
                        resolved: ResolvedModelConfig::from_provider_profile(
                            "openai",
                            "gpt-goal",
                            None,
                            ResolvedCredential::None,
                            None,
                        ),
                        requests: Arc::new(Mutex::new(Vec::new())),
                    }),
                )
                .with_goal_store(db.clone()),
            ),
        };
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        insert_index_thread(&db, dir.path(), &started.thread.id).await;
        manager
            .submit_boundary_op(BoundaryOp::ThreadGoalSet(ThreadGoalSetParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                objective: Some("old objective".into()),
                status: Some(ThreadGoalStatus::Paused),
                token_budget: None,
            }))
            .await
            .expect("create paused goal");

        manager
            .submit_boundary_op(BoundaryOp::ThreadGoalSet(ThreadGoalSetParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                objective: Some("updated external objective".into()),
                status: Some(ThreadGoalStatus::Paused),
                token_budget: None,
            }))
            .await
            .expect("update paused goal");

        let replay =
            wait_for_goal_updated_event(&manager, &started.thread.id, "updated external objective")
                .await;
        assert_eq!(
            replay
                .snapshot
                .expect("live replay snapshot")
                .conversation_message_count,
            1
        );
    }

    #[test]
    fn thread_resume_uses_loaded_runtime_workspace_when_request_omits_workspace_root() {
        let base_dir = tempdir().unwrap();
        let thread_dir = tempdir().unwrap();
        let base_root = std::fs::canonicalize(base_dir.path()).unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: base_root.clone(),
                cwd: base_root,
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let resumed = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                cwd: None,
            })
            .expect("thread resume");

        assert_eq!(resumed.thread.id, started.thread.id);
    }

    #[test]
    fn thread_resume_rejects_loaded_runtime_workspace_mismatch() {
        let thread_dir = tempdir().unwrap();
        let other_dir = tempdir().unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let other_root = std::fs::canonicalize(other_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let err = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: Some(other_root.display().to_string()),
                cwd: None,
            })
            .expect_err("workspace mismatch must be rejected");

        assert!(err.to_string().contains("belongs to workspace"));
    }

    #[tokio::test]
    async fn run_turn_uses_loaded_runtime_workspace_when_request_omits_workspace_root() {
        let base_dir = tempdir().unwrap();
        let thread_dir = tempdir().unwrap();
        let base_root = std::fs::canonicalize(base_dir.path()).unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: base_root.clone(),
                cwd: base_root.clone(),
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("done in loaded workspace".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let (thread_id, workspace_root, final_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "continue".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("turn");
        let response = agent_run_response(thread_id, final_turn);

        assert_eq!(workspace_root, thread_root);
        let _ = base_root;
        assert_eq!(response.text.as_deref(), Some("done in loaded workspace"));
    }

    #[tokio::test]
    async fn agent_tree_reports_tokens_used_from_loaded_runtime_snapshot() {
        let dir = tempdir().unwrap();
        let usage = TokenUsage {
            input_tokens: 40,
            cached_input_tokens: 5,
            output_tokens: 10,
            reasoning_output_tokens: 2,
            total_tokens: 52,
        };
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new_completions(vec![LlmCompletion {
                turn: AssistantTurn {
                    text: Some("counted".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                token_usage: Some(usage.clone()),
            }])),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "count tokens".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("turn");

        let replay = manager
            .events_replay(EventsReplayParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                after_event_id: None,
                limit: None,
                include_snapshot: false,
                event_kinds: vec![],
            })
            .expect("events replay");
        assert!(replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::TokenCount { info: Some(info) }
                    if info.total_token_usage.total_tokens == usage.total_tokens
            )
        }));

        let tree = manager
            .agent_tree(AgentTreeParams {
                thread_id: started.thread.id,
                workspace_root: None,
            })
            .await
            .expect("agent tree");

        assert_eq!(tree.root.tokens_used, Some(usage.total_tokens));
    }

    #[tokio::test]
    async fn run_turn_rejects_loaded_runtime_workspace_mismatch() {
        let thread_dir = tempdir().unwrap();
        let other_dir = tempdir().unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let other_root = std::fs::canonicalize(other_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let err = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id,
                prompt: "continue".into(),
                input: vec![],
                workspace_root: Some(other_root.display().to_string()),
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect_err("workspace mismatch must be rejected");

        assert!(err.to_string().contains("belongs to workspace"));
    }

    #[tokio::test]
    async fn approval_decision_restores_pending_command_from_loaded_runtime_workspace() {
        let base_dir = tempdir().unwrap();
        let thread_dir = tempdir().unwrap();
        std::fs::create_dir_all(thread_dir.path().join("scratch")).unwrap();
        let base_root = std::fs::canonicalize(base_dir.path()).unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: base_root.clone(),
                cwd: base_root,
                policy_mode: PolicyMode::Enforced,
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("try risky command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_restore_loaded_workspace".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }])),
            || {
                let mut registry = ToolRegistry::new();
                registry.register(crate::tools::run_command::RunCommandTool);
                registry
            },
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let turn = manager
            .turn_start(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "request approval".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("turn start");
        let approval_id = wait_for_approval_requested(&manager, &started.thread.id).await;

        manager
            .services
            .policy
            .cancel_pending_for_thread(&started.thread.id)
            .await;
        assert_eq!(
            manager
                .services
                .policy
                .pending_count_for_thread(&started.thread.id)
                .await,
            0
        );

        let decision = manager
            .approval_decision(ApprovalDecisionParams {
                thread_id: started.thread.id.clone(),
                turn_id: Some(turn.turn.id),
                approval_id,
                decision: ApprovalDecisionStatus::Denied,
                note: Some("deny after restore".into()),
                workspace_root: None,
            })
            .await
            .expect("approval decision should restore command from loaded workspace");

        assert_eq!(decision.status, ApprovalDecisionStatus::Denied);
    }

    #[tokio::test]
    async fn turn_start_rejects_while_checkpoint_restore_guard_is_active() {
        let dir = tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: root.clone(),
                cwd: root.clone(),
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("should not run during restore".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let _restore_guard = manager
            .services
            .runtime_loader
            .begin_workspace_restore(&root)
            .expect("restore guard");

        let err = manager
            .turn_start(TurnStartParams {
                thread_id: started.thread.id,
                prompt: "must reject".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect_err("turn start should reject while restore is guarded");

        assert!(matches!(
            err.downcast_ref::<crate::app_server::AppServerError>(),
            Some(crate::app_server::AppServerError::InvalidRequest(message))
                if message.contains("checkpoint restore is in progress")
        ));
    }

    #[tokio::test]
    async fn approval_decision_rejects_while_checkpoint_restore_guard_is_active() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: root.clone(),
                cwd: root.clone(),
                policy_mode: PolicyMode::Enforced,
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("try risky command".into()),
                tool_calls: vec![ToolCall {
                    id: "call_restore_guarded_approval".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            }])),
            || {
                let mut registry = ToolRegistry::new();
                registry.register(crate::tools::run_command::RunCommandTool);
                registry
            },
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let turn = manager
            .turn_start(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "request approval".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("turn start");
        let approval_id = wait_for_approval_requested(&manager, &started.thread.id).await;
        let _restore_guard = manager
            .services
            .runtime_loader
            .begin_workspace_restore(&root)
            .expect("restore guard");

        let err = manager
            .approval_decision(ApprovalDecisionParams {
                thread_id: started.thread.id,
                turn_id: Some(turn.turn.id),
                approval_id,
                decision: ApprovalDecisionStatus::Approved,
                note: Some("must reject".into()),
                workspace_root: None,
            })
            .await
            .expect_err("approval decision should reject while restore is guarded");

        assert!(matches!(
            err.downcast_ref::<crate::app_server::AppServerError>(),
            Some(crate::app_server::AppServerError::InvalidRequest(message))
                if message.contains("checkpoint restore is in progress")
        ));
        assert!(
            root.join("scratch").exists(),
            "rejected approval decision must not run the command"
        );
    }

    #[tokio::test]
    async fn approvals_list_omits_policy_pending_items_without_loaded_runtime() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                policy_mode: PolicyMode::Enforced,
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![])),
            ToolRegistry::new,
        );
        let thread_id = ThreadId::new("thread_unloaded_pending_approval");
        manager
            .services
            .policy
            .create_command_approval(
                thread_id,
                "run_command",
                "rm -rf scratch",
                dir.path().to_path_buf(),
                None,
                false,
                "approval required".into(),
            )
            .await;

        let listed = manager
            .approvals_list(ApprovalsListParams {
                workspace_root: None,
            })
            .await
            .expect("approvals list");

        assert!(listed.approvals.is_empty());
    }

    async fn wait_for_approval_requested(
        manager: &ThreadManager,
        thread_id: &ThreadId,
    ) -> crate::session::ApprovalId {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if let Some(approval_id) = replay.events.iter().find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested { approval_id, .. } => {
                    Some(approval_id.clone())
                }
                _ => None,
            }) {
                return approval_id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for approval request");
    }

    #[tokio::test]
    async fn mcp_tool_specs_follow_existing_provider_tool_gate() {
        let visible_dir = tempdir().unwrap();
        let visible_factory = CountingMcpFactory::default();
        let visible_observed_tools = Arc::new(Mutex::new(Vec::new()));
        let visible_manager = mcp_thread_manager(
            visible_dir.path(),
            true,
            visible_factory.clone(),
            visible_observed_tools.clone(),
        );
        let visible_thread = visible_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("visible thread start");

        visible_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: visible_thread.thread.id,
                prompt: "record visible tools".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("visible turn");

        assert_eq!(visible_factory.connected.load(Ordering::SeqCst), 1);
        assert_eq!(visible_factory.listed.load(Ordering::SeqCst), 1);
        assert!(visible_observed_tools
            .lock()
            .unwrap()
            .iter()
            .any(|tools| tools.iter().any(|name| name == "mcp__records__lookup")));

        let hidden_dir = tempdir().unwrap();
        let hidden_factory = CountingMcpFactory::default();
        let hidden_observed_tools = Arc::new(Mutex::new(Vec::new()));
        let hidden_manager = mcp_thread_manager(
            hidden_dir.path(),
            false,
            hidden_factory.clone(),
            hidden_observed_tools.clone(),
        );
        let hidden_thread = hidden_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("hidden thread start");

        hidden_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: hidden_thread.thread.id,
                prompt: "record hidden tools".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("hidden turn");

        assert_eq!(hidden_factory.connected.load(Ordering::SeqCst), 0);
        assert_eq!(hidden_factory.listed.load(Ordering::SeqCst), 0);
        assert!(hidden_observed_tools
            .lock()
            .unwrap()
            .iter()
            .all(|tools| tools.is_empty()));
    }

    #[tokio::test]
    async fn plan_mode_root_turn_uses_planner_tool_policy() {
        let dir = tempdir().unwrap();
        let observed_tools = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(RecordingToolsLlm {
                observed_tools: observed_tools.clone(),
            }),
            crate::default_tool_registry,
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "make a plan".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: TurnMode::Plan,
                turn_context: None,
            })
            .await
            .expect("plan turn");

        let tools = observed_tools.lock().unwrap();
        let names = tools.first().expect("observed tool names");
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"search_files".to_string()));
        assert!(names.contains(&"list_agents".to_string()));
        assert!(!names.contains(&"write_file".to_string()));
        assert!(!names.contains(&"run_command".to_string()));
        assert!(!names.contains(&"spawn_agent".to_string()));
        assert!(!names.contains(&"followup_task".to_string()));
    }

    #[tokio::test]
    async fn plan_mode_denies_forced_write_file_tool_call() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ForcedWritePlanModeLlm),
            crate::default_tool_registry,
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let response = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "plan but force a write".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: TurnMode::Plan,
                turn_context: None,
            })
            .await
            .expect("plan turn");

        assert_eq!(response.2.text.as_deref(), Some("write denied"));
        assert!(!dir.path().join("plan-mode-should-not-write.txt").exists());
    }

    #[tokio::test]
    async fn plan_mode_denies_forced_run_command_tool_call() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ForcedRunPlanModeLlm),
            crate::default_tool_registry,
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let response = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "plan but force a command".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: TurnMode::Plan,
                turn_context: None,
            })
            .await
            .expect("plan turn");

        assert_eq!(response.2.text.as_deref(), Some("run denied"));
        assert!(!dir.path().join("plan-mode-should-not-run.txt").exists());
    }

    #[tokio::test]
    async fn plan_mode_denies_forced_spawn_agent_tool_call() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ForcedSpawnPlanModeLlm),
            crate::default_tool_registry,
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let response = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "plan but force a worker spawn".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: TurnMode::Plan,
                turn_context: None,
            })
            .await
            .expect("plan turn");

        assert_eq!(response.2.text.as_deref(), Some("spawn denied"));
        let tree = manager
            .agent_tree(AgentTreeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
            })
            .await
            .expect("agent tree");
        assert!(tree.root.children.is_empty());
    }

    #[tokio::test]
    async fn spawn_agent_creates_clean_child_thread_with_lineage() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(DispatchSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, final_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "delegate root".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        assert_eq!(final_turn.text.as_deref(), Some("parent done"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        let child_prompt = child_prompts
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("child prompt should be recorded");
        assert!(child_prompt.iter().any(|message| {
            matches!(message.role, crate::types::MessageRole::User)
                && message.content == "research child task"
        }));
        assert!(!child_prompt
            .iter()
            .any(|message| message.content.contains("delegate root")));

        let rollout_paths = crate::state::rollout::rollout_paths(dir.path(), &child_thread_id);
        let items =
            RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("child rollout");
        let Some(RolloutItem::ThreadMeta(meta)) = items.first() else {
            panic!("expected child thread meta");
        };
        assert_eq!(meta.thread_source, crate::session::ThreadSource::Subagent);
        let lineage = meta.lineage.as_ref().expect("child lineage");
        assert_eq!(lineage.parent_thread_id, started.thread.id);
        assert_eq!(lineage.root_thread_id, started.thread.id);
        assert_eq!(lineage.depth, 1);
        assert_eq!(lineage.agent_path, "/root/research");
        let restarted_edge_store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        let edges = restarted_edge_store
            .read_edges_blocking()
            .expect("read spawn edges");
        assert_eq!(edges.len(), 1);
        let edge = &edges[0];
        assert_eq!(edge.parent_thread_id, started.thread.id);
        assert_eq!(edge.child_thread_id, child_thread_id);
        assert_eq!(edge.root_thread_id, started.thread.id);
        assert_eq!(edge.agent_path, "/root/research");
        assert_eq!(edge.status, SpawnEdgeStatus::Open);
        assert!(edge.closed_at.is_none());

        let parent_edges = restarted_edge_store
            .list_by_parent_blocking(&started.thread.id, None)
            .expect("list parent spawn edges");
        assert_eq!(parent_edges, edges);
        let root_open_edges = restarted_edge_store
            .list_by_root_blocking(&started.thread.id, Some(SpawnEdgeStatus::Open))
            .expect("list open root spawn edges");
        assert_eq!(root_open_edges, edges);

        assert!(items.iter().any(|item| {
            matches!(
                item,
                RolloutItem::ResponseItem(response)
                    if response.message.content == "research child task"
            )
        }));
        assert!(!items.iter().any(|item| {
            matches!(
                item,
                RolloutItem::ResponseItem(response)
                    if response.message.content == "delegate root"
            )
        }));
    }

    #[tokio::test]
    async fn spawn_agent_fork_all_copies_parent_history_into_child_thread() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ForkSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, first_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "remember parent fact".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent memory turn");
        assert_eq!(first_turn.text.as_deref(), Some("remembered parent fact"));

        let (_thread_id, _workspace_root, fork_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "fork with all".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent fork turn");
        assert_eq!(fork_turn.text.as_deref(), Some("parent forked all"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;
        let prompts = child_prompts.lock().unwrap();
        let child_prompt = prompts.first().expect("child prompt");
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "remember parent fact"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "remembered parent fact"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "fork with all"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "fork child task"));
        assert!(!child_prompt.iter().any(|message| {
            message
                .tool_calls
                .iter()
                .any(|call| call.name == "spawn_agent")
        }));

        let rollout_paths = crate::state::rollout::rollout_paths(dir.path(), &child_thread_id);
        let items =
            RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("child rollout");
        let Some(RolloutItem::ThreadMeta(meta)) = items.first() else {
            panic!("expected child thread meta");
        };
        let lineage = meta.lineage.as_ref().expect("child lineage");
        assert_eq!(lineage.parent_thread_id, started.thread.id);
        assert_eq!(lineage.forked_from_id.as_ref(), Some(&started.thread.id));
        assert!(!items
            .iter()
            .skip(1)
            .any(|item| matches!(item, RolloutItem::ThreadMeta(_))));
        assert!(!items.iter().any(|item| match item {
            RolloutItem::EventMsg(event) => event.thread_id == started.thread.id,
            _ => false,
        }));
    }

    #[tokio::test]
    async fn spawn_agent_fork_last_n_copies_only_recent_boundary_turns() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ForkSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "old parent fact".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("old parent turn");
        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "new parent fact".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("new parent turn");

        let (_thread_id, _workspace_root, fork_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "fork last two".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent fork turn");
        assert_eq!(fork_turn.text.as_deref(), Some("parent forked last two"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;
        let prompts = child_prompts.lock().unwrap();
        let child_prompt = prompts.first().expect("child prompt");
        assert!(!child_prompt
            .iter()
            .any(|message| message.content.contains("old parent fact")));
        assert!(!child_prompt
            .iter()
            .any(|message| message.content.contains("old parent answer")));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "new parent fact"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "new parent answer"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "fork last two"));
        assert!(child_prompt
            .iter()
            .any(|message| message.content == "fork child task"));
    }

    #[tokio::test]
    async fn spawn_agent_overrides_child_model_thinking_and_role_metadata() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let child_tools = Arc::new(Mutex::new(Vec::new()));
        let child_options = Arc::new(Mutex::new(Vec::new()));
        let resolver_requests = Arc::new(Mutex::new(Vec::new()));
        let override_model = ResolvedModelConfig::from_provider_profile(
            "openai",
            "gpt-subagent",
            None,
            ResolvedCredential::None,
            None,
        );
        let manager = ThreadManager::with_llm_and_model_resolver(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                thinking_mode: Some(ThinkingMode::Low),
                ..AgentConfig::default()
            },
            Box::new(SpawnOverrideSubagentLlm {
                child_prompts: child_prompts.clone(),
                child_tools: child_tools.clone(),
                child_options: child_options.clone(),
            }),
            || ToolRegistry::new(),
            Arc::new(StaticModelResolver {
                resolved: override_model,
                requests: resolver_requests.clone(),
            }),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn override child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        assert_eq!(turn.text.as_deref(), Some("parent spawned override"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        let requests = resolver_requests.lock().unwrap();
        assert_eq!(
            requests.as_slice(),
            &[ModelRef::new("openai", "gpt-subagent")]
        );

        let prompts = child_prompts.lock().unwrap();
        let child_prompt = prompts.first().expect("child prompt");
        assert!(child_prompt
            .iter()
            .any(|message| message.content.contains("Model: openai:gpt-subagent")));
        let options = child_options.lock().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].thinking_mode, Some(ThinkingMode::High));

        let rollout_paths = crate::state::rollout::rollout_paths(dir.path(), &child_thread_id);
        let items =
            RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("child rollout");
        let Some(RolloutItem::ThreadMeta(meta)) = items.first() else {
            panic!("expected child thread meta");
        };
        let lineage = meta.lineage.as_ref().expect("child lineage");
        assert_eq!(lineage.agent_type, Some(AgentType::Reviewer));
        assert_eq!(lineage.agent_role.as_deref(), Some("reviewer"));
        assert!(items.iter().any(|item| match item {
            RolloutItem::TurnContext(context) => context.model.model_id == "gpt-subagent",
            _ => false,
        }));
    }

    #[tokio::test]
    async fn spawn_agent_injects_role_prompt_context() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let child_tools = Arc::new(Mutex::new(Vec::new()));
        let child_options = Arc::new(Mutex::new(Vec::new()));
        let resolver_requests = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm_and_model_resolver(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(SpawnOverrideSubagentLlm {
                child_prompts: child_prompts.clone(),
                child_tools: child_tools.clone(),
                child_options,
            }),
            crate::default_tool_registry,
            Arc::new(StaticModelResolver {
                resolved: ResolvedModelConfig::from_provider_profile(
                    "openai",
                    "gpt-subagent",
                    None,
                    ResolvedCredential::None,
                    None,
                ),
                requests: resolver_requests,
            }),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn role child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        let prompts = child_prompts.lock().unwrap();
        let child_prompt = prompts.first().expect("child prompt");
        assert!(child_prompt.iter().any(|message| {
            message.injected && message.content.contains("Agent role: reviewer")
        }));
        assert!(child_prompt.iter().any(|message| {
            message.injected && message.content.contains("Agent type: reviewer")
        }));
        assert!(child_prompt.iter().any(|message| {
            message.injected
                && message
                    .content
                    .contains("Agent profile instructions:\nYou are a reviewer agent.")
        }));
        assert!(child_prompt
            .iter()
            .any(|message| { message.injected && message.content.contains("Response guidance:") }));
        let tools = child_tools.lock().unwrap();
        let child_tool_names = tools.first().expect("child tools");
        assert!(child_tool_names.contains(&"read_file".to_string()));
        assert!(child_tool_names.contains(&"search_files".to_string()));
        assert!(child_tool_names.contains(&"list_agents".to_string()));
        assert!(!child_tool_names.contains(&"write_file".to_string()));
        assert!(!child_tool_names.contains(&"run_command".to_string()));
        assert!(!child_tool_names.contains(&"spawn_agent".to_string()));
    }

    #[tokio::test]
    async fn default_mode_can_spawn_real_planner_subagent() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let child_tools = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(SpawnPlannerSubagentLlm {
                child_prompts: child_prompts.clone(),
                child_tools: child_tools.clone(),
            }),
            crate::default_tool_registry,
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, parent_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn planner child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        assert_eq!(parent_turn.text.as_deref(), Some("parent spawned planner"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        let tree = manager
            .agent_tree(AgentTreeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
            })
            .await
            .expect("agent tree");
        assert!(tree
            .root
            .children
            .iter()
            .any(|child| child.agent_type == Some(AgentType::Planner)));

        let prompts = child_prompts.lock().unwrap();
        let child_prompt = prompts.first().expect("child prompt");
        assert!(child_prompt.iter().any(|message| {
            message.injected && message.content.contains("Agent type: planner")
        }));
        assert!(child_prompt.iter().any(|message| {
            message.injected
                && message
                    .content
                    .contains("Agent profile instructions:\nYou are a planner agent.")
        }));

        let tools = child_tools.lock().unwrap();
        let child_tool_names = tools.first().expect("child tools");
        assert!(child_tool_names.contains(&"read_file".to_string()));
        assert!(child_tool_names.contains(&"search_files".to_string()));
        assert!(child_tool_names.contains(&"list_agents".to_string()));
        assert!(!child_tool_names.contains(&"write_file".to_string()));
        assert!(!child_tool_names.contains(&"run_command".to_string()));
        assert!(!child_tool_names.contains(&"spawn_agent".to_string()));
    }

    #[tokio::test]
    async fn cold_turn_start_rehydrates_subagent_tools() {
        let dir = tempdir().unwrap();
        let observed_tools = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(RecordingToolsLlm {
                observed_tools: observed_tools.clone(),
            }),
            crate::default_tool_registry,
        );
        let thread_id = ThreadId::new("thread_cold_subagent_tools");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_paths = rollout_paths(dir.path(), &thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[RolloutItem::ThreadMeta(thread_meta_from_snapshot(
                &snapshot,
            ))])
            .expect("write cold thread meta");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id,
                prompt: "which tools are visible?".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("cold turn start");

        let tools = observed_tools.lock().unwrap();
        let names = tools.first().expect("observed tool names");
        assert!(names.contains(&"spawn_agent".to_string()));
        assert!(names.contains(&"list_agents".to_string()));
        assert!(names.contains(&"close_agent".to_string()));
        assert!(names.contains(&"send_message".to_string()));
        assert!(names.contains(&"followup_task".to_string()));
        assert!(names.contains(&"wait_agent".to_string()));
    }

    #[tokio::test]
    async fn turn_after_cold_events_subscribe_keeps_subagent_tools() {
        let dir = tempdir().unwrap();
        let observed_tools = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(RecordingToolsLlm {
                observed_tools: observed_tools.clone(),
            }),
            crate::default_tool_registry,
        );
        let thread_id = ThreadId::new("thread_subscribe_then_turn_subagent_tools");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_paths = rollout_paths(dir.path(), &thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[RolloutItem::ThreadMeta(thread_meta_from_snapshot(
                &snapshot,
            ))])
            .expect("write cold thread meta");

        // Subscribing first loads and caches the runtime; the cached instance
        // must still carry subagent control when a turn later reuses it.
        let _rx = manager
            .events_subscribe(crate::app_server::protocol::EventsSubscribeParams {
                thread_id: thread_id.clone(),
                workspace_root: None,
                after_event_id: None,
            })
            .expect("cold events subscribe");

        manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id,
                prompt: "which tools are visible?".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("turn after subscribe");

        let tools = observed_tools.lock().unwrap();
        let names = tools.first().expect("observed tool names");
        assert!(names.contains(&"spawn_agent".to_string()));
        assert!(names.contains(&"list_agents".to_string()));
        assert!(names.contains(&"close_agent".to_string()));
        assert!(names.contains(&"send_message".to_string()));
        assert!(names.contains(&"followup_task".to_string()));
        assert!(names.contains(&"wait_agent".to_string()));
    }

    #[tokio::test]
    async fn cold_background_turn_start_rehydrates_subagent_tools() {
        let dir = tempdir().unwrap();
        let observed_tools = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(RecordingToolsLlm {
                observed_tools: observed_tools.clone(),
            }),
            crate::default_tool_registry,
        );
        let thread_id = ThreadId::new("thread_cold_background_subagent_tools");
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        let rollout_paths = rollout_paths(dir.path(), &thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[RolloutItem::ThreadMeta(thread_meta_from_snapshot(
                &snapshot,
            ))])
            .expect("write cold thread meta");

        manager
            .turn_start(TurnStartParams {
                thread_id,
                prompt: "which tools are visible?".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("cold background turn start");

        for _ in 0..50 {
            if !observed_tools.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let tools = observed_tools.lock().unwrap();
        let names = tools.first().expect("observed tool names");
        assert!(names.contains(&"spawn_agent".to_string()));
        assert!(names.contains(&"list_agents".to_string()));
        assert!(names.contains(&"close_agent".to_string()));
        assert!(names.contains(&"send_message".to_string()));
        assert!(names.contains(&"followup_task".to_string()));
        assert!(names.contains(&"wait_agent".to_string()));
    }

    #[tokio::test]
    async fn close_agent_shuts_down_child_and_marks_edge_closed() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(CloseSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, final_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "delegate and close".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        assert_eq!(final_turn.text.as_deref(), Some("parent closed child"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_subagent_closed(&manager, &started.thread.id, &child_thread_id).await;
        assert!(manager
            .services
            .runtime_loader
            .runtime_for(&child_thread_id)
            .is_none());

        let edge_store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        let edges = edge_store
            .list_by_parent_blocking(&started.thread.id, None)
            .expect("list parent spawn edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].child_thread_id, child_thread_id);
        assert_eq!(edges[0].status, SpawnEdgeStatus::Closed);
        assert!(edges[0].closed_at.is_some());
        assert!(edge_store
            .list_by_root_blocking(&started.thread.id, Some(SpawnEdgeStatus::Open))
            .expect("list open edges")
            .is_empty());
    }

    #[tokio::test]
    async fn thread_resume_rehydrates_open_subagent_registry() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let first_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = first_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let (_thread_id, _workspace_root, turn) = first_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("spawn parent turn");
        assert_eq!(turn.text.as_deref(), Some("parent spawned child"));
        let child_thread_id = wait_for_subagent_spawned(&first_manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&first_manager, &child_thread_id).await;

        let resumed_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm { child_prompts }),
            || ToolRegistry::new(),
        );
        resumed_manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                cwd: None,
            })
            .expect("resume root");
        let (_thread_id, _workspace_root, list_turn) = resumed_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id,
                prompt: "list agents after resume".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("list agents turn");

        assert_eq!(list_turn.text.as_deref(), Some("listed /root/research"));
    }

    #[tokio::test]
    async fn send_message_to_resumed_open_subagent_loads_cold_runtime() {
        let dir = tempdir().unwrap();
        let first_child_prompts = Arc::new(Mutex::new(Vec::new()));
        let first_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(MessageSubagentLlm {
                child_prompts: first_child_prompts,
            }),
            || ToolRegistry::new(),
        );
        let started = first_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        let (_thread_id, _workspace_root, spawn_turn) = first_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("spawn parent turn");
        assert_eq!(spawn_turn.text.as_deref(), Some("parent spawned child"));
        let child_thread_id = wait_for_subagent_spawned(&first_manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&first_manager, &child_thread_id).await;

        let resumed_child_prompts = Arc::new(Mutex::new(Vec::new()));
        let resumed_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(MessageSubagentLlm {
                child_prompts: resumed_child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        resumed_manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                cwd: None,
            })
            .expect("resume root");
        let (_thread_id, _workspace_root, send_turn) = resumed_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "send resumed child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("send parent turn");
        assert_eq!(
            send_turn.text.as_deref(),
            Some("parent sent resumed message")
        );
        wait_for_inter_agent_message_sent(
            &resumed_manager,
            &started.thread.id,
            &child_thread_id,
            false,
        )
        .await;

        let (_thread_id, _workspace_root, _child_turn) = resumed_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: child_thread_id.clone(),
                prompt: "drain queued resumed mail".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("manual child turn drains resumed mail");
        wait_for_child_prompt_count(&resumed_child_prompts, 1).await;
        let prompts = resumed_child_prompts.lock().unwrap();
        let prompt = prompts.first().expect("child prompt");
        assert!(prompt.iter().any(|message| {
            message.injected
                && InterAgentCommunication::from_conversation_message(message).is_some_and(|mail| {
                    mail.content == "send-only resumed research update"
                        && !mail.trigger_turn
                        && mail.source_turn_id.is_some()
                })
        }));
    }

    #[tokio::test]
    async fn thread_resume_ignores_closed_spawn_edges() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let first_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(CloseSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = first_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        first_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "delegate and close".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("close parent turn");

        let resumed_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm { child_prompts }),
            || ToolRegistry::new(),
        );
        resumed_manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                cwd: None,
            })
            .expect("resume root");
        let (_thread_id, _workspace_root, list_turn) = resumed_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id,
                prompt: "list agents after resume".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("list agents turn");

        assert_eq!(list_turn.text.as_deref(), Some("listed missing child"));
    }

    #[tokio::test]
    async fn cold_thread_read_shows_subagent_rollout_after_restart() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let first_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = first_manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");
        first_manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("spawn parent turn");
        let child_thread_id = wait_for_subagent_spawned(&first_manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&first_manager, &child_thread_id).await;

        let restarted_manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm { child_prompts }),
            || ToolRegistry::new(),
        );
        let read = restarted_manager
            .thread_read(ThreadReadParams {
                thread_id: child_thread_id,
                workspace_root: None,
            })
            .expect("cold child read");

        assert!(read.thread.turns.iter().any(|turn| {
            turn.items.iter().any(|item| {
                matches!(
                    item,
                    crate::app_server::protocol::ThreadItem::UserMessage { text, .. }
                        if text == "research child task"
                )
            })
        }));
    }

    #[tokio::test]
    async fn agent_tree_projects_root_descendants_closed_roster_and_metadata() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: Arc::new(Mutex::new(Vec::new())),
            }),
            || ToolRegistry::new(),
        );

        let root = ThreadId::new("thread_root");
        let research = ThreadId::new("thread_research");
        let scraper = ThreadId::new("thread_scraper");
        let tests = ThreadId::new("thread_tests");

        write_agent_thread(
            dir.path(),
            ThreadSnapshot::new_thread(
                root.clone(),
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
            ),
            vec![
                RuntimeEvent {
                    event_id: EventId::new("evt_spawn_research"),
                    thread_id: root.clone(),
                    turn_id: Some(TurnId::new("turn_root")),
                    kind: RuntimeEventKind::SubagentSpawned {
                        invocation_id: "inv_research".into(),
                        tool_call_id: "call_research".into(),
                        parent_thread_id: root.clone(),
                        child_thread_id: research.clone(),
                        task_name: "researcher".into(),
                        message_preview: "map the inspector state".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_spawn_tests"),
                    thread_id: root.clone(),
                    turn_id: Some(TurnId::new("turn_root")),
                    kind: RuntimeEventKind::SubagentSpawned {
                        invocation_id: "inv_tests".into(),
                        tool_call_id: "call_tests".into(),
                        parent_thread_id: root.clone(),
                        child_thread_id: tests.clone(),
                        task_name: "test-writer".into(),
                        message_preview: "cover the panel reducer".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_msg_research"),
                    thread_id: root.clone(),
                    turn_id: Some(TurnId::new("turn_root")),
                    kind: RuntimeEventKind::InterAgentMessageSent {
                        invocation_id: "inv_msg".into(),
                        tool_call_id: "call_msg".into(),
                        author_thread_id: root.clone(),
                        recipient_thread_id: research.clone(),
                        author_path: "root".into(),
                        recipient_path: "root/researcher".into(),
                        content_preview: "also check activeSessionId consumers".into(),
                        followup: true,
                        started_turn_id: None,
                    },
                },
            ],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                research.clone(),
                root.clone(),
                root.clone(),
                1,
                "root/researcher",
                Some(AgentType::Explorer),
                Some("research role"),
                Some("Rhea"),
            ),
            vec![
                RuntimeEvent {
                    event_id: EventId::new("evt_spawn_scraper"),
                    thread_id: research.clone(),
                    turn_id: Some(TurnId::new("turn_research")),
                    kind: RuntimeEventKind::SubagentSpawned {
                        invocation_id: "inv_scraper".into(),
                        tool_call_id: "call_scraper".into(),
                        parent_thread_id: research.clone(),
                        child_thread_id: scraper.clone(),
                        task_name: "scraper".into(),
                        message_preview: "read protocol.rs".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_msg_research_root"),
                    thread_id: research.clone(),
                    turn_id: Some(TurnId::new("turn_research")),
                    kind: RuntimeEventKind::InterAgentMessageSent {
                        invocation_id: "inv_research_msg".into(),
                        tool_call_id: "call_research_msg".into(),
                        author_thread_id: research.clone(),
                        recipient_thread_id: root.clone(),
                        author_path: "root/researcher".into(),
                        recipient_path: "root".into(),
                        content_preview: "sent scraper update".into(),
                        followup: false,
                        started_turn_id: None,
                    },
                },
            ],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                scraper.clone(),
                research.clone(),
                root.clone(),
                2,
                "root/researcher/scraper",
                None,
                None,
                None,
            ),
            vec![RuntimeEvent {
                event_id: EventId::new("evt_scraper_error"),
                thread_id: scraper.clone(),
                turn_id: Some(TurnId::new("turn_scraper")),
                kind: RuntimeEventKind::RuntimeError {
                    message: "child failed".into(),
                },
            }],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                tests.clone(),
                root.clone(),
                root.clone(),
                1,
                "root/test-writer",
                None,
                Some("test role"),
                None,
            ),
            vec![],
        );

        let edge_store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                research.clone(),
                root.clone(),
                "root/researcher",
            ))
            .expect("research edge");
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                research.clone(),
                scraper.clone(),
                root.clone(),
                "root/researcher/scraper",
            ))
            .expect("scraper edge");
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                tests.clone(),
                root.clone(),
                "root/test-writer",
            ))
            .expect("tests edge");
        edge_store
            .mark_closed_blocking(&tests)
            .expect("close tests edge");

        let response = manager
            .agent_tree(AgentTreeParams {
                thread_id: root.clone(),
                workspace_root: None,
            })
            .await
            .expect("agent tree");

        assert_eq!(response.root.thread_id.as_ref(), Some(&root));
        assert_eq!(response.root.children.len(), 2);
        let research_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&research))
            .expect("research node");
        assert_eq!(research_node.status.as_str(), "idle");
        assert_eq!(research_node.agent_type, Some(AgentType::Explorer));
        assert_eq!(research_node.agent_role.as_deref(), Some("research role"));
        assert_eq!(research_node.agent_nickname.as_deref(), Some("Rhea"));
        assert_eq!(
            research_node.last_task_message.as_deref(),
            Some("map the inspector state")
        );
        assert_eq!(
            research_node.last_activity.as_deref(),
            Some("sent scraper update")
        );
        assert_eq!(research_node.children.len(), 1);
        assert_eq!(
            research_node.children[0].agent_path,
            "root/researcher/scraper"
        );
        assert_eq!(research_node.children[0].status.as_str(), "failed");
        assert_eq!(
            research_node.children[0].last_task_message.as_deref(),
            Some("read protocol.rs")
        );

        let tests_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&tests))
            .expect("tests node");
        assert_eq!(tests_node.status.as_str(), "done");
        assert_eq!(tests_node.agent_role.as_deref(), Some("test role"));
        assert_eq!(
            tests_node.last_task_message.as_deref(),
            Some("cover the panel reducer")
        );
    }

    #[tokio::test]
    async fn agent_tree_marks_pending_approval_child_as_waiting_approval() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: Arc::new(Mutex::new(Vec::new())),
            }),
            || ToolRegistry::new(),
        );

        let root = ThreadId::new("thread_root");
        let reviewer = ThreadId::new("thread_reviewer");

        write_agent_thread(
            dir.path(),
            ThreadSnapshot::new_thread(
                root.clone(),
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
            ),
            vec![RuntimeEvent {
                event_id: EventId::new("evt_spawn_reviewer"),
                thread_id: root.clone(),
                turn_id: Some(TurnId::new("turn_root")),
                kind: RuntimeEventKind::SubagentSpawned {
                    invocation_id: "inv_reviewer".into(),
                    tool_call_id: "call_reviewer".into(),
                    parent_thread_id: root.clone(),
                    child_thread_id: reviewer.clone(),
                    task_name: "reviewer".into(),
                    message_preview: "inspect the approval gate".into(),
                },
            }],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                reviewer.clone(),
                root.clone(),
                root.clone(),
                1,
                "root/reviewer",
                Some(AgentType::Reviewer),
                None,
                None,
            ),
            vec![],
        );

        ThreadSpawnEdgeStore::for_workspace(dir.path())
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                reviewer.clone(),
                root.clone(),
                "root/reviewer",
            ))
            .expect("reviewer edge");
        manager
            .services
            .policy
            .restore_command_approval(PendingCommandApproval {
                approval_id: ApprovalId::new("approval_agent_tree"),
                thread_id: reviewer.clone(),
                tool_name: "run_command".into(),
                command: "rm -rf scratch".into(),
                cwd: dir.path().to_path_buf(),
                timeout_secs: None,
                persistent: false,
                reason: "requires review".into(),
                checkpoint_id: None,
            })
            .await;

        let response = manager
            .agent_tree(AgentTreeParams {
                thread_id: root,
                workspace_root: None,
            })
            .await
            .expect("agent tree");

        let reviewer_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&reviewer))
            .expect("reviewer node");
        assert_eq!(reviewer_node.status.as_str(), "waiting_approval");
    }

    #[tokio::test]
    async fn agent_tree_projects_child_current_tool_and_tokens_used() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: Arc::new(Mutex::new(Vec::new())),
            }),
            || ToolRegistry::new(),
        );

        let root = ThreadId::new("thread_root");
        let worker = ThreadId::new("thread_worker");
        let turn_id = TurnId::new("turn_worker");
        let token_info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                total_tokens: 12_345,
                ..TokenUsage::default()
            },
            last_token_usage: TokenUsage {
                total_tokens: 345,
                ..TokenUsage::default()
            },
            model_context_window: Some(128_000),
        };
        let worker_snapshot = subagent_snapshot(
            dir.path(),
            worker.clone(),
            root.clone(),
            root.clone(),
            1,
            "root/worker",
            None,
            None,
            None,
        );

        write_agent_thread(
            dir.path(),
            ThreadSnapshot::new_thread(
                root.clone(),
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
            ),
            vec![RuntimeEvent {
                event_id: EventId::new("evt_spawn_worker"),
                thread_id: root.clone(),
                turn_id: Some(TurnId::new("turn_root")),
                kind: RuntimeEventKind::SubagentSpawned {
                    invocation_id: "inv_worker".into(),
                    tool_call_id: "call_worker".into(),
                    parent_thread_id: root.clone(),
                    child_thread_id: worker.clone(),
                    task_name: "worker".into(),
                    message_preview: "inspect token and tool state".into(),
                },
            }],
        );
        write_agent_thread(
            dir.path(),
            worker_snapshot,
            vec![
                RuntimeEvent {
                    event_id: EventId::new("evt_token_count"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::TokenCount {
                        info: Some(token_info),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_completed_start"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationStarted {
                        invocation_id: "inv_completed".into(),
                        tool_call_id: "call_completed".into(),
                        tool_name: "read_file".into(),
                        mutating: false,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_completed_done"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationCompleted {
                        invocation_id: "inv_completed".into(),
                        tool_call_id: "call_completed".into(),
                        tool_name: "read_file".into(),
                        status: ToolStatus::Success,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_active_start"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationStarted {
                        invocation_id: "inv_active".into(),
                        tool_call_id: "call_active".into(),
                        tool_name: "search_files".into(),
                        mutating: false,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_failed_start"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationStarted {
                        invocation_id: "inv_failed".into(),
                        tool_call_id: "call_failed".into(),
                        tool_name: "write_file".into(),
                        mutating: true,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_failed_done"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationFailed {
                        invocation_id: "inv_failed".into(),
                        tool_call_id: "call_failed".into(),
                        tool_name: "write_file".into(),
                        message: "write failed".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_cancelled_start"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationStarted {
                        invocation_id: "inv_cancelled".into(),
                        tool_call_id: "call_cancelled".into(),
                        tool_name: "run_command".into(),
                        mutating: true,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_cancelled_done"),
                    thread_id: worker.clone(),
                    turn_id: Some(turn_id),
                    kind: RuntimeEventKind::ToolInvocationCancelled {
                        invocation_id: "inv_cancelled".into(),
                        tool_call_id: "call_cancelled".into(),
                        tool_name: "run_command".into(),
                        reason: "interrupted".into(),
                    },
                },
            ],
        );
        let stored_worker =
            crate::app_server::thread_store::read_thread_state_from_storage(dir.path(), &worker)
                .expect("read stored worker")
                .expect("stored worker state");
        assert_eq!(
            stored_worker
                .snapshot
                .token_info
                .as_ref()
                .map(|info| info.total_token_usage.total_tokens),
            Some(12_345)
        );

        ThreadSpawnEdgeStore::for_workspace(dir.path())
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                worker.clone(),
                root.clone(),
                "root/worker",
            ))
            .expect("worker edge");

        let response = manager
            .agent_tree(AgentTreeParams {
                thread_id: root,
                workspace_root: None,
            })
            .await
            .expect("agent tree");

        let worker_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&worker))
            .expect("worker node");
        assert_eq!(worker_node.current_tool.as_deref(), Some("search_files"));
        assert_eq!(worker_node.tokens_used, Some(12_345));
    }

    #[tokio::test]
    async fn agent_tree_clears_current_tool_after_approval_decision() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(ResumeTreeSubagentLlm {
                child_prompts: Arc::new(Mutex::new(Vec::new())),
            }),
            || ToolRegistry::new(),
        );

        let root = ThreadId::new("thread_root");
        let gated = ThreadId::new("thread_gated");
        let active = ThreadId::new("thread_active");
        let approval_id = ApprovalId::new("approval_gated_tool");
        let turn_id = TurnId::new("turn_child");

        write_agent_thread(
            dir.path(),
            ThreadSnapshot::new_thread(
                root.clone(),
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
            ),
            vec![
                RuntimeEvent {
                    event_id: EventId::new("evt_spawn_gated"),
                    thread_id: root.clone(),
                    turn_id: Some(TurnId::new("turn_root")),
                    kind: RuntimeEventKind::SubagentSpawned {
                        invocation_id: "inv_spawn_gated".into(),
                        tool_call_id: "call_spawn_gated".into(),
                        parent_thread_id: root.clone(),
                        child_thread_id: gated.clone(),
                        task_name: "gated".into(),
                        message_preview: "run an approval-gated tool".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_spawn_active"),
                    thread_id: root.clone(),
                    turn_id: Some(TurnId::new("turn_root")),
                    kind: RuntimeEventKind::SubagentSpawned {
                        invocation_id: "inv_spawn_active".into(),
                        tool_call_id: "call_spawn_active".into(),
                        parent_thread_id: root.clone(),
                        child_thread_id: active.clone(),
                        task_name: "active".into(),
                        message_preview: "keep a tool active".into(),
                    },
                },
            ],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                gated.clone(),
                root.clone(),
                root.clone(),
                1,
                "root/gated",
                None,
                None,
                None,
            ),
            vec![
                RuntimeEvent {
                    event_id: EventId::new("evt_gated_start"),
                    thread_id: gated.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationStarted {
                        invocation_id: "inv_gated".into(),
                        tool_call_id: "call_gated".into(),
                        tool_name: "run_command".into(),
                        mutating: true,
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_gated_waiting"),
                    thread_id: gated.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolInvocationWaitingApproval {
                        invocation_id: "inv_gated".into(),
                        approval_id: approval_id.clone(),
                        reason: "approval required".into(),
                    },
                },
                RuntimeEvent {
                    event_id: EventId::new("evt_gated_decision"),
                    thread_id: gated.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ApprovalDecision {
                        approval_id,
                        status: ApprovalStatus::Denied,
                        note: Some("denied".into()),
                    },
                },
            ],
        );
        write_agent_thread(
            dir.path(),
            subagent_snapshot(
                dir.path(),
                active.clone(),
                root.clone(),
                root.clone(),
                1,
                "root/active",
                None,
                None,
                None,
            ),
            vec![RuntimeEvent {
                event_id: EventId::new("evt_active_start"),
                thread_id: active.clone(),
                turn_id: Some(turn_id),
                kind: RuntimeEventKind::ToolInvocationStarted {
                    invocation_id: "inv_active".into(),
                    tool_call_id: "call_active".into(),
                    tool_name: "search_files".into(),
                    mutating: false,
                },
            }],
        );

        let edge_store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                gated.clone(),
                root.clone(),
                "root/gated",
            ))
            .expect("gated edge");
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                root.clone(),
                active.clone(),
                root.clone(),
                "root/active",
            ))
            .expect("active edge");

        let response = manager
            .agent_tree(AgentTreeParams {
                thread_id: root,
                workspace_root: None,
            })
            .await
            .expect("agent tree");

        let gated_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&gated))
            .expect("gated node");
        let active_node = response
            .root
            .children
            .iter()
            .find(|node| node.thread_id.as_ref() == Some(&active))
            .expect("active node");
        assert_eq!(gated_node.current_tool, None);
        assert_eq!(active_node.current_tool.as_deref(), Some("search_files"));
    }

    #[tokio::test]
    async fn send_message_enqueues_mail_without_starting_recipient_turn() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(MessageSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, final_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "send child only".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("parent turn");
        assert_eq!(final_turn.text.as_deref(), Some("parent sent message"));

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_inter_agent_message_sent(&manager, &started.thread.id, &child_thread_id, false)
            .await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        assert_eq!(child_prompts.lock().unwrap().len(), 1);
        let child_events = manager
            .events_replay(EventsReplayParams {
                thread_id: child_thread_id.clone(),
                workspace_root: None,
                after_event_id: None,
                limit: None,
                include_snapshot: false,
                event_kinds: vec![],
            })
            .expect("child events");
        let started_turns = child_events
            .events
            .iter()
            .filter(|event| matches!(event.kind, RuntimeEventKind::TurnStarted))
            .count();
        assert_eq!(started_turns, 1);

        let (_thread_id, _workspace_root, _child_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: child_thread_id.clone(),
                prompt: "drain queued send-only mail".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("manual child turn drains send-only mail");
        wait_for_child_prompt_count(&child_prompts, 2).await;
        let prompts = child_prompts.lock().unwrap();
        let second_prompt = prompts.get(1).expect("second child prompt");
        assert!(second_prompt.iter().any(|message| {
            message.injected
                && InterAgentCommunication::from_conversation_message(message).is_some_and(|mail| {
                    mail.content == "send-only research update"
                        && !mail.trigger_turn
                        && mail.source_turn_id.is_some()
                })
        }));
    }

    #[tokio::test]
    async fn followup_task_starts_idle_recipient_turn_with_mailbox_context() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(MessageSubagentLlm {
                child_prompts: child_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, first_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("spawn parent turn");
        assert_eq!(first_turn.text.as_deref(), Some("parent spawned child"));
        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;

        let (_thread_id, _workspace_root, followup_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "follow up child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            })
            .await
            .expect("followup parent turn");
        assert_eq!(followup_turn.text.as_deref(), Some("parent followed up"));
        let event =
            wait_for_inter_agent_message_sent(&manager, &started.thread.id, &child_thread_id, true)
                .await;
        let RuntimeEventKind::InterAgentMessageSent {
            started_turn_id, ..
        } = event.kind
        else {
            panic!("expected inter-agent event");
        };
        assert!(started_turn_id.is_some());

        wait_for_child_prompt_count(&child_prompts, 2).await;
        let prompts = child_prompts.lock().unwrap();
        let second_prompt = prompts.get(1).expect("second child prompt");
        assert!(second_prompt.iter().any(|message| {
            message.injected
                && InterAgentCommunication::from_conversation_message(message).is_some_and(|mail| {
                    mail.content == "follow-up research update"
                        && mail.trigger_turn
                        && mail.source_turn_id.is_some()
                })
        }));
    }

    #[tokio::test]
    async fn followup_task_queues_mail_without_starting_busy_recipient_turn() {
        let dir = tempdir().unwrap();
        let child_prompts = Arc::new(Mutex::new(Vec::new()));
        let child_started = Arc::new(tokio::sync::Notify::new());
        let release_child = Arc::new(tokio::sync::Notify::new());
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(BusyFollowupSubagentLlm {
                child_prompts: child_prompts.clone(),
                child_started: child_started.clone(),
                release_child: release_child.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, final_turn) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            manager.run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "follow up busy child".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            }),
        )
        .await
        .expect("busy followup must not block on child turn")
        .expect("parent turn");
        assert_eq!(
            final_turn.text.as_deref(),
            Some("parent busy followup queued")
        );

        let child_thread_id = wait_for_subagent_spawned(&manager, &started.thread.id).await;
        let event =
            wait_for_inter_agent_message_sent(&manager, &started.thread.id, &child_thread_id, true)
                .await;
        let RuntimeEventKind::InterAgentMessageSent {
            started_turn_id, ..
        } = event.kind
        else {
            panic!("expected inter-agent event");
        };
        assert!(started_turn_id.is_none());

        wait_for_child_prompt_count(&child_prompts, 1).await;
        release_child.notify_waiters();
        wait_for_turn_completed_for_manager(&manager, &child_thread_id).await;
        wait_for_child_prompt_count(&child_prompts, 2).await;
        let prompts = child_prompts.lock().unwrap();
        let second_prompt = prompts.get(1).expect("second child prompt");
        assert!(second_prompt.iter().any(|message| {
            message.injected
                && InterAgentCommunication::from_conversation_message(message).is_some_and(|mail| {
                    mail.content == "busy follow-up research update"
                        && mail.trigger_turn
                        && mail.source_turn_id.is_some()
                })
        }));
    }

    #[tokio::test]
    async fn child_turn_completion_notifies_parent_wait_agent() {
        let dir = tempdir().unwrap();
        let parent_prompts = Arc::new(Mutex::new(Vec::new()));
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            Box::new(CompletionForwardingSubagentLlm {
                parent_prompts: parent_prompts.clone(),
            }),
            || ToolRegistry::new(),
        );
        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: None,
                cwd: None,
                permission_profile: None,
            })
            .expect("thread start");

        let (_thread_id, _workspace_root, final_turn) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            manager.run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "spawn child and wait".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            }),
        )
        .await
        .expect("parent wait should complete")
        .expect("parent turn");

        assert_eq!(
            final_turn.text.as_deref(),
            Some("parent saw child completion")
        );
        let prompts = parent_prompts.lock().unwrap();
        let final_prompt = prompts.last().expect("parent final prompt");
        assert!(final_prompt.iter().any(|message| {
            message.injected
                && message.content.contains("subagent_turn_completed")
                && message.content.contains("child final answer")
                && message.content.contains("completed")
        }));
    }

    async fn wait_for_subagent_spawned(
        manager: &ThreadManager,
        parent_thread_id: &ThreadId,
    ) -> ThreadId {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: parent_thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if let Some(child_thread_id) =
                replay.events.iter().find_map(|event| match &event.kind {
                    RuntimeEventKind::SubagentSpawned {
                        child_thread_id, ..
                    } => Some(child_thread_id.clone()),
                    _ => None,
                })
            {
                return child_thread_id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for subagent spawn event");
    }

    fn write_agent_thread(
        workspace_root: &Path,
        snapshot: ThreadSnapshot,
        events: Vec<RuntimeEvent>,
    ) {
        let paths = rollout_paths(workspace_root, &snapshot.thread_id);
        let store = RolloutStore::new(paths.rollout_path);
        let mut items = vec![RolloutItem::ThreadMeta(thread_meta_from_snapshot(
            &snapshot,
        ))];
        items.extend(events.into_iter().map(RolloutItem::EventMsg));
        store
            .append_items_blocking(&items)
            .expect("write agent thread rollout");
    }

    fn subagent_snapshot(
        workspace_root: &Path,
        thread_id: ThreadId,
        parent_thread_id: ThreadId,
        root_thread_id: ThreadId,
        depth: u32,
        agent_path: &str,
        agent_type: Option<AgentType>,
        agent_role: Option<&str>,
        agent_nickname: Option<&str>,
    ) -> ThreadSnapshot {
        ThreadSnapshot::new_thread_with_options(
            thread_id,
            workspace_root.to_path_buf(),
            workspace_root.to_path_buf(),
            crate::config::PermissionProfile::FullAccess,
            ThreadSource::Subagent,
            Some(ThreadLineage {
                parent_thread_id,
                root_thread_id,
                depth,
                agent_path: agent_path.to_string(),
                agent_type,
                agent_role: agent_role.map(str::to_string),
                agent_nickname: agent_nickname.map(str::to_string),
                forked_from_id: None,
            }),
        )
    }

    async fn wait_for_subagent_closed(
        manager: &ThreadManager,
        parent_thread_id: &ThreadId,
        expected_child_thread_id: &ThreadId,
    ) {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: parent_thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if replay.events.iter().any(|event| match &event.kind {
                RuntimeEventKind::SubagentClosed {
                    closed_thread_id, ..
                } => closed_thread_id == expected_child_thread_id,
                _ => false,
            }) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for subagent close event");
    }

    async fn wait_for_inter_agent_message_sent(
        manager: &ThreadManager,
        parent_thread_id: &ThreadId,
        expected_recipient_thread_id: &ThreadId,
        expected_followup: bool,
    ) -> RuntimeEvent {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: parent_thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if let Some(event) = replay.events.into_iter().find(|event| match &event.kind {
                RuntimeEventKind::InterAgentMessageSent {
                    recipient_thread_id,
                    followup,
                    ..
                } => {
                    recipient_thread_id == expected_recipient_thread_id
                        && *followup == expected_followup
                }
                _ => false,
            }) {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for inter-agent message event");
    }

    async fn wait_for_child_prompt_count(
        child_prompts: &Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
        expected_count: usize,
    ) {
        for _ in 0..200 {
            if child_prompts.lock().unwrap().len() >= expected_count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for child prompt count");
    }

    async fn wait_for_goal_updated_event(
        manager: &ThreadManager,
        thread_id: &ThreadId,
        expected_objective: &str,
    ) -> EventsReplayResponse {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: true,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if replay.events.iter().any(|event| match &event.kind {
                RuntimeEventKind::ThreadGoalUpdated { goal } => {
                    goal.objective == expected_objective
                }
                _ => false,
            }) {
                return replay;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for goal update event");
    }

    async fn insert_index_thread(db: &IndexDb, workspace_root: &Path, thread_id: &ThreadId) {
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Goal Tests".into(),
                path: workspace_root.into(),
            })
            .await
            .expect("upsert project");
        let rollout_path = rollout_paths(workspace_root, thread_id).rollout_path;
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project.id)
        .bind(rollout_path.display().to_string())
        .bind(format!("{} title", thread_id.as_str()))
        .bind(format!("{} preview", thread_id.as_str()))
        .execute(db.pool())
        .await
        .expect("insert index thread");
    }

    async fn wait_for_turn_completed_for_manager(manager: &ThreadManager, thread_id: &ThreadId) {
        for _ in 0..200 {
            let replay = manager
                .events_replay(EventsReplayParams {
                    thread_id: thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                })
                .expect("events replay");
            if replay
                .events
                .iter()
                .any(|event| matches!(event.kind, RuntimeEventKind::TurnCompleted))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for child turn completion");
    }
}
