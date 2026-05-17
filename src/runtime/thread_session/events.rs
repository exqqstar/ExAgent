use super::ThreadSession;

use anyhow::Result;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::types::{EventId, TurnId};

impl ThreadSession {
    pub(crate) fn append_and_broadcast(
        &mut self,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event = RuntimeEvent {
            event_id: EventId::new(format!("evt_{}", self.next_event_index)),
            session_id: self.thread_id.clone(),
            turn_id: turn_id.cloned(),
            kind,
        };
        self.next_event_index += 1;
        crate::transcript::append_json_line(&self.paths.events_path, &event)?;
        self.events.push(event.clone());
        if let Ok(mut live_state) = self.live_state.lock() {
            live_state.events = self.events.clone();
        }
        let _ = self.event_tx.send(event.clone());
        Ok(event)
    }
}
