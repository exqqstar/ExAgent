use std::path::PathBuf;

use anyhow::Result;

use crate::config::PermissionProfile;
use crate::events::RuntimeEventKind;
use crate::runtime::thread_session::{LiveEventSink, ThreadEventRecorder};
use crate::session::{ApprovalId, ApprovalStatus, ExecSessionId, ThreadSnapshot};
use crate::tools::{ToolOutcome, ToolRuntimeEffect};
use crate::types::{ToolResult, TurnId};

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionOutcome {
    pub(crate) result: ToolResult,
    effects: Vec<ToolEffect>,
}

impl ToolExecutionOutcome {
    pub(super) fn from_tool_outcome(outcome: ToolOutcome) -> Self {
        Self {
            result: outcome.model_result,
            effects: outcome
                .effects
                .into_iter()
                .map(ToolEffect::from_runtime_effect)
                .collect(),
        }
    }

    pub(crate) fn apply_effects(
        &self,
        recorder: &mut ThreadEventRecorder,
        snapshot: &mut ThreadSnapshot,
        turn_id: &TurnId,
    ) -> Result<()> {
        for effect in &self.effects {
            apply_tool_effect(recorder, snapshot, turn_id, effect.clone())?;
        }
        Ok(())
    }

    pub(crate) fn attach_approval_note(&mut self, approval_id: &ApprovalId, note: Option<String>) {
        for effect in &mut self.effects {
            match effect {
                ToolEffect::ApprovalUpdate(ApprovalUpdate::Approved {
                    approval_id: effect_approval_id,
                    note: effect_note,
                })
                | ToolEffect::ApprovalUpdate(ApprovalUpdate::Denied {
                    approval_id: effect_approval_id,
                    note: effect_note,
                }) if effect_approval_id == approval_id => {
                    *effect_note = note.clone();
                }
                _ => {}
            }
        }
    }

    pub(crate) fn approval_matches(
        &self,
        approval_id: &ApprovalId,
        status: &ApprovalStatus,
    ) -> bool {
        self.effects.iter().any(|effect| match (effect, status) {
            (
                ToolEffect::ApprovalUpdate(ApprovalUpdate::Approved {
                    approval_id: effect_approval_id,
                    ..
                }),
                ApprovalStatus::Approved,
            )
            | (
                ToolEffect::ApprovalUpdate(ApprovalUpdate::Denied {
                    approval_id: effect_approval_id,
                    ..
                }),
                ApprovalStatus::Denied,
            ) => effect_approval_id == approval_id,
            _ => false,
        })
    }

