use super::ThreadSession;

use anyhow::Result;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::types::EventId;
use crate::types::{SessionId, TurnId};

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
        let _ = self.event_tx.send(event.clone());
        Ok(event)
    }

    pub(crate) fn broadcast_events_since(&self, event_count: usize) -> Result<()> {
        for event in
            crate::transcript::read_session_events(&self.config.workspace_root, &self.thread_id)?
                .into_iter()
                .skip(event_count)
        {
            let _ = self.event_tx.send(event);
        }
        Ok(())
    }

    pub(crate) fn persisted_event_count(&self, session_id: &SessionId) -> Result<usize> {
        Ok(crate::transcript::read_session_events(&self.config.workspace_root, session_id)?.len())
    }
}
