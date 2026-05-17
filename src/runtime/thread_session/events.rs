use super::ThreadSession;

use anyhow::Result;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::types::{SessionId, TurnId};

impl ThreadSession {
    pub(crate) fn append_and_broadcast(
        &self,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event = crate::transcript::append_runtime_event(
            &self.config.workspace_root,
            &self.thread_id,
            turn_id,
            kind,
        )?;
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
