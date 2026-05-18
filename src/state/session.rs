use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::types::{ConversationMessage, EventId, SessionId, TurnId};

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
pub enum AgentRole {
    Primary,
    Spec,
    Test,
    Judge,
    Implementation,
}

impl Default for AgentRole {
    fn default() -> Self {
        Self::Primary
    }
}

impl FromStr for AgentRole {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "primary" => Ok(Self::Primary),
            "spec" => Ok(Self::Spec),
            "test" => Ok(Self::Test),
            "judge" => Ok(Self::Judge),
            "implementation" => Ok(Self::Implementation),
            _ => Err(anyhow::anyhow!("unknown agent role: {value}")),
        }
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    #[serde(default)]
    pub root_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by_turn_id: Option<TurnId>,
    #[serde(default)]
    pub agent_role: AgentRole,
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

impl SessionSnapshot {
    pub fn new_root(
        session_id: SessionId,
        workspace_root: PathBuf,
        cwd: PathBuf,
        user_prompt: impl Into<String>,
    ) -> Self {
        Self {
            root_session_id: session_id.clone(),
            session_id,
            parent_session_id: None,
            spawned_by_turn_id: None,
            agent_role: AgentRole::Primary,
            workspace_root,
            cwd,
            conversation: vec![ConversationMessage::user(user_prompt)],
            open_exec_sessions: vec![],
            latest_compaction: None,
            pending_approvals: vec![],
        }
    }

    pub fn new_thread(session_id: SessionId, workspace_root: PathBuf, cwd: PathBuf) -> Self {
        Self {
            root_session_id: session_id.clone(),
            session_id,
            parent_session_id: None,
            spawned_by_turn_id: None,
            agent_role: AgentRole::Primary,
            workspace_root,
            cwd,
            conversation: vec![],
            open_exec_sessions: vec![],
            latest_compaction: None,
            pending_approvals: vec![],
        }
    }

    pub fn normalize_lineage(&mut self) {
        if self.root_session_id.as_str().is_empty() {
            self.root_session_id = self.session_id.clone();
        }
    }
}
