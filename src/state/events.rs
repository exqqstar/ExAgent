use serde::{Deserialize, Serialize};

use crate::session::{ApprovalId, ApprovalStatus, CompactionSummary, ExecSessionId};
use crate::types::{AssistantTurn, EventId, ThreadId, TokenUsageInfo, ToolResult, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeEvent {
    pub event_id: EventId,
    pub thread_id: ThreadId,
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
    TokenCount {
        info: Option<TokenUsageInfo>,
    },
    RuntimeError {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TokenUsage;

    #[test]
    fn token_count_event_serializes_as_snake_case_variant() {
        let value = serde_json::to_value(RuntimeEventKind::TokenCount {
            info: Some(TokenUsageInfo {
                total_token_usage: TokenUsage {
                    total_tokens: 100,
                    ..TokenUsage::default()
                },
                last_token_usage: TokenUsage {
                    total_tokens: 25,
                    ..TokenUsage::default()
                },
                model_context_window: Some(1_000),
            }),
        })
        .expect("serialize token count event");

        assert_eq!(value["type"], "token_count");
        assert_eq!(value["info"]["total_token_usage"]["total_tokens"], 100);
        assert_eq!(value["info"]["last_token_usage"]["total_tokens"], 25);
        assert_eq!(value["info"]["model_context_window"], 1_000);
    }
}
