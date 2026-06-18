use anyhow::Result;

use super::super::ThreadSession;
use super::recording::{record_approval_decision_outcome, record_tool_outcome};
use crate::events::RuntimeEventKind;
use crate::runtime::thread_runtime::{ThreadOpResult, ThreadRuntimeError};
use crate::session::{ApprovalId, ApprovalStatus, ThreadSnapshot};
use crate::types::{ToolCall, TurnId};

impl ThreadSession {
    pub(crate) async fn handle_approval_decision(
        &mut self,
        requested_turn_id: Option<TurnId>,
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    ) -> Result<ThreadOpResult> {
        if matches!(status, ApprovalStatus::Pending) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "approval decision cannot be pending".to_string(),
            }
            .into());
        }

        let (turn_id, mut snapshot, tool_name) =
            self.resolve_pending_approval(requested_turn_id, &approval_id)?;
        let cwd = snapshot.cwd.clone();
        let tool_runtime = self
            .agent
            .tool_runtime(
                snapshot.thread_id.clone(),
                turn_id.clone(),
                snapshot.workspace_root.clone(),
                cwd,
                Some(self.recorder.exec_output_event_sink()),
                crate::runtime::agent_profile::AgentToolPolicy::all(),
                None,
            )
            .await?;
        let decision = match status {
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Pending => unreachable!("pending status is rejected above"),
        };
        let call = ToolCall {
            id: format!("approval_decision_{}", approval_id.as_str()),
            name: tool_name,
            arguments: serde_json::json!({
                "approval_id": approval_id.as_str(),
                "decision": decision,
            }),
            thought_signature: None,
        };
        let mut outcome = tool_runtime
            .execute_with_lifecycle(call, &mut self.recorder, &snapshot, &turn_id)
            .await?;
        outcome.attach_approval_note(&approval_id, note.clone());
        if !outcome.approval_matches(&approval_id, &status) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: outcome.result.content,
            }
            .into());
        }
        record_approval_decision_outcome(&mut self.recorder, &mut snapshot, &turn_id, outcome)?;

        Ok(ThreadOpResult::ApprovalDecision {
            turn_id,
            approval_id,
            status,
        })
    }

    pub(crate) async fn handle_user_input_response(
        &mut self,
        requested_turn_id: Option<TurnId>,
        request_id: ApprovalId,
        dismissed: bool,
    ) -> Result<ThreadOpResult> {
        let (turn_id, mut snapshot, tool_name) =
            self.resolve_pending_user_input(requested_turn_id, &request_id)?;
        let cwd = snapshot.cwd.clone();
        let tool_runtime = self
            .agent
            .tool_runtime(
                snapshot.thread_id.clone(),
                turn_id.clone(),
                snapshot.workspace_root.clone(),
                cwd,
                Some(self.recorder.exec_output_event_sink()),
                crate::runtime::agent_profile::AgentToolPolicy::all(),
                None,
            )
            .await?;
        let decision = if dismissed { "dismissed" } else { "answered" };
        let call = ToolCall {
            id: format!("user_input_response_{}", request_id.as_str()),
            name: tool_name,
            arguments: serde_json::json!({
                "request_id": request_id.as_str(),
                "decision": decision,
            }),
            thought_signature: None,
        };
        let outcome = tool_runtime
            .execute_with_lifecycle(call, &mut self.recorder, &snapshot, &turn_id)
            .await?;
        if !outcome.user_input_resolved_matches(&request_id, dismissed) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: outcome.result.content,
            }
            .into());
        }
        record_tool_outcome(
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            &turn_id,
            outcome,
        )?;

        Ok(ThreadOpResult::UserInputSubmitted {
            turn_id,
            request_id,
            dismissed,
        })
    }

    fn resolve_pending_approval(
        &self,
        requested_turn_id: Option<TurnId>,
        approval_id: &ApprovalId,
    ) -> Result<(TurnId, ThreadSnapshot, String)> {
        let state = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        let tool_name = state
            .overlay
            .pending_approvals
            .iter()
            .find(|approval| &approval.approval_id == approval_id)
            .map(|approval| approval.tool_name.clone());
        let Some(tool_name) = tool_name else {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: format!("unknown approval id: {}", approval_id.as_str()),
            }
            .into());
        };
        let approval_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested {
                    approval_id: event_approval_id,
                    ..
                } if event_approval_id == approval_id => event.turn_id.clone(),
                _ => None,
            });
        let latest_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| event.turn_id.clone());
        let resolved_turn_id = requested_turn_id
            .or(approval_turn_id)
            .or(latest_turn_id)
            .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "approval has no turn id".to_string(),
            })?;
        if let Some(event_turn_id) = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested {
                    approval_id: event_approval_id,
                    ..
                } if event_approval_id == approval_id => event.turn_id.clone(),
                _ => None,
            })
        {
            if event_turn_id != resolved_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("approval turn is {}", event_turn_id.as_str()),
                }
                .into());
            }
        }

        Ok((resolved_turn_id, state.snapshot.clone(), tool_name))
    }

    fn resolve_pending_user_input(
        &self,
        requested_turn_id: Option<TurnId>,
        request_id: &ApprovalId,
    ) -> Result<(TurnId, ThreadSnapshot, String)> {
        let state = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        let tool_name = state
            .overlay
            .pending_user_inputs
            .iter()
            .find(|input| &input.request_id == request_id)
            .map(|input| input.tool_name.clone());
        let Some(tool_name) = tool_name else {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: format!("unknown user input request id: {}", request_id.as_str()),
            }
            .into());
        };
        let request_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::UserInputRequested {
                    request_id: event_request_id,
                    ..
                } if event_request_id == request_id => event.turn_id.clone(),
                _ => None,
            });
        let latest_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| event.turn_id.clone());
        let resolved_turn_id = requested_turn_id
            .or(request_turn_id)
            .or(latest_turn_id)
            .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "user input request has no turn id".to_string(),
            })?;
        if let Some(event_turn_id) = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::UserInputRequested {
                    request_id: event_request_id,
                    ..
                } if event_request_id == request_id => event.turn_id.clone(),
                _ => None,
            })
        {
            if event_turn_id != resolved_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("user input request turn is {}", event_turn_id.as_str()),
                }
                .into());
            }
        }

        Ok((resolved_turn_id, state.snapshot.clone(), tool_name))
    }
}
