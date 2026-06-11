use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::app_server::protocol::{
    AgentRunResponse, AgentTreeParams, AgentTreeResponse, ApprovalDecisionParams,
    ApprovalDecisionResponse, BoundaryOp, BoundaryOpResponse, EventsReplayParams,
    EventsReplayResponse, EventsSubscribeParams, RunParams, ThreadCompactParams,
    ThreadCompactResponse, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, TurnInterruptParams,
    TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use crate::app_server::ThreadManager;
use crate::config::AgentConfig;
use crate::events::{redact_runtime_event_for_public_boundary, RuntimeEvent};
use crate::llm::LlmClient;
use crate::model::factory::LlmClientFactory;
use crate::registry::ToolRegistry;
use crate::resolver::ModelResolver;

#[async_trait]
pub trait AppServerBoundary: Send + Sync {
    async fn run(&self, params: RunParams) -> Result<AgentRunResponse>;
    async fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse>;
    async fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse>;
    async fn thread_compact(&self, params: ThreadCompactParams) -> Result<ThreadCompactResponse>;
    async fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse>;
    async fn agent_tree(&self, params: AgentTreeParams) -> Result<AgentTreeResponse>;
    async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse>;
    async fn turn_interrupt(&self, params: TurnInterruptParams) -> Result<TurnInterruptResponse>;
    async fn approval_decision(
        &self,
        params: ApprovalDecisionParams,
    ) -> Result<ApprovalDecisionResponse>;
    async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse>;
    async fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse>;
    async fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>>;
}

pub struct AppServerService {
    thread_manager: ThreadManager,
}

impl Default for AppServerService {
    fn default() -> Self {
        Self::new()
    }
}

impl AppServerService {
    pub fn new() -> Self {
        Self {
            thread_manager: ThreadManager::default(),
        }
    }

    pub fn with_config(config: AgentConfig) -> Self {
        Self {
            thread_manager: ThreadManager::from_env(config),
        }
    }

    pub fn with_config_and_model_resolver(
        config: AgentConfig,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self {
        Self {
            thread_manager: ThreadManager::with_model_resolver(config, model_resolver),
        }
    }

    pub fn with_config_model_resolver_and_goal_store(
        config: AgentConfig,
        model_resolver: Arc<dyn ModelResolver>,
        goal_store: crate::index_db::IndexDb,
    ) -> Self {
        Self {
            thread_manager: ThreadManager::with_model_resolver_and_goal_store(
                config,
                model_resolver,
                goal_store,
            ),
        }
    }

    pub fn with_config_llm_factory_model_resolver_and_goal_store(
        config: AgentConfig,
        llm_factory: Arc<dyn LlmClientFactory>,
        model_resolver: Arc<dyn ModelResolver>,
        goal_store: crate::index_db::IndexDb,
    ) -> Self {
        Self {
            thread_manager: ThreadManager::with_llm_factory_model_resolver_and_goal_store(
                config,
                llm_factory,
                model_resolver,
                goal_store,
            ),
        }
    }

    pub fn with_llm<F>(config: AgentConfig, llm: Box<dyn LlmClient>, registry_factory: F) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            thread_manager: ThreadManager::with_llm(config, llm, registry_factory),
        }
    }

    pub fn with_llm_and_model_resolver<F>(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            thread_manager: ThreadManager::with_llm_and_model_resolver(
                config,
                llm,
                registry_factory,
                model_resolver,
            ),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        self.thread_manager.run(params).await
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        self.thread_manager.thread_start(params)
    }

    pub fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse> {
        self.thread_manager.thread_read(params)
    }

    pub async fn thread_compact(
        &self,
        params: ThreadCompactParams,
    ) -> Result<ThreadCompactResponse> {
        self.thread_manager.thread_compact(params).await
    }

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        self.thread_manager.thread_resume(params)
    }

    pub fn agent_tree(&self, params: AgentTreeParams) -> Result<AgentTreeResponse> {
        self.thread_manager.agent_tree(params)
    }

    pub async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        self.thread_manager.turn_start(params).await
    }

    pub async fn turn_interrupt(
        &self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResponse> {
        self.thread_manager.turn_interrupt(params).await
    }

    pub async fn approval_decision(
        &self,
        params: ApprovalDecisionParams,
    ) -> Result<ApprovalDecisionResponse> {
        self.thread_manager.approval_decision(params).await
    }

    pub async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        self.thread_manager.submit_boundary_op(op).await
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        self.thread_manager.events_replay(params)
    }

    pub fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>> {
        let mut raw_events = self.thread_manager.events_subscribe(params)?;
        let (public_tx, public_rx) = broadcast::channel(256);
        tokio::spawn(async move {
            loop {
                match raw_events.recv().await {
                    Ok(event) => {
                        if public_tx
                            .send(redact_runtime_event_for_public_boundary(event))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(public_rx)
    }
}

#[async_trait]
impl AppServerBoundary for AppServerService {
    async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        self.run(params).await
    }

    async fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        self.thread_start(params)
    }

    async fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse> {
        self.thread_read(params)
    }

    async fn thread_compact(&self, params: ThreadCompactParams) -> Result<ThreadCompactResponse> {
        self.thread_compact(params).await
    }

    async fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        self.thread_resume(params)
    }

    async fn agent_tree(&self, params: AgentTreeParams) -> Result<AgentTreeResponse> {
        self.agent_tree(params)
    }

    async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        self.turn_start(params).await
    }

    async fn turn_interrupt(&self, params: TurnInterruptParams) -> Result<TurnInterruptResponse> {
        self.turn_interrupt(params).await
    }

    async fn approval_decision(
        &self,
        params: ApprovalDecisionParams,
    ) -> Result<ApprovalDecisionResponse> {
        self.approval_decision(params).await
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        self.submit_boundary_op(op).await
    }

    async fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        self.events_replay(params)
    }

    async fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>> {
        self.events_subscribe(params)
    }
}
