use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::events::RuntimeEvent;
use crate::orchestration::{ChildSessionSummary, CollectedChildSession};
use crate::session::AgentRole;
use crate::types::{SessionId, ToolCall, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub session_id: SessionId,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectResponse {
    pub children: Vec<ChildSessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectResponse {
    pub session: CollectedChildSession,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RunParams {
    pub prompt: String,
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
    pub session_id: Option<SessionId>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ForkParams {
    pub parent_session_id: SessionId,
    pub agent_role: AgentRole,
    pub prompt: String,
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub spawned_by_turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct InspectParams {
    pub parent_session_id: SessionId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CollectParams {
    pub session_id: SessionId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ThreadStartParams {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadStartResponse {
    pub thread_id: SessionId,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TurnStartParams {
    pub thread_id: SessionId,
    pub prompt: String,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnStartResponse {
    pub thread_id: SessionId,
    pub turn_id: TurnId,
    pub output: AgentRunResponse,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ThreadSpawnChildParams {
    pub parent_thread_id: SessionId,
    pub agent_role: AgentRole,
    pub prompt: String,
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub spawned_by_turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSpawnChildResponse {
    pub parent_thread_id: SessionId,
    pub child_thread_id: SessionId,
    pub agent_role: AgentRole,
    pub output: AgentRunResponse,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EventsReplayParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsReplayResponse {
    pub thread_id: SessionId,
    pub events: Vec<RuntimeEvent>,
}
