pub mod events;
pub mod turn;

use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult, ThreadRuntimeStatus};
use crate::types::{SessionId, TurnId};

pub(crate) struct RuntimeInterrupt {
    pub(crate) interrupt_rx: oneshot::Receiver<()>,
    pub(crate) interrupted: Arc<Notify>,
}

pub struct ThreadSessionOptions {
    pub thread_id: SessionId,
    pub config: AgentConfig,
    pub agent_factory: Option<AgentFactory>,
    pub event_tx: broadcast::Sender<RuntimeEvent>,
    pub status_tx: watch::Sender<ThreadRuntimeStatus>,
}

impl ThreadSessionOptions {
    pub fn new(thread_id: SessionId, config: AgentConfig) -> Self {
        let (event_tx, _) = broadcast::channel(1);
        let (status_tx, _) = watch::channel(ThreadRuntimeStatus::Idle);
        Self {
            thread_id,
            config,
            agent_factory: None,
            event_tx,
            status_tx,
        }
    }

    pub fn with_agent_factory(mut self, agent_factory: Option<AgentFactory>) -> Self {
        self.agent_factory = agent_factory;
        self
    }

    pub fn with_event_tx(mut self, event_tx: broadcast::Sender<RuntimeEvent>) -> Self {
        self.event_tx = event_tx;
        self
    }

    pub fn with_status_tx(mut self, status_tx: watch::Sender<ThreadRuntimeStatus>) -> Self {
        self.status_tx = status_tx;
        self
    }
}

pub struct ThreadSession {
    thread_id: SessionId,
    config: AgentConfig,
    agent_factory: Option<AgentFactory>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
}

impl ThreadSession {
    pub fn new(options: ThreadSessionOptions) -> Self {
        Self {
            thread_id: options.thread_id,
            config: options.config,
            agent_factory: options.agent_factory,
            event_tx: options.event_tx,
            status_tx: options.status_tx,
        }
    }

    pub fn thread_id(&self) -> &SessionId {
        &self.thread_id
    }

    pub fn mark_stopped(&self) {
        let _ = self.status_tx.send(ThreadRuntimeStatus::Stopped);
    }

    pub(crate) fn set_status(&self, status: ThreadRuntimeStatus) {
        let _ = self.status_tx.send(status);
    }

    pub(crate) async fn handle_interrupt(
        &self,
        turn_id: Option<TurnId>,
    ) -> anyhow::Result<ThreadOpResult> {
        turn_id
            .map(|turn_id| ThreadOpResult::Interrupted { turn_id })
            .ok_or_else(|| anyhow::anyhow!("thread has no active turn"))
    }
}
