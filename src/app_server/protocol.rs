use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::events::RuntimeEvent;
use crate::orchestration::{ChildSessionSummary, CollectedChildSession};
use crate::session::{AgentRole, CompactionSummary};
use crate::types::{EventId, SessionId, ToolCall, TurnId};

pub const BOUNDARY_PROTOCOL_VERSION: &str = "appserver-runtime-boundary-v2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitializeParams {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryCapability {
    Initialize,
    ThreadStart,
    ThreadResume,
    ThreadSpawnChild,
    ThreadRead,
    TurnStart,
    TurnInterrupt,
    EventsSubscribe,
    EventsReplay,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub supported_ops: Vec<BoundaryCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_streams: Vec<BoundaryCapability>,
}

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

// Legacy adapter compatibility DTOs for the pre-boundary HTTP/CLI surface.
// Keep these stable for old clients, but add new runtime capabilities through
// BoundaryOp and the thread/turn protocol types below.
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadStartParams {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadStartResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Idle,
    Running,
    WaitingApproval,
    Failed,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Queued,
    Running,
    InProgress,
    WaitingApproval,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnState {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadView {
    pub id: SessionId,
    pub status: ThreadStatus,
    pub active_turn: Option<TurnView>,
    pub turns: Vec<TurnView>,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnView {
    pub id: TurnId,
    pub status: TurnStatus,
    pub items: Vec<ThreadItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadItem {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: Option<String>,
    },
    ToolResult {
        name: String,
    },
    ExecOutput {
        text: String,
    },
    ApprovalRequested {
        tool_name: String,
        reason: String,
    },
    ApprovalDecision {
        status: String,
        note: Option<String>,
    },
    RuntimeError {
        message: String,
    },
    StructuredResult {
        kind: String,
    },
    CompactionWritten,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadReadParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadReadResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadResumeParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IgnoredOverrideField {
    Cwd,
    Model,
    Provider,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadResumeResponse {
    pub thread: ThreadView,
    pub ignored_overrides: Vec<IgnoredOverrideField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnContextOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnStartParams {
    pub thread_id: SessionId,
    pub prompt: String,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_context: Option<TurnContextOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnStartResponse {
    pub thread_id: SessionId,
    pub turn: TurnView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptParams {
    pub thread_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptResponse {
    pub thread_id: SessionId,
    pub interrupted_turn: Option<TurnState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub ignored_overrides: Vec<IgnoredOverrideField>,
    pub output: AgentRunResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoundaryOp {
    Initialize(InitializeParams),
    ThreadStart(ThreadStartParams),
    ThreadRead(ThreadReadParams),
    ThreadResume(ThreadResumeParams),
    TurnStart(TurnStartParams),
    TurnInterrupt(TurnInterruptParams),
    ThreadSpawnChild(ThreadSpawnChildParams),
    EventsReplay(EventsReplayParams),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoundaryOpResponse {
    Initialized(InitializeResponse),
    ThreadStarted(ThreadStartResponse),
    ThreadRead(ThreadReadResponse),
    ThreadResumed(ThreadResumeResponse),
    TurnStarted(TurnStartResponse),
    TurnInterrupted(TurnInterruptResponse),
    ThreadChildSpawned(ThreadSpawnChildResponse),
    EventsReplayed(EventsReplayResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueuedThreadOp {
    UserInput { prompt: String },
    UserInputWithTurnContext { prompt: String },
    Interrupt,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKindFilter {
    TurnStarted,
    TurnCompleted,
    TurnInterrupted,
    AssistantTurn,
    ToolResult,
    SessionSpawned,
    ExecOutput,
    ApprovalRequested,
    ApprovalDecision,
    CompactionWritten,
    StructuredResultRecorded,
    RuntimeError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventsReplayParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_snapshot: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_kinds: Vec<RuntimeEventKindFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventsSubscribeParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_event_id: Option<EventId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplaySnapshotView {
    pub thread_id: SessionId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<CompactionSummary>,
    pub open_exec_session_count: usize,
    pub conversation_message_count: usize,
    pub pending_approval_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsReplayResponse {
    pub thread_id: SessionId,
    pub events: Vec<RuntimeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ReplaySnapshotView>,
}
