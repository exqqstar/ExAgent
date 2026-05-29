use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::policy::PolicyMode;
use crate::types::{ConversationMessage, EventId, ThreadId};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(ExecSessionId);
string_id!(ApprovalId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecSessionStatus {
    Running,
    Exited,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecSessionRef {
    pub exec_session_id: ExecSessionId,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ExecSessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionSummary {
    pub summary: String,
    pub source_event_ids: Vec<EventId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    pub requested_event_id: EventId,
    pub tool_name: String,
    pub reason: String,
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSnapshot {
    pub thread_id: ThreadId,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_turn_context: Option<TurnContextItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation: Vec<ConversationMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_exec_sessions: Vec<ExecSessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<CompactionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approvals: Vec<PendingApproval>,
}

impl ThreadSnapshot {
    pub fn new_thread(thread_id: ThreadId, workspace_root: PathBuf, cwd: PathBuf) -> Self {
        Self {
            thread_id,
            workspace_root,
            cwd,
            reference_turn_context: None,
            conversation: vec![],
            open_exec_sessions: vec![],
            latest_compaction: None,
            pending_approvals: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnContextItem {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub model: String,
    pub policy_mode: PolicyMode,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    #[serde(
        default,
        alias = "current_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_utc_date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_deserializes_without_reference_turn_context() {
        let snapshot: ThreadSnapshot = serde_json::from_value(json!({
            "thread_id": "session_old",
            "workspace_root": "/tmp/workspace",
            "cwd": "/tmp/workspace"
        }))
        .expect("deserialize legacy snapshot");

        assert_eq!(snapshot.thread_id, ThreadId::new("session_old"));
        assert!(snapshot.reference_turn_context.is_none());
        assert!(snapshot.conversation.is_empty());
    }
}
