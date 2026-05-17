pub mod events;
pub mod turn;

pub(crate) use events::{LiveEventSink, ThreadEventRecorder};

use std::sync::{Arc, RwLock};

use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::policy::PolicyManager;
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus,
};
use crate::session::{ApprovalStatus, SessionSnapshot};
use crate::types::{SessionId, TurnId};

const DEFAULT_THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;
const DEFAULT_LIVE_EVENT_BUFFER_CAP: usize = 2048;

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
    policy: Arc<PolicyManager>,
    live_event_buffer_cap: usize,
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
            policy: Arc::new(PolicyManager::default()),
            live_event_buffer_cap: DEFAULT_LIVE_EVENT_BUFFER_CAP,
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

    pub fn with_policy(mut self, policy: Arc<PolicyManager>) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_live_event_buffer_cap(mut self, cap: usize) -> Self {
        self.live_event_buffer_cap = cap.max(1);
        self
    }
}

pub struct ThreadSession {
    thread_id: SessionId,
    agent: Agent,
    recorder: ThreadEventRecorder,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
    policy: Arc<PolicyManager>,
}

#[derive(Debug, Clone)]
pub struct ThreadSessionLiveView {
    pub thread_id: SessionId,
    pub snapshot: SessionSnapshot,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}

/// Shared, lock-protected publication surface for live thread state.
///
/// Readers (thread_read, live_view) take a read lock and clone out what they
/// need. The actor loop holds the only writer: each event emitted by
/// `ThreadEventRecorder` republishes snapshot + events + status atomically
/// behind the write lock, so the publication never lags by more than one
/// event.
#[derive(Debug)]
pub(crate) struct ThreadSessionLiveState {
    pub(crate) snapshot: SessionSnapshot,
    pub(crate) events: Vec<RuntimeEvent>,
    pub(crate) status: ThreadRuntimeStatus,
}

/// Marks the thread as stopped from `Drop`, so the runtime loop reports
/// termination even if a handler panics and the explicit end-of-loop path is
/// skipped.
pub(crate) struct ThreadSessionStoppedGuard {
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
}

impl Drop for ThreadSessionStoppedGuard {
    fn drop(&mut self) {
        if let Ok(mut live_state) = self.live_state.write() {
            live_state.status = ThreadRuntimeStatus::Stopped;
        }
        let _ = self.status_tx.send(ThreadRuntimeStatus::Stopped);
    }
}

impl ThreadSession {
    pub fn new(options: ThreadSessionOptions) -> anyhow::Result<Self> {
        let ThreadSessionOptions {
            thread_id,
            config,
            agent_factory,
            event_tx,
            status_tx,
            policy,
            live_event_buffer_cap,
        } = options;
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        let mut snapshot: SessionSnapshot = crate::transcript::read_json(&paths.snapshot_path)?;
        snapshot.normalize_lineage();
        let mut events = crate::transcript::read_json_lines::<RuntimeEvent>(&paths.events_path)?;
        let live_event_buffer_cap = live_event_buffer_cap.max(1);
        let next_event_index = events.len() + 1;
        let overflow = events.len().saturating_sub(live_event_buffer_cap);
        if overflow > 0 {
            events.drain(0..overflow);
        }
        let agent = agent_factory(config.clone())?;
        let live_state = Arc::new(RwLock::new(ThreadSessionLiveState {
            snapshot: snapshot.clone(),
            events,
            status: ThreadRuntimeStatus::Idle,
        }));
        let recorder = ThreadEventRecorder::new(
            thread_id.clone(),
            paths.snapshot_path,
            paths.events_path,
            event_tx,
            live_state.clone(),
            next_event_index,
            live_event_buffer_cap,
        );

        Ok(Self {
            thread_id,
            agent,
            recorder,
            status_tx,
            live_state,
            policy,
        })
    }

    pub fn thread_id(&self) -> &SessionId {
        &self.thread_id
    }

    pub(crate) fn live_state_handle(&self) -> Arc<RwLock<ThreadSessionLiveState>> {
        self.live_state.clone()
    }

    pub(crate) fn stopped_guard(&self) -> ThreadSessionStoppedGuard {
        ThreadSessionStoppedGuard {
            status_tx: self.status_tx.clone(),
            live_state: self.live_state.clone(),
        }
    }

    pub(crate) fn set_status(&self, status: ThreadRuntimeStatus) {
        if let Ok(mut live_state) = self.live_state.write() {
            live_state.status = status;
        }
        let _ = self.status_tx.send(status);
    }

    pub(crate) fn live_view_from_state(
        thread_id: SessionId,
        state: &Arc<RwLock<ThreadSessionLiveState>>,
    ) -> ThreadSessionLiveView {
        let state = state
            .read()
            .expect("thread session live state rwlock poisoned");
        ThreadSessionLiveView {
            thread_id,
            snapshot: state.snapshot.clone(),
            events: state.events.clone(),
            status: state.status,
        }
    }

