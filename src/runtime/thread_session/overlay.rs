use crate::session::{
    ApprovalId, ApprovalStatus, ExecSessionRef, ExecSessionStatus, PendingApproval,
};
use crate::types::EventId;

use super::super::tool_call_runtime::ExecSessionUpdate;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RuntimeOverlay {
    pub(crate) open_exec_sessions: Vec<ExecSessionRef>,
    pub(crate) pending_approvals: Vec<PendingApproval>,
}

impl RuntimeOverlay {
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
    ) {
        self.clear_approval(&approval_id);
        self.pending_approvals.push(PendingApproval {
            approval_id,
            requested_event_id,
            tool_name,
            reason,
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

    pub(crate) fn has_pending_approval(&self) -> bool {
        self.pending_approvals
            .iter()
            .any(|approval| matches!(approval.status, ApprovalStatus::Pending))
    }
}
