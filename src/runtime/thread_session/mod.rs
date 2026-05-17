pub mod events;
pub mod turn;

use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus,
};
use crate::session::{ApprovalStatus, SessionSnapshot};
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
    agent: Agent,
    snapshot: SessionSnapshot,
    paths: SessionPaths,
    events: Vec<RuntimeEvent>,
    next_event_index: usize,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    live_state: Arc<Mutex<ThreadSessionLiveState>>,
}

#[derive(Debug, Clone)]
pub struct ThreadSessionLiveView {
    pub thread_id: SessionId,
    pub snapshot: SessionSnapshot,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadSessionLiveState {
    snapshot: SessionSnapshot,
    events: Vec<RuntimeEvent>,
    status: ThreadRuntimeStatus,
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
        let events = crate::transcript::read_json_lines::<RuntimeEvent>(&paths.events_path)?;
        let next_event_index = events.len() + 1;
        let agent = agent_factory(config.clone())?;
        let live_state = Arc::new(Mutex::new(ThreadSessionLiveState {
            snapshot: snapshot.clone(),
            events: events.clone(),
            status: ThreadRuntimeStatus::Idle,
        }));

        Ok(Self {
            thread_id,
            agent,
            snapshot,
            paths,
            events,
            next_event_index,
            event_tx,
            status_tx,
            live_state,
        })
    }

    pub fn thread_id(&self) -> &SessionId {
        &self.thread_id
    }

    pub(crate) fn live_state_handle(&self) -> Arc<Mutex<ThreadSessionLiveState>> {
        self.live_state.clone()
    }

    pub fn mark_stopped(&self) {
        self.set_status(ThreadRuntimeStatus::Stopped);
    }

    pub(crate) fn set_status(&self, status: ThreadRuntimeStatus) {
        if let Ok(mut live_state) = self.live_state.lock() {
            live_state.status = status;
        }
        let _ = self.status_tx.send(status);
    }

    pub(crate) fn checkpoint_snapshot(&self) -> anyhow::Result<()> {
        crate::transcript::write_json(&self.paths.snapshot_path, &self.snapshot)?;
        self.publish_live_snapshot();
        Ok(())
    }

    fn publish_live_snapshot(&self) {
        if let Ok(mut live_state) = self.live_state.lock() {
            live_state.snapshot = self.snapshot.clone();
        }
    }

    pub(crate) fn live_view_from_state(
        thread_id: SessionId,
        state: &Arc<Mutex<ThreadSessionLiveState>>,
    ) -> anyhow::Result<ThreadSessionLiveView> {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("thread session live state mutex poisoned"))?;
        Ok(ThreadSessionLiveView {
            thread_id,
            snapshot: state.snapshot.clone(),
            events: state.events.clone(),
            status: state.status,
        })
    }

    pub(crate) async fn handle_interrupt(
        &mut self,
        turn_id: Option<TurnId>,
    ) -> anyhow::Result<ThreadOpResult> {
        let has_pending_approval = self
            .snapshot
            .pending_approvals
            .iter()
            .any(|approval| matches!(approval.status, ApprovalStatus::Pending));
        if !has_pending_approval {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "thread has no active turn".to_string(),
            }
            .into());
        }

        let latest_turn_id = self
            .events
            .iter()
            .rev()
            .find_map(|event| event.turn_id.clone());
        let interrupted_turn_id =
            turn_id
                .or(latest_turn_id.clone())
                .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: "waiting approval has no turn id".to_string(),
                })?;
        if let Some(latest_turn_id) = latest_turn_id {
            if latest_turn_id != interrupted_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("waiting approval turn is {}", latest_turn_id.as_str()),
                }
                .into());
            }
        }

        self.snapshot
            .pending_approvals
            .retain(|approval| !matches!(approval.status, ApprovalStatus::Pending));
        self.checkpoint_snapshot()?;
        self.append_and_broadcast(
            Some(&interrupted_turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;

        Ok(ThreadOpResult::Interrupted {
            turn_id: interrupted_turn_id,
        })
    }
}
