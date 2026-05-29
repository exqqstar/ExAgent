use thiserror::Error;

use crate::types::ThreadId;

#[derive(Debug, Error)]
pub enum AppServerError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("thread not found: {}", .0.as_str())]
    ThreadNotFound(ThreadId),

    #[error("thread is busy: {}", .0.as_str())]
    ThreadBusy(ThreadId),

    #[error("turn rejected for thread {}: {reason}", thread_id.as_str())]
    TurnRejected { thread_id: ThreadId, reason: String },

    #[error("turn interrupted for thread {}: {}", thread_id.as_str(), turn_id.as_str())]
    TurnInterrupted {
        thread_id: ThreadId,
        turn_id: crate::types::TurnId,
    },
}
