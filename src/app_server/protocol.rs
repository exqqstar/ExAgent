use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{PermissionProfile, ThinkingMode};
use crate::events::RuntimeEvent;
use crate::policy::QuestionPrompt;
use crate::resolved::ModelRef;
use crate::runtime::agent_profile::AgentType;
use crate::runtime::turn_mode::TurnMode;
use crate::session::ApprovalId;
use crate::session::CompactionSummary;
use crate::types::{EventId, ThreadId, ToolCall, TurnId, UserInput};

pub const BOUNDARY_PROTOCOL_VERSION: &str = "appserver-runtime-boundary-v2";
pub const MAX_THREAD_GOAL_OBJECTIVE_CHARS: usize = 4_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitializeParams {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryCapability {
    Initialize,
    ThreadStart,
    ThreadResume,
    ThreadRead,
    ThreadFork,
    ThreadCompact,
    ThreadGoal,
    AgentTree,
    ApprovalsList,
    CheckpointRestore,
    OpenQuestionResolve,
    TurnStart,
    TurnInterrupt,
    ApprovalDecision,
    SubmitUserInput,
    EventsSubscribe,
    EventsReplay,
    MemorySearch,
    MemorySave,
    MemoryUpdate,
    MemoryForget,
    MemoryAudit,
    MemoryListCandidates,
    MemoryListArchived,
    MemoryPromote,
    WorkflowStart,
    WorkflowRead,
    WorkflowCancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub supported_ops: Vec<BoundaryCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_streams: Vec<BoundaryCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_permission_profiles: Vec<PermissionProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunParams {
    pub prompt: String,
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<PermissionProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowTemplateId {
    DeepResearch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPresetId {
    Quick,
    Standard,
    Deep,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Queued,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhaseStatus {
    Pending,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStopReason {
    TokenBudgetExceeded,
    RuntimeExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStartParams {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
    pub template_id: WorkflowTemplateId,
    pub preset_id: WorkflowPresetId,
    pub question: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStartResponse {
    pub run_id: String,
    pub thread_id: ThreadId,
    pub status: WorkflowRunStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowReadParams {
    pub workspace_root: Option<String>,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowReadResponse {
    pub run: WorkflowRunView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowCancelParams {
    pub workspace_root: Option<String>,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowCancelResponse {
    pub run: WorkflowRunView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRunView {
    pub run_id: String,
    pub thread_id: ThreadId,
    pub template_id: WorkflowTemplateId,
    pub preset_id: WorkflowPresetId,
    pub label: String,
    pub status: WorkflowRunStatus,
    pub phases: Vec<WorkflowPhaseView>,
    pub artifacts: Vec<WorkflowArtifactSummary>,
    pub stats: WorkflowStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowStopReason>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowPhaseView {
    pub id: String,
    pub label: String,
    pub status: WorkflowPhaseStatus,
    pub planned_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowArtifactSummary {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowStats {
    pub agent_calls: usize,
    pub failed_agent_calls: usize,
    pub skipped_agent_calls: usize,
    pub total_artifacts: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<i64>,
    pub elapsed_ms: i64,
    #[serde(default)]
    pub template_stats: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadStartParams {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<PermissionProfile>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalMode {
    #[default]
    Standard,
    Reviewed,
    Intensive,
}

impl ThreadGoalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Reviewed => "reviewed",
            Self::Intensive => "intensive",
        }
    }

    pub fn is_review_gated(self) -> bool {
        matches!(self, Self::Reviewed | Self::Intensive)
    }

    pub fn is_intensive(self) -> bool {
        matches!(self, Self::Intensive)
    }
}

impl From<bool> for ThreadGoalMode {
    fn from(intensive: bool) -> Self {
        if intensive {
            Self::Intensive
        } else {
            Self::Standard
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalReport {
    pub goal_id: String,
    pub objective: String,
    pub final_status: ThreadGoalStatus,
    pub turns_run: i64,
    pub tokens_used: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    pub time_used_seconds: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<String>,
    pub pending_approvals_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<ThreadGoalReportOpenQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_summary: Option<ThreadGoalReviewSummary>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalReportOpenQuestion {
    pub question_id: String,
    pub question: String,
    pub blocks_what: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalReviewSummary {
    pub ticket_id: String,
    pub status: ThreadGoalReviewStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_category: Option<ThreadGoalReviewRejectCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalReviewStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalReviewRejectCategory {
    RetriableGap,
    NeedsUser,
    ExternalBlocker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoal {
    pub thread_id: ThreadId,
    pub goal_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub continuation_suppressed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_suppressed_after_turn_id: Option<TurnId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalSetParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadGoalStatus>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_token_budget_update"
    )]
    pub token_budget: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ThreadGoalMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalSetResponse {
    pub goal: ThreadGoal,
    pub mode: ThreadGoalMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalGetParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalGetResponse {
    pub goal: Option<ThreadGoal>,
    pub mode: ThreadGoalMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalClearParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalClearResponse {
    pub cleared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadForkParams {
    pub thread_id: ThreadId,
    pub at_turn_id: TurnId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadForkResponse {
    pub new_thread_id: ThreadId,
    pub parent_thread_id: ThreadId,
    pub fork_point_turn_id: TurnId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadView {
    pub id: ThreadId,
    pub status: ThreadStatus,
    pub active_turn: Option<TurnView>,
    pub turns: Vec<TurnView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<ThreadGoal>,
    #[serde(default)]
    pub goal_mode: ThreadGoalMode,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        input: Vec<UserInput>,
    },
    AssistantMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        text: Option<String>,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<String>,
    },
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        name: String,
    },
    ExecOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        text: String,
    },
    ToolInvocation {
        invocation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_id: Option<ApprovalId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<ApprovalId>,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mutating: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_preview: Option<String>,
    },
    ApprovalRequested {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        approval_id: ApprovalId,
        tool_name: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<String>,
        #[serde(default)]
        permission_profile: PermissionProfile,
        #[serde(default = "crate::config::default_boundary_none")]
        filesystem_sandbox: String,
        #[serde(default = "crate::config::default_boundary_none")]
        network_sandbox: String,
        #[serde(default = "crate::config::default_boundary_none")]
        env_isolation: String,
    },
    ApprovalDecision {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_id: Option<ApprovalId>,
        status: String,
        note: Option<String>,
    },
    UserInputRequested {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        request_id: ApprovalId,
        tool_name: String,
        questions: Vec<QuestionPrompt>,
        status: String,
    },
    UserInputResolved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        request_id: ApprovalId,
        dismissed: bool,
    },
    RuntimeError {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        message: String,
    },
    CompactionWritten,
    SubagentSpawn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        invocation_id: String,
        tool_call_id: String,
        parent_thread_id: ThreadId,
        child_thread_id: ThreadId,
        task_name: String,
        message_preview: String,
    },
    SubagentClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        invocation_id: String,
        tool_call_id: String,
        parent_thread_id: ThreadId,
        closed_thread_id: ThreadId,
        agent_path: String,
    },
    InterAgentMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        invocation_id: String,
        tool_call_id: String,
        author_thread_id: ThreadId,
        recipient_thread_id: ThreadId,
        author_path: String,
        recipient_path: String,
        content_preview: String,
        followup: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_turn_id: Option<TurnId>,
    },
    GoalReport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<EventId>,
        report: ThreadGoalReport,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadReadParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadReadResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadCompactParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadCompactResponse {
    pub thread_id: ThreadId,
    pub latest_compaction: Option<CompactionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadResumeParams {
    pub thread_id: ThreadId,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTreeParams {
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTreeResponse {
    pub root: AgentTreeNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTreeNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<ThreadId>,
    pub root_thread_id: ThreadId,
    pub depth: u32,
    pub agent_path: String,
    pub status: AgentTreeAgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AgentTreeNode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTreeAgentStatus {
    Idle,
    Running,
    WaitingApproval,
    Done,
    Failed,
}

impl AgentTreeAgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnContextOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_thinking_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnStartParams {
    pub thread_id: ThreadId,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<UserInput>,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "TurnMode::is_default")]
    pub turn_mode: TurnMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_context: Option<TurnContextOverrides>,
}

impl TurnStartParams {
    pub fn effective_input(&self) -> Vec<UserInput> {
        if self.input.is_empty() {
            vec![UserInput::Text {
                text: self.prompt.clone(),
            }]
        } else {
            self.input.clone()
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnStartResponse {
    pub thread_id: ThreadId,
    pub turn: TurnView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptParams {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnInterruptResponse {
    pub thread_id: ThreadId,
    pub interrupted_turn: Option<TurnState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionStatus {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalsListParams {
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalsListResponse {
    pub approvals: Vec<PendingApprovalItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointRestoreParams {
    pub workspace_root: String,
    pub checkpoint_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointRestoreStatus {
    Restored,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointRestoreResponse {
    pub workspace_root: String,
    pub checkpoint_id: String,
    pub status: CheckpointRestoreStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingApprovalItem {
    pub thread_id: ThreadId,
    pub approval_id: ApprovalId,
    pub kind: PendingApprovalKind,
    pub summary: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    pub requested_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingApprovalKind {
    Command,
    OpenQuestion,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenQuestionResolveStatus {
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenQuestionResolveParams {
    pub thread_id: ThreadId,
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenQuestionResolveResponse {
    pub thread_id: ThreadId,
    pub question_id: String,
    pub goal_id: String,
    pub status: OpenQuestionResolveStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalDecisionParams {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecisionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalDecisionResponse {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub approval_id: ApprovalId,
    pub status: ApprovalDecisionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitUserInputParams {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub request_id: ApprovalId,
    #[serde(default)]
    pub answers: Vec<Vec<String>>,
    #[serde(default)]
    pub dismissed: bool,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitUserInputResponse {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub request_id: ApprovalId,
    pub dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchParams {
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub query: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySearchResponse {
    pub hits: Vec<MemoryHitView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryHitView {
    pub id: String,
    pub source: String,
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub files: Vec<String>,
    pub concepts: Vec<String>,
    pub confidence: f64,
    pub stale: bool,
    pub quarantined: bool,
    pub rank: f64,
    pub pinned: bool,
    pub status: Option<String>,
    pub use_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySaveParams {
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub input: MemorySaveInputView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySaveInputView {
    pub kind: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub concepts: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySaveResponse {
    pub entry: MemoryEntryView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryUpdateParams {
    pub workspace_root: Option<String>,
    pub entry_id: String,
    pub action: MemoryUpdateAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concepts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUpdateAction {
    Pin,
    Unpin,
    Archive,
    Unarchive,
    Reject,
    Supersede,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryUpdateResponse {
    pub entry: MemoryEntryView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryForgetParams {
    pub workspace_root: Option<String>,
    pub entry_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryForgetResponse {
    pub entry_id: String,
    pub forgotten: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryAuditParams {
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryAuditResponse {
    pub events: Vec<MemoryAuditEventView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryAuditEventView {
    pub id: String,
    pub memory_id: String,
    pub action: String,
    pub actor: String,
    pub created_at_ms: i64,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryListCandidatesParams {
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryListCandidatesResponse {
    pub candidates: Vec<MemoryEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryListArchivedParams {
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryListArchivedResponse {
    pub archived: Vec<MemoryEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryPromoteParams {
    pub workspace_root: Option<String>,
    pub entry_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default)]
    pub allow_quarantined_override: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryPromoteResponse {
    pub entry: MemoryEntryView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntryView {
    pub id: String,
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub files: Vec<String>,
    pub concepts: Vec<String>,
    pub confidence: f64,
    pub pinned: bool,
    pub status: String,
    pub stale: bool,
    pub quarantined: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactive_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_reason: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoundaryOp {
    Initialize(InitializeParams),
    ThreadStart(ThreadStartParams),
    ThreadRead(ThreadReadParams),
    ThreadFork(ThreadForkParams),
    ThreadCompact(ThreadCompactParams),
    ThreadResume(ThreadResumeParams),
    ThreadGoalSet(ThreadGoalSetParams),
    ThreadGoalGet(ThreadGoalGetParams),
    ThreadGoalClear(ThreadGoalClearParams),
    AgentTree(AgentTreeParams),
    ApprovalsList(ApprovalsListParams),
    CheckpointRestore(CheckpointRestoreParams),
    OpenQuestionResolve(OpenQuestionResolveParams),
    TurnStart(TurnStartParams),
    TurnInterrupt(TurnInterruptParams),
    ApprovalDecision(ApprovalDecisionParams),
    SubmitUserInput(SubmitUserInputParams),
    EventsReplay(EventsReplayParams),
    MemorySearch(MemorySearchParams),
    MemorySave(MemorySaveParams),
    MemoryUpdate(MemoryUpdateParams),
    MemoryForget(MemoryForgetParams),
    MemoryAudit(MemoryAuditParams),
    MemoryListCandidates(MemoryListCandidatesParams),
    MemoryListArchived(MemoryListArchivedParams),
    MemoryPromote(MemoryPromoteParams),
    WorkflowStart(WorkflowStartParams),
    WorkflowRead(WorkflowReadParams),
    WorkflowCancel(WorkflowCancelParams),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoundaryOpResponse {
    Initialized(InitializeResponse),
    ThreadStarted(ThreadStartResponse),
    ThreadRead(ThreadReadResponse),
    ThreadFork(ThreadForkResponse),
    ThreadCompacted(ThreadCompactResponse),
    ThreadResumed(ThreadResumeResponse),
    ThreadGoalSet(ThreadGoalSetResponse),
    ThreadGoalGet(ThreadGoalGetResponse),
    ThreadGoalCleared(ThreadGoalClearResponse),
    AgentTree(AgentTreeResponse),
    ApprovalsList(ApprovalsListResponse),
    CheckpointRestored(CheckpointRestoreResponse),
    OpenQuestionResolved(OpenQuestionResolveResponse),
    TurnStarted(TurnStartResponse),
    TurnInterrupted(TurnInterruptResponse),
    ApprovalDecisionSubmitted(ApprovalDecisionResponse),
    UserInputSubmitted(SubmitUserInputResponse),
    EventsReplayed(EventsReplayResponse),
    MemorySearched(MemorySearchResponse),
    MemorySaved(MemorySaveResponse),
    MemoryUpdated(MemoryUpdateResponse),
    MemoryForgotten(MemoryForgetResponse),
    MemoryAudit(MemoryAuditResponse),
    MemoryCandidatesListed(MemoryListCandidatesResponse),
    MemoryArchivedListed(MemoryListArchivedResponse),
    MemoryPromoted(MemoryPromoteResponse),
    WorkflowStarted(WorkflowStartResponse),
    WorkflowRead(WorkflowReadResponse),
    WorkflowCancelled(WorkflowCancelResponse),
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
    AssistantTextDelta,
    AssistantTurn,
    ReasoningDelta,
    Reasoning,
    ToolResult,
    ToolInvocationStarted,
    ToolInvocationWaitingApproval,
    ToolInvocationWaitingUserInput,
    ToolInvocationOutputDelta,
    ToolInvocationCompleted,
    ToolInvocationFailed,
    ToolInvocationCancelled,
    ExecOutput,
    ApprovalRequested,
    ApprovalDecision,
    UserInputRequested,
    UserInputResolved,
    CompactionWritten,
    SubagentSpawned,
    SubagentClosed,
    InterAgentMessageSent,
    TokenCount,
    ThreadGoalTurnStarted,
    ThreadGoalToolCompleted,
    ReviewSubmitted,
    OpenQuestionRecorded,
    OpenQuestionResolved,
    ThreadGoalReport,
    RuntimeError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventsReplayParams {
    pub thread_id: ThreadId,
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
    pub thread_id: ThreadId,
    pub workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_event_id: Option<EventId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplaySnapshotView {
    pub thread_id: ThreadId,
    pub cwd: PathBuf,
    #[serde(default)]
    pub permission_profile: PermissionProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<CompactionSummary>,
    pub open_exec_session_count: usize,
    pub conversation_message_count: usize,
    pub pending_approval_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsReplayResponse {
    pub thread_id: ThreadId,
    pub events: Vec<RuntimeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ReplaySnapshotView>,
}

pub fn validate_thread_goal_objective(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("goal objective must not be empty".to_string());
    }
    if trimmed.chars().count() > MAX_THREAD_GOAL_OBJECTIVE_CHARS {
        return Err(format!(
            "goal objective must be at most {MAX_THREAD_GOAL_OBJECTIVE_CHARS} characters"
        ));
    }
    Ok(())
}

fn deserialize_optional_token_budget_update<'de, D>(
    deserializer: D,
) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approvals_list_boundary_op_round_trips() {
        let params = ApprovalsListParams {
            workspace_root: Some("/repo".to_string()),
        };
        let op = BoundaryOp::ApprovalsList(params.clone());

        let value = serde_json::to_value(&op).expect("serialize approvals list op");
        assert_eq!(
            value,
            json!({
                "type": "approvals_list",
                "workspace_root": "/repo"
            })
        );
        let decoded: BoundaryOp =
            serde_json::from_value(value).expect("deserialize approvals list op");
        assert_eq!(decoded, op);

        let item = PendingApprovalItem {
            thread_id: ThreadId::new("thread_approval_protocol"),
            approval_id: ApprovalId::new("approval_protocol"),
            kind: PendingApprovalKind::Command,
            summary: "run_command: rm -rf scratch".to_string(),
            detail: "rm -rf scratch".to_string(),
            goal_id: Some("goal_protocol".to_string()),
            requested_at_ms: 1_234,
            checkpoint_id: Some("checkpoint_protocol".to_string()),
        };
        let response = BoundaryOpResponse::ApprovalsList(ApprovalsListResponse {
            approvals: vec![item.clone()],
        });

        let response_value =
            serde_json::to_value(&response).expect("serialize approvals list response");
        assert_eq!(
            response_value,
            json!({
                "type": "approvals_list",
                "approvals": [{
                    "thread_id": "thread_approval_protocol",
                    "approval_id": "approval_protocol",
                    "kind": "command",
                    "summary": "run_command: rm -rf scratch",
                    "detail": "rm -rf scratch",
                    "goal_id": "goal_protocol",
                    "requested_at_ms": 1234,
                    "checkpoint_id": "checkpoint_protocol"
                }]
            })
        );
        let decoded_response: BoundaryOpResponse =
            serde_json::from_value(response_value).expect("deserialize approvals list response");
        assert_eq!(decoded_response, response);

        let params_value = serde_json::to_value(&params).expect("serialize approvals list params");
        let decoded_params: ApprovalsListParams =
            serde_json::from_value(params_value).expect("deserialize approvals list params");
        assert_eq!(decoded_params, params);
    }

    #[test]
    fn checkpoint_restore_boundary_op_round_trips() {
        let params = CheckpointRestoreParams {
            workspace_root: "/repo".to_string(),
            checkpoint_id: "checkpoint_protocol".to_string(),
        };
        let op = BoundaryOp::CheckpointRestore(params.clone());

        let value = serde_json::to_value(&op).expect("serialize checkpoint restore op");
        assert_eq!(
            value,
            json!({
                "type": "checkpoint_restore",
                "workspace_root": "/repo",
                "checkpoint_id": "checkpoint_protocol"
            })
        );
        let decoded: BoundaryOp =
            serde_json::from_value(value).expect("deserialize checkpoint restore op");
        assert_eq!(decoded, op);

        let response = BoundaryOpResponse::CheckpointRestored(CheckpointRestoreResponse {
            workspace_root: "/repo".to_string(),
            checkpoint_id: "checkpoint_protocol".to_string(),
            status: CheckpointRestoreStatus::Restored,
            message: "workspace restored from checkpoint".to_string(),
        });

        let response_value =
            serde_json::to_value(&response).expect("serialize checkpoint restore response");
        assert_eq!(
            response_value,
            json!({
                "type": "checkpoint_restored",
                "workspace_root": "/repo",
                "checkpoint_id": "checkpoint_protocol",
                "status": "restored",
                "message": "workspace restored from checkpoint"
            })
        );
        let decoded_response: BoundaryOpResponse = serde_json::from_value(response_value)
            .expect("deserialize checkpoint restore response");
        assert_eq!(decoded_response, response);

        let params_value =
            serde_json::to_value(&params).expect("serialize checkpoint restore params");
        let decoded_params: CheckpointRestoreParams =
            serde_json::from_value(params_value).expect("deserialize checkpoint restore params");
        assert_eq!(decoded_params, params);
    }

    #[test]
    fn thread_goal_boundary_op_round_trips() {
        let goal = ThreadGoal {
            thread_id: ThreadId::new("thread_goal_protocol"),
            goal_id: "goal_1".to_string(),
            objective: "ship durable goal runtime".to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: Some(50_000),
            tokens_used: 0,
            time_used_seconds: 0,
            continuation_suppressed: false,
            continuation_suppressed_after_turn_id: None,
            created_at_ms: 1_000,
            updated_at_ms: 2_000,
        };
        let params = ThreadGoalSetParams {
            thread_id: ThreadId::new("thread_goal_protocol"),
            workspace_root: Some("/repo".to_string()),
            objective: Some("ship durable goal runtime".to_string()),
            status: Some(ThreadGoalStatus::Active),
            token_budget: Some(Some(50_000)),
            mode: Some(ThreadGoalMode::Reviewed),
        };
        let op = BoundaryOp::ThreadGoalSet(params.clone());

        let value = serde_json::to_value(&op).expect("serialize thread goal set op");
        assert_eq!(
            value,
            json!({
                "type": "thread_goal_set",
                "thread_id": "thread_goal_protocol",
                "objective": "ship durable goal runtime",
                "status": "active",
                "token_budget": 50000,
                "mode": "reviewed",
                "workspace_root": "/repo"
            })
        );
        let decoded: BoundaryOp =
            serde_json::from_value(value).expect("deserialize thread goal set op");
        assert_eq!(decoded, op);

        let goal_value = serde_json::to_value(&goal).expect("serialize thread goal");
        let decoded_goal: ThreadGoal =
            serde_json::from_value(goal_value).expect("deserialize thread goal");
        assert_eq!(decoded_goal, goal);

        let params_value = serde_json::to_value(&params).expect("serialize thread goal set params");
        let decoded_params: ThreadGoalSetParams =
            serde_json::from_value(params_value).expect("deserialize thread goal set params");
        assert_eq!(decoded_params, params);

        let response = BoundaryOpResponse::ThreadGoalSet(ThreadGoalSetResponse {
            goal: goal.clone(),
            mode: ThreadGoalMode::Reviewed,
        });
        let response_value =
            serde_json::to_value(&response).expect("serialize thread goal set response");
        assert_eq!(response_value["type"], "thread_goal_set");
        let decoded_response: BoundaryOpResponse =
            serde_json::from_value(response_value).expect("deserialize thread goal set response");
        assert_eq!(decoded_response, response);

        let get_response = BoundaryOpResponse::ThreadGoalGet(ThreadGoalGetResponse {
            goal: Some(goal),
            mode: ThreadGoalMode::Reviewed,
        });
        let get_response_value =
            serde_json::to_value(&get_response).expect("serialize thread goal get response");
        assert_eq!(get_response_value["mode"], json!("reviewed"));
        let decoded_get_response: BoundaryOpResponse = serde_json::from_value(get_response_value)
            .expect("deserialize thread goal get response");
        assert_eq!(decoded_get_response, get_response);
    }

    #[test]
    fn thread_goal_mode_serializes_as_snake_case_sidecar_value() {
        assert_eq!(
            serde_json::to_value(ThreadGoalMode::Standard).unwrap(),
            json!("standard")
        );
        assert_eq!(
            serde_json::to_value(ThreadGoalMode::Reviewed).unwrap(),
            json!("reviewed")
        );
        assert_eq!(
            serde_json::to_value(ThreadGoalMode::Intensive).unwrap(),
            json!("intensive")
        );
        assert_eq!(
            serde_json::from_value::<ThreadGoalMode>(json!("standard")).unwrap(),
            ThreadGoalMode::Standard
        );
        assert_eq!(
            serde_json::from_value::<ThreadGoalMode>(json!("reviewed")).unwrap(),
            ThreadGoalMode::Reviewed
        );
        assert_eq!(
            serde_json::from_value::<ThreadGoalMode>(json!("intensive")).unwrap(),
            ThreadGoalMode::Intensive
        );
    }

    #[test]
    fn open_question_resolve_boundary_op_round_trips() {
        let params = OpenQuestionResolveParams {
            thread_id: ThreadId::new("thread_open_question_protocol"),
            question_id: "oq_protocol".to_string(),
            answer: Some("Ship beta users first".to_string()),
            workspace_root: Some("/repo".to_string()),
        };
        let op = BoundaryOp::OpenQuestionResolve(params.clone());

        let value = serde_json::to_value(&op).expect("serialize open question resolve op");
        assert_eq!(
            value,
            json!({
                "type": "open_question_resolve",
                "thread_id": "thread_open_question_protocol",
                "question_id": "oq_protocol",
                "answer": "Ship beta users first",
                "workspace_root": "/repo"
            })
        );
        let decoded: BoundaryOp =
            serde_json::from_value(value).expect("deserialize open question resolve op");
        assert_eq!(decoded, op);

        let response = BoundaryOpResponse::OpenQuestionResolved(OpenQuestionResolveResponse {
            thread_id: ThreadId::new("thread_open_question_protocol"),
            question_id: "oq_protocol".to_string(),
            goal_id: "goal_protocol".to_string(),
            status: OpenQuestionResolveStatus::Resolved,
        });
        let response_value =
            serde_json::to_value(&response).expect("serialize open question resolve response");
        assert_eq!(
            response_value,
            json!({
                "type": "open_question_resolved",
                "thread_id": "thread_open_question_protocol",
                "question_id": "oq_protocol",
                "goal_id": "goal_protocol",
                "status": "resolved"
            })
        );
        let decoded_response: BoundaryOpResponse =
            serde_json::from_value(response_value).expect("deserialize open question response");
        assert_eq!(decoded_response, response);
    }

    #[test]
    fn thread_fork_boundary_op_round_trips() {
        let params = ThreadForkParams {
            thread_id: ThreadId::new("thread_fork_parent"),
            at_turn_id: TurnId::new("turn_1"),
            workspace_root: Some("/repo".to_string()),
        };
        let op = BoundaryOp::ThreadFork(params.clone());

        let value = serde_json::to_value(&op).expect("serialize thread fork op");
        assert_eq!(
            value,
            json!({
                "type": "thread_fork",
                "thread_id": "thread_fork_parent",
                "at_turn_id": "turn_1",
                "workspace_root": "/repo"
            })
        );
        let decoded: BoundaryOp =
            serde_json::from_value(value).expect("deserialize thread fork op");
        assert_eq!(decoded, op);

        let params_value = serde_json::to_value(&params).expect("serialize thread fork params");
        let decoded_params: ThreadForkParams =
            serde_json::from_value(params_value).expect("deserialize thread fork params");
        assert_eq!(decoded_params, params);

        let response = BoundaryOpResponse::ThreadFork(ThreadForkResponse {
            new_thread_id: ThreadId::new("thread_fork_child"),
            parent_thread_id: ThreadId::new("thread_fork_parent"),
            fork_point_turn_id: TurnId::new("turn_1"),
        });
        let response_value =
            serde_json::to_value(&response).expect("serialize thread fork response");
        assert_eq!(
            response_value,
            json!({
                "type": "thread_fork",
                "new_thread_id": "thread_fork_child",
                "parent_thread_id": "thread_fork_parent",
                "fork_point_turn_id": "turn_1"
            })
        );
        let decoded_response: BoundaryOpResponse =
            serde_json::from_value(response_value).expect("deserialize thread fork response");
        assert_eq!(decoded_response, response);
    }

    #[test]
    fn workflow_protocol_ids_and_statuses_serialize_with_expected_case() {
        assert_eq!(
            serde_json::to_value(WorkflowTemplateId::DeepResearch).unwrap(),
            json!("deep-research")
        );
        assert_eq!(
            serde_json::from_value::<WorkflowTemplateId>(json!("deep-research")).unwrap(),
            WorkflowTemplateId::DeepResearch
        );

        let presets = [
            (WorkflowPresetId::Quick, "quick"),
            (WorkflowPresetId::Standard, "standard"),
            (WorkflowPresetId::Deep, "deep"),
        ];
        for (preset, expected) in presets {
            assert_eq!(serde_json::to_value(preset).unwrap(), json!(expected));
            assert_eq!(
                serde_json::from_value::<WorkflowPresetId>(json!(expected)).unwrap(),
                preset
            );
        }

        let statuses = [
            (WorkflowRunStatus::Queued, "queued"),
            (WorkflowRunStatus::Running, "running"),
            (WorkflowRunStatus::WaitingApproval, "waiting_approval"),
            (WorkflowRunStatus::Completed, "completed"),
            (WorkflowRunStatus::Failed, "failed"),
            (WorkflowRunStatus::Cancelled, "cancelled"),
        ];
        for (status, expected) in statuses {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
            assert_eq!(
                serde_json::from_value::<WorkflowRunStatus>(json!(expected)).unwrap(),
                status
            );
        }

        let phase_statuses = [
            (WorkflowPhaseStatus::Pending, "pending"),
            (WorkflowPhaseStatus::Running, "running"),
            (WorkflowPhaseStatus::WaitingApproval, "waiting_approval"),
            (WorkflowPhaseStatus::Completed, "completed"),
            (WorkflowPhaseStatus::Failed, "failed"),
            (WorkflowPhaseStatus::Skipped, "skipped"),
            (WorkflowPhaseStatus::Cancelled, "cancelled"),
        ];
        for (status, expected) in phase_statuses {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
            assert_eq!(
                serde_json::from_value::<WorkflowPhaseStatus>(json!(expected)).unwrap(),
                status
            );
        }
    }

    #[test]
    fn workflow_start_params_round_trip() {
        let params = WorkflowStartParams {
            workspace_root: Some(".".to_string()),
            cwd: Some(".".to_string()),
            template_id: WorkflowTemplateId::DeepResearch,
            preset_id: WorkflowPresetId::Standard,
            question: "Research world models".to_string(),
        };

        let value = serde_json::to_value(&params).expect("serialize workflow start params");
        assert_eq!(
            value,
            json!({
                "workspace_root": ".",
                "cwd": ".",
                "template_id": "deep-research",
                "preset_id": "standard",
                "question": "Research world models"
            })
        );
        let decoded: WorkflowStartParams =
            serde_json::from_value(value).expect("deserialize workflow start params");
        assert_eq!(decoded, params);
    }

    #[test]
    fn workflow_boundary_ops_round_trip() {
        let start = BoundaryOp::WorkflowStart(WorkflowStartParams {
            workspace_root: Some("/repo".to_string()),
            cwd: Some("research".to_string()),
            template_id: WorkflowTemplateId::DeepResearch,
            preset_id: WorkflowPresetId::Quick,
            question: "Research world models".to_string(),
        });

        let start_value = serde_json::to_value(&start).expect("serialize workflow start op");
        assert_eq!(
            start_value,
            json!({
                "type": "workflow_start",
                "workspace_root": "/repo",
                "cwd": "research",
                "template_id": "deep-research",
                "preset_id": "quick",
                "question": "Research world models"
            })
        );
        let decoded_start: BoundaryOp =
            serde_json::from_value(start_value).expect("deserialize workflow start op");
        assert_eq!(decoded_start, start);

        let read = BoundaryOp::WorkflowRead(WorkflowReadParams {
            workspace_root: Some("/repo".to_string()),
            run_id: "workflow_run_1".to_string(),
        });
        let read_value = serde_json::to_value(&read).expect("serialize workflow read op");
        assert_eq!(
            read_value,
            json!({
                "type": "workflow_read",
                "workspace_root": "/repo",
                "run_id": "workflow_run_1"
            })
        );
        let decoded_read: BoundaryOp =
            serde_json::from_value(read_value).expect("deserialize workflow read op");
        assert_eq!(decoded_read, read);

        let cancel = BoundaryOp::WorkflowCancel(WorkflowCancelParams {
            workspace_root: Some("/repo".to_string()),
            run_id: "workflow_run_1".to_string(),
        });
        let cancel_value = serde_json::to_value(&cancel).expect("serialize workflow cancel op");
        assert_eq!(
            cancel_value,
            json!({
                "type": "workflow_cancel",
                "workspace_root": "/repo",
                "run_id": "workflow_run_1"
            })
        );
        let decoded_cancel: BoundaryOp =
            serde_json::from_value(cancel_value).expect("deserialize workflow cancel op");
        assert_eq!(decoded_cancel, cancel);
    }

    #[test]
    fn workflow_boundary_responses_round_trip_with_run_view() {
        let view = WorkflowRunView {
            run_id: "workflow_run_1".to_string(),
            thread_id: ThreadId::new("thread_workflow"),
            template_id: WorkflowTemplateId::DeepResearch,
            preset_id: WorkflowPresetId::Standard,
            label: "Research world models".to_string(),
            status: WorkflowRunStatus::Running,
            phases: vec![WorkflowPhaseView {
                id: "scope".to_string(),
                label: "Scope".to_string(),
                status: WorkflowPhaseStatus::Completed,
                planned_count: 1,
                completed_count: 1,
                failed_count: 0,
                skipped_count: 0,
                started_at_ms: Some(1_000),
                updated_at_ms: 1_100,
                completed_at_ms: Some(1_100),
            }],
            artifacts: vec![WorkflowArtifactSummary {
                id: "artifact_report".to_string(),
                label: "Report".to_string(),
                status: Some("draft".to_string()),
                created_at_ms: 1_200,
                updated_at_ms: 1_300,
            }],
            stats: WorkflowStats {
                agent_calls: 42,
                failed_agent_calls: 1,
                skipped_agent_calls: 0,
                total_artifacts: 1,
                tokens_used: Some(12_000),
                elapsed_ms: 3_000,
                template_stats: json!({
                    "search_agents": 4,
                    "fetch_agents": 12,
                    "verify_agents": 24,
                    "synth_agents": 1
                }),
            },
            report_summary: Some("Early findings are available.".to_string()),
            stop_reason: Some(WorkflowStopReason::TokenBudgetExceeded),
            created_at_ms: 900,
            updated_at_ms: 1_300,
            started_at_ms: Some(1_000),
            completed_at_ms: None,
        };

        let started = BoundaryOpResponse::WorkflowStarted(WorkflowStartResponse {
            run_id: "workflow_run_1".to_string(),
            thread_id: ThreadId::new("thread_workflow"),
            status: WorkflowRunStatus::Queued,
        });
        let started_value =
            serde_json::to_value(&started).expect("serialize workflow started response");
        assert_eq!(
            started_value,
            json!({
                "type": "workflow_started",
                "run_id": "workflow_run_1",
                "thread_id": "thread_workflow",
                "status": "queued"
            })
        );
        let decoded_started: BoundaryOpResponse =
            serde_json::from_value(started_value).expect("deserialize workflow started response");
        assert_eq!(decoded_started, started);

        let read = BoundaryOpResponse::WorkflowRead(WorkflowReadResponse { run: view.clone() });
        let read_value = serde_json::to_value(&read).expect("serialize workflow read response");
        assert_eq!(read_value["type"], "workflow_read");
        assert_eq!(read_value["run"]["template_id"], "deep-research");
        assert_eq!(
            read_value["run"]["stats"]["template_stats"]["verify_agents"],
            24
        );
        assert_eq!(read_value["run"]["stop_reason"], "token_budget_exceeded");
        let decoded_read: BoundaryOpResponse =
            serde_json::from_value(read_value).expect("deserialize workflow read response");
        assert_eq!(decoded_read, read);

        let cancelled = BoundaryOpResponse::WorkflowCancelled(WorkflowCancelResponse { run: view });
        let cancelled_value =
            serde_json::to_value(&cancelled).expect("serialize workflow cancelled response");
        assert_eq!(cancelled_value["type"], "workflow_cancelled");
        let decoded_cancelled: BoundaryOpResponse = serde_json::from_value(cancelled_value)
            .expect("deserialize workflow cancelled response");
        assert_eq!(decoded_cancelled, cancelled);
    }

    #[test]
    fn thread_goal_set_params_preserve_explicit_null_token_budget() {
        let decoded: ThreadGoalSetParams = serde_json::from_value(json!({
            "thread_id": "thread_goal_protocol",
            "token_budget": null
        }))
        .expect("deserialize explicit null token budget");

        assert_eq!(decoded.token_budget, Some(None));

        let missing: ThreadGoalSetParams = serde_json::from_value(json!({
            "thread_id": "thread_goal_protocol"
        }))
        .expect("deserialize missing token budget");

        assert_eq!(missing.token_budget, None);
    }

    #[test]
    fn agent_tree_node_omits_absent_activity_and_usage_fields() {
        let node = AgentTreeNode {
            thread_id: Some(ThreadId::new("thread_agent_tree")),
            parent_thread_id: None,
            root_thread_id: ThreadId::new("thread_agent_tree"),
            depth: 0,
            agent_path: "root".to_string(),
            status: AgentTreeAgentStatus::Idle,
            agent_type: None,
            agent_role: None,
            agent_nickname: None,
            last_task_message: None,
            last_activity: None,
            current_tool: None,
            tokens_used: None,
            children: vec![],
        };

        let value = serde_json::to_value(&node).expect("serialize agent tree node");

        assert!(value.get("current_tool").is_none());
        assert!(value.get("tokens_used").is_none());

        let decoded: AgentTreeNode = serde_json::from_value(json!({
            "thread_id": "thread_agent_tree",
            "root_thread_id": "thread_agent_tree",
            "depth": 0,
            "agent_path": "root",
            "status": "idle"
        }))
        .expect("deserialize legacy agent tree node");

        assert_eq!(decoded.current_tool, None);
        assert_eq!(decoded.tokens_used, None);
    }

    #[test]
    fn agent_tree_node_includes_present_activity_and_usage_fields() {
        let node = AgentTreeNode {
            thread_id: Some(ThreadId::new("thread_agent_tree")),
            parent_thread_id: None,
            root_thread_id: ThreadId::new("thread_agent_tree"),
            depth: 0,
            agent_path: "root".to_string(),
            status: AgentTreeAgentStatus::Running,
            agent_type: None,
            agent_role: None,
            agent_nickname: None,
            last_task_message: None,
            last_activity: None,
            current_tool: Some("run_command".to_string()),
            tokens_used: Some(42_000),
            children: vec![],
        };

        let value = serde_json::to_value(&node).expect("serialize agent tree node");

        assert_eq!(value["current_tool"], "run_command");
        assert_eq!(value["tokens_used"], 42_000);
    }

    #[test]
    fn thread_goal_objective_validation_rejects_empty_whitespace_and_long_values() {
        assert_eq!(
            validate_thread_goal_objective("").unwrap_err(),
            "goal objective must not be empty"
        );
        assert_eq!(
            validate_thread_goal_objective("   \n\t").unwrap_err(),
            "goal objective must not be empty"
        );
        assert_eq!(
            validate_thread_goal_objective(&"x".repeat(MAX_THREAD_GOAL_OBJECTIVE_CHARS + 1))
                .unwrap_err(),
            "goal objective must be at most 4000 characters"
        );
    }
}
