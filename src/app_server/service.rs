use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::app_server::protocol::{
    AgentRunResponse, ApprovalDecisionParams, ApprovalDecisionResponse, BoundaryOp,
    BoundaryOpResponse, EventsReplayParams, EventsReplayResponse, EventsSubscribeParams, RunParams,
    ThreadReadParams, ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse,
    ThreadStartParams, ThreadStartResponse, TurnInterruptParams, TurnInterruptResponse,
    TurnStartParams, TurnStartResponse,
};
use crate::app_server::ThreadManager;
use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::llm::LlmClient;
use crate::registry::ToolRegistry;
use crate::resolver::ModelResolver;

#[async_trait]
pub trait AppServerBoundary: Send + Sync {
    async fn run(&self, params: RunParams) -> Result<AgentRunResponse>;
    async fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse>;
    async fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse>;
    async fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse>;
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

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        self.thread_manager.thread_resume(params)
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
        self.thread_manager.events_subscribe(params)
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

    async fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        self.thread_resume(params)
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
