use serde::{Deserialize, Serialize};

use crate::result_contract::StructuredSessionResult;
use crate::session::{AgentRole, ApprovalId, ApprovalStatus, CompactionSummary, ExecSessionId};
use crate::types::{AssistantTurn, EventId, SessionId, ToolResult, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub kind: RuntimeEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    TurnStarted,
    TurnCompleted,
    TurnInterrupted,
    AssistantTurn {
        turn: AssistantTurn,
    },
    ToolResult {
        result: ToolResult,
    },
    SessionSpawned {
        child_session_id: SessionId,
        parent_session_id: SessionId,
        agent_role: AgentRole,
        spawned_by_turn_id: Option<TurnId>,
    },
    ExecOutput {
        exec_session_id: ExecSessionId,
        stream: ExecOutputStream,
        chunk: String,
    },
    ApprovalRequested {
        approval_id: ApprovalId,
        tool_name: String,
        reason: String,
    },
    ApprovalDecision {
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    },
    CompactionWritten {
        summary: CompactionSummary,
    },
    StructuredResultRecorded {
        result: StructuredSessionResult,
    },
    RuntimeError {
        message: String,
    },
}
