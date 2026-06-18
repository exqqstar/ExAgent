use std::collections::HashMap;

use crate::config::PermissionProfile;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::session::{
    ApprovalId, ApprovalStatus, ExecSessionRef, ExecSessionStatus, PendingApproval,
    PendingUserInput,
};
use crate::types::EventId;

use crate::runtime::tool::orchestrator::ExecSessionUpdate;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RuntimeOverlay {
    pub(crate) open_exec_sessions: Vec<ExecSessionRef>,
    pub(crate) pending_approvals: Vec<PendingApproval>,
    pub(crate) pending_user_inputs: Vec<PendingUserInput>,
    pub(crate) active_tool_invocations: Vec<ActiveToolInvocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveToolInvocation {
    pub(crate) invocation_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
}

impl RuntimeOverlay {
    pub(crate) fn from_events(events: &[RuntimeEvent]) -> Self {
        let mut overlay = Self::default();
        let mut invocation_approvals: HashMap<String, ApprovalId> = HashMap::new();
        let mut invocation_user_inputs: HashMap<String, ApprovalId> = HashMap::new();

        for event in events {
            match &event.kind {
                RuntimeEventKind::ToolInvocationWaitingApproval {
                    invocation_id,
                    approval_id,
                    ..
                } => {
                    invocation_approvals.insert(invocation_id.clone(), approval_id.clone());
                }
                RuntimeEventKind::ToolInvocationWaitingUserInput {
                    invocation_id,
                    request_id,
                    ..
                } => {
                    invocation_user_inputs.insert(invocation_id.clone(), request_id.clone());
                }
                RuntimeEventKind::ToolInvocationCompleted { invocation_id, .. }
                | RuntimeEventKind::ToolInvocationFailed { invocation_id, .. }
                | RuntimeEventKind::ToolInvocationCancelled { invocation_id, .. } => {
                    if let Some(approval_id) = invocation_approvals.remove(invocation_id) {
                        overlay.clear_approval(&approval_id);
                    }
                    if let Some(request_id) = invocation_user_inputs.remove(invocation_id) {
                        overlay.clear_user_input(&request_id);
                    }
                }
                RuntimeEventKind::ApprovalRequested {
                    approval_id,
                    tool_name,
                    reason,
                    checkpoint_id,
                    permission_profile,
                    filesystem_sandbox,
                    network_sandbox,
                    env_isolation,
                    ..
                } => overlay.apply_approval_requested(
                    approval_id.clone(),
                    event.event_id.clone(),
                    tool_name.clone(),
                    reason.clone(),
                    checkpoint_id.clone(),
                    *permission_profile,
                    filesystem_sandbox.clone(),
                    network_sandbox.clone(),
                    env_isolation.clone(),
                ),
                RuntimeEventKind::ApprovalDecision { approval_id, .. } => {
                    overlay.clear_approval(approval_id);
                    invocation_approvals.retain(|_, id| id != approval_id);
                }
                RuntimeEventKind::UserInputRequested {
                    request_id,
                    tool_name,
                    questions,
                } => overlay.apply_user_input_requested(
                    request_id.clone(),
                    event.event_id.clone(),
                    tool_name.clone(),
                    questions.clone(),
                ),
                RuntimeEventKind::UserInputResolved { request_id, .. } => {
                    overlay.clear_user_input(request_id);
                    invocation_user_inputs.retain(|_, id| id != request_id);
                }
                RuntimeEventKind::TurnInterrupted => {
                    overlay.clear_pending_approvals();
                    overlay.clear_pending_user_inputs();
                    invocation_approvals.clear();
                    invocation_user_inputs.clear();
                }
                _ => {}
            }
        }

        overlay
    }

    pub(crate) fn apply_exec_session_update(&mut self, update: ExecSessionUpdate) {
        let exec_session_id = match &update {
            ExecSessionUpdate::Running {
                exec_session_id, ..
            }
            | ExecSessionUpdate::NotRunning { exec_session_id } => exec_session_id.clone(),
        };
        self.open_exec_sessions
            .retain(|entry| entry.exec_session_id != exec_session_id);

        if let ExecSessionUpdate::Running {
            exec_session_id,
            command,
            cwd,
        } = update
        {
            self.open_exec_sessions.push(ExecSessionRef {
                exec_session_id,
                command,
                cwd,
                status: ExecSessionStatus::Running,
            });
        }
    }

    pub(crate) fn apply_approval_requested(
        &mut self,
        approval_id: ApprovalId,
        requested_event_id: EventId,
        tool_name: String,
        reason: String,
        checkpoint_id: Option<String>,
        permission_profile: PermissionProfile,
        filesystem_sandbox: String,
        network_sandbox: String,
        env_isolation: String,
    ) {
        self.clear_approval(&approval_id);
        self.pending_approvals.push(PendingApproval {
            approval_id,
            requested_event_id,
            tool_name,
            reason,
            checkpoint_id,
            permission_profile,
            filesystem_sandbox,
            network_sandbox,
            env_isolation,
            status: ApprovalStatus::Pending,
        });
    }

    pub(crate) fn clear_approval(&mut self, approval_id: &ApprovalId) {
        self.pending_approvals
            .retain(|entry| &entry.approval_id != approval_id);
    }

    pub(crate) fn clear_pending_approvals(&mut self) {
        self.pending_approvals.clear();
    }

    pub(crate) fn apply_user_input_requested(
        &mut self,
        request_id: ApprovalId,
        requested_event_id: EventId,
        tool_name: String,
        questions: Vec<crate::policy::QuestionPrompt>,
    ) {
        self.clear_user_input(&request_id);
        self.pending_user_inputs.push(PendingUserInput {
            request_id,
            requested_event_id,
            tool_name,
            questions,
            status: ApprovalStatus::Pending,
        });
    }

    pub(crate) fn clear_user_input(&mut self, request_id: &ApprovalId) {
        self.pending_user_inputs
            .retain(|entry| &entry.request_id != request_id);
    }

    pub(crate) fn clear_pending_user_inputs(&mut self) {
        self.pending_user_inputs.clear();
    }

    pub(crate) fn has_pending_approval(&self) -> bool {
        self.pending_approvals
            .iter()
            .any(|approval| matches!(approval.status, ApprovalStatus::Pending))
    }

    pub(crate) fn has_pending_user_input(&self) -> bool {
        self.pending_user_inputs
            .iter()
            .any(|input| matches!(input.status, ApprovalStatus::Pending))
    }

    pub(crate) fn has_pending_approval_id(&self, approval_id: &ApprovalId) -> bool {
        self.pending_approvals.iter().any(|approval| {
            matches!(approval.status, ApprovalStatus::Pending)
                && approval.approval_id == *approval_id
        })
    }

    pub(crate) fn has_pending_user_input_id(&self, request_id: &ApprovalId) -> bool {
        self.pending_user_inputs.iter().any(|input| {
            matches!(input.status, ApprovalStatus::Pending) && input.request_id == *request_id
        })
    }

    pub(crate) fn mark_tool_invocation_active(&mut self, invocation: ActiveToolInvocation) {
        self.clear_tool_invocation(&invocation.invocation_id);
        self.active_tool_invocations.push(invocation);
    }

    pub(crate) fn clear_tool_invocation(&mut self, invocation_id: &str) {
        self.active_tool_invocations
            .retain(|entry| entry.invocation_id != invocation_id);
    }

    pub(crate) fn has_tool_invocation(&self, invocation_id: &str) -> bool {
        self.active_tool_invocations
            .iter()
            .any(|entry| entry.invocation_id == invocation_id)
    }

    pub(crate) fn take_active_tool_invocations(&mut self) -> Vec<ActiveToolInvocation> {
        std::mem::take(&mut self.active_tool_invocations)
    }
}