    pub(crate) fn next_turn_id_from_state(state: &Arc<RwLock<ThreadSessionLiveState>>) -> TurnId {
        let state = state
            .read()
            .expect("thread session live state rwlock poisoned");
        let assistant_turn_count = state
            .snapshot
            .conversation
            .iter()
            .filter(|message| matches!(message.role, crate::types::MessageRole::Assistant))
            .count();
        TurnId::new(format!("turn_{}", assistant_turn_count + 1))
    }

    pub(crate) async fn handle_interrupt(
        &mut self,
        turn_id: Option<TurnId>,
    ) -> anyhow::Result<ThreadOpResult> {
        let (interrupted_turn_id, snapshot) = {
            let mut state = self
                .live_state
                .write()
                .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
            let has_pending_approval = state
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

            let latest_turn_id = state
                .events
                .iter()
                .rev()
                .find_map(|event| event.turn_id.clone());
            let interrupted_turn_id = turn_id.or(latest_turn_id.clone()).ok_or_else(|| {
                ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: "waiting approval has no turn id".to_string(),
                }
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

            state
                .snapshot
                .pending_approvals
                .retain(|approval| !matches!(approval.status, ApprovalStatus::Pending));
            (interrupted_turn_id, state.snapshot.clone())
        };
        self.policy
            .cancel_pending_for_session(&self.thread_id)
            .await;
        // append_and_broadcast checkpoints the snapshot atomically with the
        // event, so a separate pre-event checkpoint is no longer needed.
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&interrupted_turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;

        Ok(ThreadOpResult::Interrupted {
            turn_id: interrupted_turn_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::llm::MockLlm;
    use crate::policy::PolicyManager;
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::AgentFactory;
    use crate::session::{ApprovalId, ApprovalStatus, PendingApproval};
    use crate::types::EventId;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[tokio::test]
    async fn handle_interrupt_cancels_pending_policy_approvals() {
        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_interrupt_policy");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };

        let mut snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        snapshot.pending_approvals.push(PendingApproval {
            approval_id: ApprovalId::new("approval_test_1"),
            requested_event_id: EventId::new("approval_evt_test_1"),
            tool_name: "run_command".to_string(),
            reason: "policy requires review".to_string(),
            status: ApprovalStatus::Pending,
        });
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();

        let policy = Arc::new(PolicyManager::default());
        let _registered = policy
            .create_command_approval(
                thread_id.clone(),
                "run_command",
                "rm -rf scratch",
                PathBuf::from("/tmp"),
                None,
                false,
                "policy requires review".to_string(),
            )
            .await;
        assert_eq!(
            policy.pending_count_for_session(&thread_id).await,
            1,
            "precondition: policy holds one pending approval"
        );

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(
            ThreadSessionOptions::new(thread_id.clone(), config, agent_factory)
                .with_policy(policy.clone()),
        )
        .expect("create thread session");

        let turn_id = TurnId::new("turn_approval_1");
        let result = session
            .handle_interrupt(Some(turn_id.clone()))
            .await
            .expect("handle_interrupt should succeed when pending approval exists");
        assert!(matches!(
            result,
            ThreadOpResult::Interrupted { turn_id: ref tid } if tid == &turn_id
        ));

        assert_eq!(
            policy.pending_count_for_session(&thread_id).await,
            0,
            "interrupt must drop the policy-side approval waiter, not just the snapshot copy"
        );
    }

    #[test]
    fn session_load_bounds_live_events_without_reusing_event_ids() {
        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_load_bounded_events");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
        for event_index in 1..=4 {
            crate::transcript::append_json_line(
                &paths.events_path,
                &RuntimeEvent {
                    event_id: EventId::new(format!("evt_{}", event_index)),
                    session_id: thread_id.clone(),
                    turn_id: Some(TurnId::new(format!("turn_{}", event_index))),
                    kind: RuntimeEventKind::TurnStarted,
                },
            )
            .unwrap();
        }

        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(
            ThreadSessionOptions::new(thread_id.clone(), config.clone(), agent_factory)
                .with_live_event_buffer_cap(2),
        )
        .expect("create thread session");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(live_view.events.len(), 2);
        assert_eq!(live_view.events[0].event_id, EventId::new("evt_3"));
        assert_eq!(live_view.events[1].event_id, EventId::new("evt_4"));

        let event = session
            .append_and_broadcast_snapshot(
                &snapshot,
                Some(&TurnId::new("turn_5")),
                RuntimeEventKind::TurnStarted,
            )
            .expect("record next event");
        assert_eq!(event.event_id, EventId::new("evt_5"));

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(live_view.events.len(), 2);
        assert_eq!(live_view.events[0].event_id, EventId::new("evt_4"));
        assert_eq!(live_view.events[1].event_id, EventId::new("evt_5"));

        let replay = crate::transcript::read_session_events(&config.workspace_root, &thread_id)
            .expect("read replay events");
        assert_eq!(replay.len(), 5);
    }
}
