use anyhow::Result;
use async_trait::async_trait;

use crate::app_server::protocol::{
    AgentRunResponse, BoundaryOp, BoundaryOpResponse, CollectParams, CollectResponse,
    EventsReplayParams, EventsReplayResponse, ForkParams, InspectParams, InspectResponse,
    RunParams, ThreadReadParams, ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse,
    ThreadSpawnChildParams, ThreadSpawnChildResponse, ThreadStartParams, ThreadStartResponse,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use crate::app_server::ThreadManager;
use crate::config::AgentConfig;
use crate::llm::LlmClient;
use crate::registry::ToolRegistry;

#[async_trait]
pub trait AppServerBoundary: Send + Sync {
    async fn run(&self, params: RunParams) -> Result<AgentRunResponse>;
    async fn fork(&self, params: ForkParams) -> Result<AgentRunResponse>;
    async fn inspect(&self, params: InspectParams) -> Result<InspectResponse>;
    async fn collect(&self, params: CollectParams) -> Result<CollectResponse>;
    async fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse>;
    async fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse>;
    async fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse>;
    async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse>;
    async fn turn_interrupt(&self, params: TurnInterruptParams) -> Result<TurnInterruptResponse>;
    async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse>;
    async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse>;
    async fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse>;
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

    pub fn with_llm<F>(config: AgentConfig, llm: Box<dyn LlmClient>, registry_factory: F) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            thread_manager: ThreadManager::with_llm(config, llm, registry_factory),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        self.thread_manager.run(params).await
    }

    pub async fn fork(&self, params: ForkParams) -> Result<AgentRunResponse> {
        self.thread_manager.fork(params).await
    }

    pub fn inspect(&self, params: InspectParams) -> Result<InspectResponse> {
        self.thread_manager.inspect(params)
    }

    pub fn collect(&self, params: CollectParams) -> Result<CollectResponse> {
        self.thread_manager.collect(params)
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

    pub async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse> {
        self.thread_manager.thread_spawn_child(params).await
    }

    pub async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        self.thread_manager.submit_boundary_op(op).await
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        self.thread_manager.events_replay(params)
    }
}

#[async_trait]
impl AppServerBoundary for AppServerService {
    async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        self.run(params).await
    }

    async fn fork(&self, params: ForkParams) -> Result<AgentRunResponse> {
        self.fork(params).await
    }

    async fn inspect(&self, params: InspectParams) -> Result<InspectResponse> {
        self.inspect(params)
    }

    async fn collect(&self, params: CollectParams) -> Result<CollectResponse> {
        self.collect(params)
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

    async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse> {
        self.thread_spawn_child(params).await
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        self.submit_boundary_op(op).await
    }

    async fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        self.events_replay(params)
    }
}
