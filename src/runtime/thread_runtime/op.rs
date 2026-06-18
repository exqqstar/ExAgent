use std::path::PathBuf;

use crate::config::ThinkingMode;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::goal::runtime::GoalRuntimeEffect;
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ApprovalId, ApprovalStatus};
use crate::types::{AssistantTurn, ThreadId, TurnId, UserInput};

#[derive(Debug, thiserror::Error)]
pub enum ThreadRuntimeError {
    #[error("thread is busy: {}", .0.as_str())]
    ThreadBusy(ThreadId),

    #[error("turn rejected for thread {}: {reason}", thread_id.as_str())]
    TurnRejected { thread_id: ThreadId, reason: String },

    #[error("turn interrupted for thread {}: {}", thread_id.as_str(), turn_id.as_str())]
    TurnInterrupted {
        thread_id: ThreadId,
        turn_id: TurnId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRuntimeStatus {
    Idle,
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
    pub resolved_model: Option<ResolvedModelConfig>,
    pub thinking_mode: Option<ThinkingMode>,
    pub clear_thinking_mode: bool,
    pub turn_mode: TurnMode,
}

#[derive(Debug, Clone)]
pub(super) enum ThreadOp {
    UserInput {
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
    },
    GoalContinuation {
        turn_id: TurnId,
        goal_id: String,
    },
    Interrupt {
        turn_id: Option<TurnId>,
    },
    ApprovalDecision {
        turn_id: Option<TurnId>,
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    },
    SubmitUserInput {
        turn_id: Option<TurnId>,
        request_id: ApprovalId,
        dismissed: bool,
    },
    GoalRuntimeEffect {
        effect: GoalRuntimeEffect,
    },
    ManualCompaction,
    Shutdown,
}

pub enum ThreadOpResult {
    UserInput {
        turn_id: TurnId,
        final_turn: AssistantTurn,
    },
    Interrupted {
        turn_id: TurnId,
    },
    ApprovalDecision {
        turn_id: TurnId,
        approval_id: ApprovalId,
        status: ApprovalStatus,
    },
    UserInputSubmitted {
        turn_id: TurnId,
        request_id: ApprovalId,
        dismissed: bool,
    },
    Ack,
}
