use serde::{Deserialize, Serialize};

use crate::app_server::protocol::{
    WorkflowArtifactSummary, WorkflowPhaseView, WorkflowPresetId, WorkflowRunStatus, WorkflowStats,
    WorkflowTemplateId,
};
use crate::config::PermissionProfile;
use crate::policy::QuestionPrompt;
use crate::session::{ApprovalId, ApprovalStatus, CompactionSummary, ExecSessionId};
use crate::types::{
    AssistantTurn, EventId, ThreadId, TokenUsageInfo, ToolResult, ToolStatus, TurnId,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalCommandPayload {
    pub command: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub persistent: bool,
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdictEvent {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRejectCategoryEvent {
    RetriableGap,
    NeedsUser,
    ExternalBlocker,
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
    AssistantTextDelta {
        delta: String,
    },
    Reasoning {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<String>,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolResult {
        result: ToolResult,
    },
    ToolInvocationStarted {
        invocation_id: String,
        tool_call_id: String,
        tool_name: String,
        mutating: bool,
    },
    ToolInvocationWaitingApproval {
        invocation_id: String,
        approval_id: ApprovalId,
        reason: String,
    },
    ToolInvocationWaitingUserInput {
        invocation_id: String,
        request_id: ApprovalId,
        reason: String,
    },
    ToolInvocationOutputDelta {
        invocation_id: String,
        stream: ExecOutputStream,
        chunk: String,
        sequence: u64,
    },
    ToolInvocationCompleted {
        invocation_id: String,
        tool_call_id: String,
        tool_name: String,
        status: ToolStatus,
    },
    ToolInvocationFailed {
        invocation_id: String,
        tool_call_id: String,
        tool_name: String,
        message: String,
    },
    ToolInvocationCancelled {
        invocation_id: String,
        tool_call_id: String,
        tool_name: String,
        reason: String,
    },
    ExecOutput {
        exec_session_id: ExecSessionId,
        stream: ExecOutputStream,
        chunk: String,
        #[serde(default)]
        sequence: u64,
    },
    ApprovalRequested {
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<ApprovalCommandPayload>,
    },
    ApprovalDecision {
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    },
    UserInputRequested {
        request_id: ApprovalId,
        tool_name: String,
        questions: Vec<QuestionPrompt>,
    },
    UserInputResolved {
        request_id: ApprovalId,
        dismissed: bool,
    },
    CompactionWritten {
        summary: CompactionSummary,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_turn_id: Option<TurnId>,
    },
    TokenCount {
        info: Option<TokenUsageInfo>,
    },
    ThreadGoalUpdated {
        goal: crate::app_server::protocol::ThreadGoal,
    },
    ThreadGoalModeUpdated {
        thread_id: ThreadId,
        goal_id: String,
        mode: crate::app_server::protocol::ThreadGoalMode,
    },
    ThreadGoalCleared {
        thread_id: ThreadId,
    },
    ThreadGoalContinuationStarted {
        goal_id: String,
    },
    ThreadGoalContinuationSuppressed {
        goal_id: String,
        reason: String,
    },
    ThreadGoalTurnStarted {
        goal_id: String,
    },
    ThreadGoalToolCompleted {
        goal_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changed_files: Vec<String>,
    },
    ReviewSubmitted {
        ticket_id: String,
        goal_id: String,
        verdict: ReviewVerdictEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reviewed_hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reject_category: Option<ReviewRejectCategoryEvent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        findings: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
    ThreadGoalReport {
        report: crate::app_server::protocol::ThreadGoalReport,
    },
    WorkflowStarted {
        run_id: String,
        template_id: WorkflowTemplateId,
        preset_id: WorkflowPresetId,
        label: String,
    },
    WorkflowPhaseStarted {
        run_id: String,
        phase_id: String,
        label: String,
        planned_count: usize,
    },
    WorkflowPhaseUpdated {
        run_id: String,
        phase: WorkflowPhaseView,
    },
    WorkflowArtifactRecorded {
        run_id: String,
        artifact: WorkflowArtifactSummary,
    },
    WorkflowCompleted {
        run_id: String,
        status: WorkflowRunStatus,
        stats: WorkflowStats,
    },
    RuntimeError {
        message: String,
    },
}

pub fn redact_runtime_event_for_public_boundary(mut event: RuntimeEvent) -> RuntimeEvent {
    event.kind = redact_runtime_event_kind_for_public_boundary(event.kind);
    event
}

pub fn redact_runtime_events_for_public_boundary(events: Vec<RuntimeEvent>) -> Vec<RuntimeEvent> {
    events
        .into_iter()
        .map(redact_runtime_event_for_public_boundary)
        .collect()
}

pub fn redact_runtime_event_kind_for_public_boundary(kind: RuntimeEventKind) -> RuntimeEventKind {
    match kind {
        RuntimeEventKind::AssistantTurn { mut turn } => {
            turn.reasoning.clear();
            for tool_call in &mut turn.tool_calls {
                tool_call.thought_signature = None;
            }
            RuntimeEventKind::AssistantTurn { turn }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ReasoningBlock, ReasoningSignature, TokenUsage, ToolCall};
    use serde_json::json;

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

    #[test]
    fn approval_requested_event_defaults_missing_boundary_fields_to_none() {
        let kind: RuntimeEventKind = serde_json::from_value(json!({
            "type": "approval_requested",
            "approval_id": "approval_legacy",
            "tool_name": "run_command",
            "reason": "approval required",
            "permission_profile": "full_access"
        }))
        .expect("deserialize legacy approval request");

        let value = serde_json::to_value(kind).expect("serialize approval request");
        assert_eq!(value["filesystem_sandbox"], "none");
        assert_eq!(value["network_sandbox"], "none");
        assert_eq!(value["env_isolation"], "none");
    }

    #[test]
    fn exec_output_event_defaults_missing_sequence_to_zero() {
        let kind: RuntimeEventKind = serde_json::from_value(json!({
            "type": "exec_output",
            "exec_session_id": "exec_legacy",
            "stream": "stdout",
            "chunk": "hello"
        }))
        .expect("deserialize legacy exec output");

        match kind {
            RuntimeEventKind::ExecOutput { sequence, .. } => assert_eq!(sequence, 0),
            other => panic!("expected exec output event, got {other:?}"),
        }
    }

    #[test]
    fn public_boundary_redaction_preserves_visible_assistant_payload() {
        let event = RuntimeEvent {
            event_id: EventId::new("evt_1"),
            thread_id: ThreadId::new("thread_1"),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::AssistantTurn {
                turn: AssistantTurn {
                    text: Some("visible answer".to_string()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        arguments: json!({"path": "Cargo.toml"}),
                        thought_signature: Some(json!("hidden-tool-signature")),
                    }],
                    reasoning: vec![ReasoningBlock {
                        text: "hidden reasoning".to_string(),
                        signature: Some(ReasoningSignature::AnthropicSignature(
                            "hidden-reasoning-signature".to_string(),
                        )),
                        redacted: false,
                    }],
                },
            },
        };

        let redacted = redact_runtime_event_for_public_boundary(event);
        let RuntimeEventKind::AssistantTurn { turn } = redacted.kind else {
            panic!("expected assistant turn");
        };

        assert_eq!(turn.text.as_deref(), Some("visible answer"));
        assert!(turn.reasoning.is_empty());
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments, json!({"path": "Cargo.toml"}));
        assert_eq!(turn.tool_calls[0].thought_signature, None);
    }

    #[test]
    fn thread_goal_events_round_trip() {
        let goal = crate::app_server::protocol::ThreadGoal {
            thread_id: ThreadId::new("thread_goal_events"),
            goal_id: "goal_1".to_string(),
            objective: "ship durable goal runtime".to_string(),
            status: crate::app_server::protocol::ThreadGoalStatus::Active,
            token_budget: Some(50_000),
            tokens_used: 123,
            time_used_seconds: 45,
            continuation_suppressed: false,
            continuation_suppressed_after_turn_id: Some(TurnId::new("turn_1")),
            created_at_ms: 1_000,
            updated_at_ms: 2_000,
        };

        let events = vec![
            RuntimeEventKind::ThreadGoalUpdated { goal },
            RuntimeEventKind::ThreadGoalModeUpdated {
                thread_id: ThreadId::new("thread_goal_events"),
                goal_id: "goal_1".to_string(),
                mode: crate::app_server::protocol::ThreadGoalMode::Reviewed,
            },
            RuntimeEventKind::ThreadGoalCleared {
                thread_id: ThreadId::new("thread_goal_events"),
            },
            RuntimeEventKind::ThreadGoalContinuationStarted {
                goal_id: "goal_1".to_string(),
            },
            RuntimeEventKind::ThreadGoalContinuationSuppressed {
                goal_id: "goal_1".to_string(),
                reason: "budget exhausted".to_string(),
            },
        ];

        for event in events {
            let value = serde_json::to_value(&event).expect("serialize thread goal event");
            let decoded: RuntimeEventKind =
                serde_json::from_value(value).expect("deserialize thread goal event");
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn workflow_events_round_trip() {
        let phase = crate::app_server::protocol::WorkflowPhaseView {
            id: "collect".to_string(),
            label: "Collect".to_string(),
            status: crate::app_server::protocol::WorkflowPhaseStatus::Running,
            planned_count: 3,
            completed_count: 1,
            failed_count: 0,
            skipped_count: 0,
            started_at_ms: Some(1_000),
            updated_at_ms: 1_100,
            completed_at_ms: None,
        };
        let artifact = WorkflowArtifactSummary {
            id: "artifact_1".to_string(),
            label: "Sources".to_string(),
            status: Some("ready".to_string()),
            created_at_ms: 1_200,
            updated_at_ms: 1_300,
        };
        let stats = WorkflowStats {
            agent_calls: 2,
            failed_agent_calls: 1,
            skipped_agent_calls: 0,
            total_artifacts: 1,
            tokens_used: Some(500),
            elapsed_ms: 250,
            template_stats: json!({"claims": 4}),
        };
        let events = vec![
            RuntimeEventKind::WorkflowStarted {
                run_id: "workflow_1".to_string(),
                template_id: WorkflowTemplateId::DeepResearch,
                preset_id: WorkflowPresetId::Quick,
                label: "Deep research".to_string(),
            },
            RuntimeEventKind::WorkflowPhaseStarted {
                run_id: "workflow_1".to_string(),
                phase_id: "collect".to_string(),
                label: "Collect".to_string(),
                planned_count: 3,
            },
            RuntimeEventKind::WorkflowPhaseUpdated {
                run_id: "workflow_1".to_string(),
                phase,
            },
            RuntimeEventKind::WorkflowArtifactRecorded {
                run_id: "workflow_1".to_string(),
                artifact,
            },
            RuntimeEventKind::WorkflowCompleted {
                run_id: "workflow_1".to_string(),
                status: WorkflowRunStatus::Completed,
                stats,
            },
        ];

        for event in events {
            let value = serde_json::to_value(&event).expect("serialize workflow event");
            let decoded: RuntimeEventKind =
                serde_json::from_value(value).expect("deserialize workflow event");
            assert_eq!(decoded, event);
        }
    }
}
