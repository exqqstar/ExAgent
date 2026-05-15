use thiserror::Error;

use crate::types::SessionId;

#[derive(Debug, Error)]
pub enum AppServerError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("thread not found: {}", .0.as_str())]
    ThreadNotFound(SessionId),

    #[error("thread is busy: {}", .0.as_str())]
    ThreadBusy(SessionId),

    #[error("turn rejected for thread {}: {reason}", thread_id.as_str())]
    TurnRejected {
        thread_id: SessionId,
        reason: String,
    },

    #[error("turn interrupted for thread {}: {}", thread_id.as_str(), turn_id.as_str())]
    TurnInterrupted {
        thread_id: SessionId,
        turn_id: crate::types::TurnId,
    },
}
