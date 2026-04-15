use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::{ConversationMessage, EventId, SessionId};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
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
pub struct SessionSnapshot {
    pub session_id: SessionId,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation: Vec<ConversationMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_exec_sessions: Vec<ExecSessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<CompactionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approvals: Vec<PendingApproval>,
}
