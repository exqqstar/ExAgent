pub mod events;
pub mod turn;

use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult, ThreadRuntimeStatus};
use crate::session::SessionSnapshot;
use crate::transcript::SessionPaths;
use crate::types::{SessionId, TurnId};

const DEFAULT_THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;

pub(crate) struct RuntimeInterrupt {
    pub(crate) interrupt_rx: oneshot::Receiver<()>,
    pub(crate) interrupted: Arc<Notify>,
}

pub struct ThreadSessionOptions {
    thread_id: SessionId,
    config: AgentConfig,
    agent_factory: AgentFactory,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
}

impl ThreadSessionOptions {
    pub fn new(thread_id: SessionId, config: AgentConfig, agent_factory: AgentFactory) -> Self {
        let (event_tx, _) = broadcast::channel(DEFAULT_THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, _) = watch::channel(ThreadRuntimeStatus::Idle);
        Self {
            thread_id,
            config,
            agent_factory,
            event_tx,
            status_tx,
        }
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
    agent: Agent,
    snapshot: SessionSnapshot,
    paths: SessionPaths,
    next_event_index: usize,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
}

impl ThreadSession {
    pub fn new(options: ThreadSessionOptions) -> anyhow::Result<Self> {
        let ThreadSessionOptions {
            thread_id,
            config,
            agent_factory,
            event_tx,
            status_tx,
        } = options;
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        let mut snapshot: SessionSnapshot = crate::transcript::read_json(&paths.snapshot_path)?;
        snapshot.normalize_lineage();
        let next_event_index =
            crate::transcript::read_json_lines::<RuntimeEvent>(&paths.events_path)?.len() + 1;
        let agent = agent_factory(config.clone())?;

        Ok(Self {
            thread_id,
            config,
            agent,
            snapshot,
            paths,
            next_event_index,
            event_tx,
            status_tx,
        })
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
