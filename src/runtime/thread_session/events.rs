use super::{ThreadSession, ThreadSessionLiveState};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::broadcast;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::session::SessionSnapshot;
use crate::types::{EventId, SessionId, TurnId};

/// Records a runtime event into the durable event log, updates the in-memory
/// live mirror, and broadcasts to subscribers. Used both by ThreadSession
/// (for lifecycle events) and by Agent (for assistant/tool events) so a
/// loaded thread has exactly one event-emitting pipeline.
pub(crate) trait LiveEventSink: Send {
    fn record(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent>;
}

pub(crate) struct ThreadEventRecorder {
    thread_id: SessionId,
    snapshot_path: PathBuf,
    events_path: PathBuf,
    next_event_index: usize,
    events: Vec<RuntimeEvent>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    live_state: Arc<Mutex<ThreadSessionLiveState>>,
}

impl ThreadEventRecorder {
    pub(crate) fn new(
        thread_id: SessionId,
        snapshot_path: PathBuf,
        events_path: PathBuf,
        events: Vec<RuntimeEvent>,
        event_tx: broadcast::Sender<RuntimeEvent>,
        live_state: Arc<Mutex<ThreadSessionLiveState>>,
    ) -> Self {
        let next_event_index = events.len() + 1;
        Self {
            thread_id,
            snapshot_path,
            events_path,
            next_event_index,
            events,
            event_tx,
            live_state,
        }
    }

    pub(crate) fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }
}

impl LiveEventSink for ThreadEventRecorder {
    fn record(
        &mut self,
        snapshot: &SessionSnapshot,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        // Pair the snapshot checkpoint with the event so durable state never
        // lags behind what subscribers observe: persist snapshot, publish it
        // to the live mirror, then write/broadcast the event itself.
        crate::transcript::write_json(&self.snapshot_path, snapshot)?;
        if let Ok(mut state) = self.live_state.lock() {
            state.snapshot = snapshot.clone();
        }

        let event = RuntimeEvent {
            event_id: EventId::new(format!("evt_{}", self.next_event_index)),
            session_id: self.thread_id.clone(),
            turn_id: turn_id.cloned(),
            kind,
        };
        self.next_event_index += 1;
        crate::transcript::append_json_line(&self.events_path, &event)?;
        self.events.push(event.clone());
        if let Ok(mut state) = self.live_state.lock() {
            state.events = self.events.clone();
        }
        let _ = self.event_tx.send(event.clone());
        Ok(event)
    }
}

impl ThreadSession {
    pub(crate) fn append_and_broadcast(
        &mut self,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        self.recorder.record(&self.snapshot, turn_id, kind)
    }
}
