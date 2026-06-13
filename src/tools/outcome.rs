use std::path::PathBuf;

use crate::config::PermissionProfile;
use crate::events::{ApprovalCommandPayload, ReviewRejectCategoryEvent, ReviewVerdictEvent};
use crate::policy::QuestionPrompt;
use crate::session::{ApprovalId, ExecSessionId};
use crate::types::{ThreadId, ToolResult, ToolStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct ToolModelOutput {
    pub content: String,
}

impl ToolModelOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub model_result: ToolResult,
    pub effects: Vec<ToolRuntimeEffect>,
}

impl ToolOutcome {
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        model_output: ToolModelOutput,
    ) -> Self {
        Self {
            model_result: ToolResult {
                tool_call_id: tool_call_id.into(),
                tool_name: tool_name.into(),
                status: ToolStatus::Success,
                content: model_output.content,
                meta: None,
                parts: Vec::new(),
            },
            effects: Vec::new(),
        }
    }

    pub fn from_result(model_result: ToolResult) -> Self {
        Self {
            model_result,
            effects: Vec::new(),
        }
    }

    pub fn with_effect(mut self, effect: ToolRuntimeEffect) -> Self {
        self.effects.push(effect);
        self
    }

    pub fn with_effects(mut self, effects: impl IntoIterator<Item = ToolRuntimeEffect>) -> Self {
        self.effects.extend(effects);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolRuntimeEffect {
    ShortCircuit {
        result: ToolResult,
    },
    ReviewSubmitted {
        ticket_id: String,
        goal_id: String,
        verdict: ReviewVerdictEvent,
        reviewed_hash: Option<String>,
        reject_category: Option<ReviewRejectCategoryEvent>,
        findings: Option<String>,
        checkpoint_id: Option<String>,
    },
    OpenQuestionRecorded {
        question_id: String,
        goal_id: String,
        question: String,
        blocks_what: String,
    },
    OpenQuestionResolved {
        question_id: String,
        goal_id: String,
        answer: Option<String>,
    },
    ExecSessionRunning {
        exec_session_id: ExecSessionId,
        command: String,
        cwd: PathBuf,
    },
    ExecSessionNotRunning {
        exec_session_id: ExecSessionId,
    },
    ApprovalRequested {
        approval_id: ApprovalId,
        tool_name: String,
        reason: String,
        checkpoint_id: Option<String>,
        permission_profile: PermissionProfile,
        filesystem_sandbox: String,
        network_sandbox: String,
        env_isolation: String,
        command: Option<ApprovalCommandPayload>,
    },
    ApprovalApproved {
        approval_id: ApprovalId,
        note: Option<String>,
    },
    ApprovalDenied {
        approval_id: ApprovalId,
        note: Option<String>,
    },
    UserInputRequested {
        request_id: ApprovalId,
        thread_id: ThreadId,
        tool_name: String,
        questions: Vec<QuestionPrompt>,
    },
    UserInputResolved {
        request_id: ApprovalId,
        dismissed: bool,
    },
    SubagentSpawned {
        invocation_id: String,
        tool_call_id: String,
        parent_thread_id: ThreadId,
        child_thread_id: ThreadId,
        task_name: String,
        message_preview: String,
    },
    SubagentClosed {
        invocation_id: String,
        tool_call_id: String,
        parent_thread_id: ThreadId,
        closed_thread_id: ThreadId,
        agent_path: String,
    },
    InterAgentMessageSent {
        invocation_id: String,
        tool_call_id: String,
        author_thread_id: ThreadId,
        recipient_thread_id: ThreadId,
        author_path: String,
        recipient_path: String,
        content_preview: String,
        followup: bool,
        started_turn_id: Option<crate::types::TurnId>,
    },
    ThreadGoalUpdated {
        goal: crate::app_server::protocol::ThreadGoal,
    },
}
