use super::{ThreadSession, ThreadSessionLiveState};

use std::sync::{Arc, RwLock};

use anyhow::Result;
use tokio::sync::broadcast;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::runtime::tool_call_runtime::ExecSessionUpdate;
use crate::session::ApprovalId;
use crate::session::SessionSnapshot;
use crate::state::rollout::{RolloutItem, RolloutStore};
use crate::types::{EventId, SessionId, TurnId};

/// Records a runtime event into the durable event log, updates the in-memory
/// live mirror, and broadcasts to subscribers. Used both by ThreadSession
/// (for lifecycle events) and by Agent (for assistant/tool events) so a
/// loaded thread has exactly one event-emitting pipeline.
pub(crate) trait LiveEventSink: Send {
    fn reserve_event_id(&mut self) -> EventId;

    fn record_reserved(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        event_id: EventId,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent>;

    fn record(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event_id = self.reserve_event_id();
        self.record_reserved(snapshot, turn_id, event_id, kind)
    }
}

pub(crate) struct ThreadEventRecorder {
    thread_id: SessionId,
    rollout_store: RolloutStore,
    next_event_index: usize,
    event_tx: broadcast::Sender<RuntimeEvent>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
    live_event_buffer_cap: usize,
}

impl ThreadEventRecorder {
    pub(crate) fn new(
        thread_id: SessionId,
        rollout_store: RolloutStore,
        event_tx: broadcast::Sender<RuntimeEvent>,
        live_state: Arc<RwLock<ThreadSessionLiveState>>,
        next_event_index: usize,
        live_event_buffer_cap: usize,
    ) -> Self {
        Self {
            thread_id,
            rollout_store,
            next_event_index,
            event_tx,
            live_state,
            live_event_buffer_cap: live_event_buffer_cap.max(1),
        }
    }

    fn record_snapshot(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        event_id: EventId,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event = RuntimeEvent {
            event_id,
            session_id: self.thread_id.clone(),
            turn_id: turn_id.cloned(),
            kind,
        };
        self.rollout_store
            .append_items_blocking(&[RolloutItem::EventMsg(event.clone())])?;
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state.snapshot = snapshot.clone();
        state.events.push(event.clone());
        let overflow = state
            .events
            .len()
            .saturating_sub(self.live_event_buffer_cap);
        if overflow > 0 {
            state.events.drain(0..overflow);
        }
        drop(state);
        let _ = self.event_tx.send(event.clone());
        Ok(event)
    }

    pub(crate) fn apply_exec_session_update(&mut self, update: ExecSessionUpdate) -> Result<()> {
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state.overlay.apply_exec_session_update(update);
        Ok(())
    }

    pub(crate) fn apply_approval_requested(
        &mut self,
        approval_id: ApprovalId,
        requested_event_id: EventId,
        tool_name: String,
        reason: String,
    ) -> Result<()> {
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state
            .overlay
            .apply_approval_requested(approval_id, requested_event_id, tool_name, reason);
        Ok(())
    }

    pub(crate) fn clear_approval(&mut self, approval_id: &ApprovalId) -> Result<()> {
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state.overlay.clear_approval(approval_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    use crate::config::AgentConfig;
    use crate::session::SessionSnapshot;

    #[test]
    fn recorder_bounds_live_buffer_without_trimming_persisted_events() {
        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_bounded_live_events");
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
        let rollout_paths =
            crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
        let rollout_store = RolloutStore::new(rollout_paths.rollout_path);
        let (event_tx, _) = broadcast::channel(16);
        let live_state = Arc::new(RwLock::new(ThreadSessionLiveState {
            snapshot: snapshot.clone(),
            overlay: crate::runtime::thread_session::RuntimeOverlay::default(),
            events: vec![],
            status: crate::runtime::thread_runtime::ThreadRuntimeStatus::Idle,
        }));
        let mut recorder = ThreadEventRecorder::new(
            thread_id.clone(),
            rollout_store.clone(),
            event_tx,
            live_state.clone(),
            1,
            2,
        );

        for turn_index in 1..=4 {
            recorder
                .record(
                    &snapshot,
                    Some(&TurnId::new(format!("turn_{}", turn_index))),
                    RuntimeEventKind::TurnStarted,
                )
                .expect("record event");
        }

        let live_events = live_state.read().expect("read live state").events.clone();
        assert_eq!(live_events.len(), 2);
        assert_eq!(live_events[0].event_id, EventId::new("evt_3"));
        assert_eq!(live_events[1].event_id, EventId::new("evt_4"));

        let rollout_items =
            RolloutStore::read_items_blocking(rollout_store.path()).expect("read rollout items");
        let replay = rollout_items
            .into_iter()
            .filter_map(|item| match item {
                RolloutItem::EventMsg(event) => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(replay.len(), 4);
        assert_eq!(replay[0].event_id, EventId::new("evt_1"));
        assert_eq!(replay[3].event_id, EventId::new("evt_4"));
    }
}

impl LiveEventSink for ThreadEventRecorder {
    fn reserve_event_id(&mut self) -> EventId {
        let event_id = EventId::new(format!("evt_{}", self.next_event_index));
        self.next_event_index += 1;
        event_id
    }

    fn record_reserved(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        event_id: EventId,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        self.record_snapshot(snapshot, turn_id, event_id, kind)
    }
}

impl ThreadSession {
    pub(crate) fn append_and_broadcast_snapshot(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event_id = self.recorder.reserve_event_id();
        self.recorder
            .record_snapshot(snapshot, turn_id, event_id, kind)
    }
}