    pub(crate) fn user_input_resolved_matches(
        &self,
        request_id: &ApprovalId,
        dismissed: bool,
    ) -> bool {
        self.effects.iter().any(|effect| {
            matches!(
                effect,
                ToolEffect::UserInputUpdate(UserInputUpdate::Resolved {
                    request_id: effect_request_id,
                    dismissed: effect_dismissed,
                }) if effect_request_id == request_id && *effect_dismissed == dismissed
            )
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ToolEffect {
    ExecSessionUpdate(ExecSessionUpdate),
    ApprovalUpdate(ApprovalUpdate),
    UserInputUpdate(UserInputUpdate),
    SubagentSpawned(SubagentSpawned),
    SubagentClosed(SubagentClosed),
    InterAgentMessageSent(InterAgentMessageSent),
    ThreadGoalUpdated(crate::app_server::protocol::ThreadGoal),
    ReviewSubmitted {
        ticket_id: String,
        goal_id: String,
        verdict: crate::events::ReviewVerdictEvent,
        reviewed_hash: Option<String>,
        reject_category: Option<crate::events::ReviewRejectCategoryEvent>,
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
    ShortCircuit,
}

impl ToolEffect {
    fn from_runtime_effect(effect: ToolRuntimeEffect) -> Self {
        match effect {
            ToolRuntimeEffect::ShortCircuit { .. } => Self::ShortCircuit,
            ToolRuntimeEffect::ReviewSubmitted {
                ticket_id,
                goal_id,
                verdict,
                reviewed_hash,
                reject_category,
                findings,
                checkpoint_id,
            } => Self::ReviewSubmitted {
                ticket_id,
                goal_id,
                verdict,
                reviewed_hash,
                reject_category,
                findings,
                checkpoint_id,
            },
            ToolRuntimeEffect::OpenQuestionRecorded {
                question_id,
                goal_id,
                question,
                blocks_what,
            } => Self::OpenQuestionRecorded {
                question_id,
                goal_id,
                question,
                blocks_what,
            },
            ToolRuntimeEffect::OpenQuestionResolved {
                question_id,
                goal_id,
                answer,
            } => Self::OpenQuestionResolved {
                question_id,
                goal_id,
                answer,
            },
            ToolRuntimeEffect::ExecSessionRunning {
                exec_session_id,
                command,
                cwd,
            } => Self::ExecSessionUpdate(ExecSessionUpdate::Running {
                exec_session_id,
                command,
                cwd,
            }),
            ToolRuntimeEffect::ExecSessionNotRunning { exec_session_id } => {
                Self::ExecSessionUpdate(ExecSessionUpdate::NotRunning { exec_session_id })
            }
            ToolRuntimeEffect::ApprovalRequested {
                approval_id,
                tool_name,
                reason,
                checkpoint_id,
                permission_profile,
                filesystem_sandbox,
                network_sandbox,
                env_isolation,
                command,
            } => Self::ApprovalUpdate(ApprovalUpdate::Requested {
                approval_id,
                tool_name,
                reason,
                checkpoint_id,
                permission_profile,
                filesystem_sandbox,
                network_sandbox,
                env_isolation,
                command,
            }),
            ToolRuntimeEffect::ApprovalApproved { approval_id, note } => {
                Self::ApprovalUpdate(ApprovalUpdate::Approved { approval_id, note })
            }
            ToolRuntimeEffect::ApprovalDenied { approval_id, note } => {
                Self::ApprovalUpdate(ApprovalUpdate::Denied { approval_id, note })
            }
            ToolRuntimeEffect::UserInputRequested {
                request_id,
                thread_id,
                tool_name,
                questions,
            } => Self::UserInputUpdate(UserInputUpdate::Requested {
                request_id,
                thread_id,
                tool_name,
                questions,
            }),
            ToolRuntimeEffect::UserInputResolved {
                request_id,
                dismissed,
            } => Self::UserInputUpdate(UserInputUpdate::Resolved {
                request_id,
                dismissed,
            }),
            ToolRuntimeEffect::SubagentSpawned {
                invocation_id,
                tool_call_id,
                parent_thread_id,
                child_thread_id,
                task_name,
                message_preview,
            } => Self::SubagentSpawned(SubagentSpawned {
                invocation_id,
                tool_call_id,
                parent_thread_id,
                child_thread_id,
                task_name,
                message_preview,
            }),
            ToolRuntimeEffect::SubagentClosed {
                invocation_id,
                tool_call_id,
                parent_thread_id,
                closed_thread_id,
                agent_path,
            } => Self::SubagentClosed(SubagentClosed {
                invocation_id,
                tool_call_id,
                parent_thread_id,
                closed_thread_id,
                agent_path,
            }),
            ToolRuntimeEffect::InterAgentMessageSent {
                invocation_id,
                tool_call_id,
                author_thread_id,
                recipient_thread_id,
                author_path,
                recipient_path,
                content_preview,
                followup,
                started_turn_id,
            } => Self::InterAgentMessageSent(InterAgentMessageSent {
                invocation_id,
                tool_call_id,
                author_thread_id,
                recipient_thread_id,
                author_path,
                recipient_path,
                content_preview,
                followup,
                started_turn_id,
            }),
            ToolRuntimeEffect::ThreadGoalUpdated { goal } => Self::ThreadGoalUpdated(goal),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ExecSessionUpdate {
    Running {
        exec_session_id: ExecSessionId,
        command: String,
        cwd: PathBuf,
    },
    NotRunning {
        exec_session_id: ExecSessionId,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ApprovalUpdate {
    Requested {
        approval_id: ApprovalId,
        tool_name: String,
        reason: String,
        checkpoint_id: Option<String>,
        permission_profile: PermissionProfile,
        filesystem_sandbox: String,
        network_sandbox: String,
        env_isolation: String,
        command: Option<crate::events::ApprovalCommandPayload>,
    },
    Approved {
        approval_id: ApprovalId,
        note: Option<String>,
    },
    Denied {
        approval_id: ApprovalId,
        note: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum UserInputUpdate {
    Requested {
        request_id: ApprovalId,
        thread_id: crate::types::ThreadId,
        tool_name: String,
        questions: Vec<crate::policy::QuestionPrompt>,
    },
    Resolved {
        request_id: ApprovalId,
        dismissed: bool,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SubagentSpawned {
    invocation_id: String,
    tool_call_id: String,
    parent_thread_id: crate::types::ThreadId,
    child_thread_id: crate::types::ThreadId,
    task_name: String,
    message_preview: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SubagentClosed {
    invocation_id: String,
    tool_call_id: String,
    parent_thread_id: crate::types::ThreadId,
    closed_thread_id: crate::types::ThreadId,
    agent_path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct InterAgentMessageSent {
    invocation_id: String,
    tool_call_id: String,
    author_thread_id: crate::types::ThreadId,
    recipient_thread_id: crate::types::ThreadId,
    author_path: String,
    recipient_path: String,
    content_preview: String,
    followup: bool,
    started_turn_id: Option<crate::types::TurnId>,
}

fn apply_tool_effect(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    effect: ToolEffect,
) -> Result<()> {
    match effect {
        ToolEffect::ExecSessionUpdate(update) => recorder.apply_exec_session_update(update),
        ToolEffect::ApprovalUpdate(update) => {
            apply_approval_update(recorder, snapshot, turn_id, update)
        }
        ToolEffect::UserInputUpdate(update) => {
            apply_user_input_update(recorder, snapshot, turn_id, update)
        }
        ToolEffect::SubagentSpawned(spawned) => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::SubagentSpawned {
                    invocation_id: spawned.invocation_id,
                    tool_call_id: spawned.tool_call_id,
                    parent_thread_id: spawned.parent_thread_id,
                    child_thread_id: spawned.child_thread_id,
                    task_name: spawned.task_name,
                    message_preview: spawned.message_preview,
                },
            )?;
            Ok(())
        }
        ToolEffect::SubagentClosed(closed) => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::SubagentClosed {
                    invocation_id: closed.invocation_id,
                    tool_call_id: closed.tool_call_id,
                    parent_thread_id: closed.parent_thread_id,
                    closed_thread_id: closed.closed_thread_id,
                    agent_path: closed.agent_path,
                },
            )?;
            Ok(())
        }
        ToolEffect::InterAgentMessageSent(sent) => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::InterAgentMessageSent {
                    invocation_id: sent.invocation_id,
                    tool_call_id: sent.tool_call_id,
                    author_thread_id: sent.author_thread_id,
                    recipient_thread_id: sent.recipient_thread_id,
                    author_path: sent.author_path,
                    recipient_path: sent.recipient_path,
                    content_preview: sent.content_preview,
                    followup: sent.followup,
                    started_turn_id: sent.started_turn_id,
                },
            )?;
            Ok(())
        }
        ToolEffect::ThreadGoalUpdated(goal) => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            Ok(())
        }
        ToolEffect::ReviewSubmitted {
            ticket_id,
            goal_id,
            verdict,
            reviewed_hash,
            reject_category,
            findings,
            checkpoint_id,
        } => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ReviewSubmitted {
                    ticket_id,
                    goal_id,
                    verdict,
                    reviewed_hash,
                    reject_category,
                    findings,
                    checkpoint_id,
                },
            )?;
            Ok(())
        }
        ToolEffect::OpenQuestionRecorded {
            question_id,
            goal_id,
            question,
            blocks_what,
        } => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::OpenQuestionRecorded {
                    question_id,
                    goal_id,
                    question,
                    blocks_what,
                },
            )?;
            Ok(())
        }
        ToolEffect::OpenQuestionResolved {
            question_id,
            goal_id,
            answer,
        } => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::OpenQuestionResolved {
                    question_id,
                    goal_id,
                    answer,
                },
            )?;
            Ok(())
        }
        ToolEffect::ShortCircuit => Ok(()),
    }
}

fn apply_user_input_update(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    update: UserInputUpdate,
) -> Result<()> {
    match update {
        UserInputUpdate::Requested {
            request_id,
            thread_id: _thread_id,
            tool_name,
            questions,
        } => {
            let event_id = recorder.reserve_event_id();
            recorder.apply_user_input_requested(
                request_id.clone(),
                event_id.clone(),
                tool_name.clone(),
                questions.clone(),
            )?;
            recorder.record_reserved(
                snapshot,
                Some(turn_id),
                event_id,
                RuntimeEventKind::UserInputRequested {
                    request_id,
                    tool_name,
                    questions,
                },
            )?;
        }
        UserInputUpdate::Resolved {
            request_id,
            dismissed,
        } => {
            recorder.clear_user_input(&request_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::UserInputResolved {
                    request_id,
                    dismissed,
                },
            )?;
        }
    }

    Ok(())
}

fn apply_approval_update(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    update: ApprovalUpdate,
) -> Result<()> {
    match update {
        ApprovalUpdate::Requested {
            approval_id,
            tool_name,
            reason,
            checkpoint_id,
            permission_profile,
            filesystem_sandbox,
            network_sandbox,
            env_isolation,
            command,
        } => {
            let event_id = recorder.reserve_event_id();
            recorder.apply_approval_requested(
                approval_id.clone(),
                event_id.clone(),
                tool_name.clone(),
                reason.clone(),
                checkpoint_id.clone(),
                permission_profile,
                filesystem_sandbox.clone(),
                network_sandbox.clone(),
                env_isolation.clone(),
            )?;
            recorder.record_reserved(
                snapshot,
                Some(turn_id),
                event_id,
                RuntimeEventKind::ApprovalRequested {
                    approval_id,
                    tool_name,
                    reason,
                    checkpoint_id,
                    permission_profile,
                    filesystem_sandbox,
                    network_sandbox,
                    env_isolation,
                    command,
                },
            )?;
        }
        ApprovalUpdate::Approved { approval_id, note } => {
            recorder.clear_approval(&approval_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Approved,
                    note,
                },
            )?;
        }
        ApprovalUpdate::Denied { approval_id, note } => {
            recorder.clear_approval(&approval_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Denied,
                    note,
                },
            )?;
        }
    }

    Ok(())
}
