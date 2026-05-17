use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::config::AgentConfig;
use crate::events::RuntimeEventKind;
use crate::exec_session::ExecSessionManager;
use crate::llm::LlmClient;
use crate::policy::PolicyManager;
use crate::registry::{ToolContext, ToolRegistry};
use crate::runtime::thread_session::LiveEventSink;
use crate::session::{
    ApprovalId, ApprovalStatus, ExecSessionId, ExecSessionRef, ExecSessionStatus, PendingApproval,
    SessionSnapshot,
};
use crate::types::{AssistantTurn, ConversationMessage, EventId, TurnId};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Box<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self::with_runtime(
            config,
            llm,
            registry,
            Arc::new(ExecSessionManager::default()),
            Arc::new(PolicyManager::default()),
        )
    }

    pub fn with_exec_sessions(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
    ) -> Self {
        Self::with_runtime(
            config,
            llm,
            registry,
            exec_sessions,
            Arc::new(PolicyManager::default()),
        )
    }

    pub fn with_runtime(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
        policy: Arc<PolicyManager>,
    ) -> Self {
        Self {
            config,
            llm,
            registry,
            exec_sessions,
            policy,
        }
    }

    pub(crate) async fn run_live_turn(
        &self,
        snapshot: &mut SessionSnapshot,
        runtime_turn_id: TurnId,
        turn_cwd: Option<PathBuf>,
        sink: &mut dyn LiveEventSink,
    ) -> Result<AssistantTurn> {
        snapshot.normalize_lineage();
        let mut session_config = self.config.clone();
        session_config.workspace_root = snapshot.workspace_root.clone();
        session_config.cwd = turn_cwd.unwrap_or_else(|| snapshot.cwd.clone());

        let session_id = snapshot.session_id.clone();
        let mut messages = snapshot.conversation.clone();

        let base_ctx = ToolContext {
            config: session_config,
            session_id: Some(session_id.clone()),
            turn_id: None,
            exec_sessions: self.exec_sessions.clone(),
            policy: self.policy.clone(),
            defer_policy_events: true,
        };
        for _ in 0..self.config.max_turns {
            let turn = self
                .llm
                .complete(&messages, &self.registry.schemas())
                .await?;

            // Push the assistant message into the live snapshot before the
            // sink records the AssistantTurn event so the checkpoint sink
            // performs sees the message already.
            if turn.text.is_some() || !turn.tool_calls.is_empty() {
                let assistant_message =
                    ConversationMessage::assistant(turn.text.clone(), turn.tool_calls.clone());
                messages.push(assistant_message.clone());
                snapshot.conversation.push(assistant_message);
            }
            sink.record(
                snapshot,
                Some(&runtime_turn_id),
                RuntimeEventKind::AssistantTurn { turn: turn.clone() },
            )?;

            if turn.tool_calls.is_empty() {
                return Ok(turn);
            }

            for call in turn.tool_calls.clone() {
                let mut ctx = base_ctx.clone();
                ctx.turn_id = Some(runtime_turn_id.clone());
                let result = self.registry.execute(call, Some(&ctx)).await;
                apply_exec_session_update(snapshot, &result);
                record_deferred_policy_event(snapshot, &result, &runtime_turn_id, sink)?;

                let tool_message = ConversationMessage::tool(
                    result.tool_call_id.clone(),
                    serde_json::to_string(&result)?,
                );
                messages.push(tool_message.clone());
                snapshot.conversation.push(tool_message);

                sink.record(
                    snapshot,
                    Some(&runtime_turn_id),
                    RuntimeEventKind::ToolResult {
                        result: result.clone(),
                    },
                )?;
            }
        }

        Err(anyhow!(
            "Agent reached max turns ({}) without a final assistant turn",
            self.config.max_turns
        ))
    }
}

fn apply_exec_session_update(snapshot: &mut SessionSnapshot, result: &crate::types::ToolResult) {
    let Some(meta) = result.meta.as_ref() else {
        return;
    };
    let Some(exec_session_id) = meta
        .get("exec_session_id")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };
    let Some(lifecycle) = meta.get("lifecycle").and_then(serde_json::Value::as_str) else {
        return;
    };

    let exec_session_id = ExecSessionId::new(exec_session_id);
    snapshot
        .open_exec_sessions
        .retain(|entry| entry.exec_session_id != exec_session_id);

    if lifecycle != "running" {
        return;
    }

    let command = meta
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cwd = meta
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| snapshot.cwd.clone());

    snapshot.open_exec_sessions.push(ExecSessionRef {
        exec_session_id,
        command,
        cwd,
        status: ExecSessionStatus::Running,
    });
}

fn record_deferred_policy_event(
    snapshot: &mut SessionSnapshot,
    result: &crate::types::ToolResult,
    turn_id: &TurnId,
    sink: &mut dyn LiveEventSink,
) -> Result<()> {
    let Some(meta) = result.meta.as_ref() else {
        return Ok(());
    };
    let Some(approval_id) = meta.get("approval_id").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let Some(approval_status) = meta
        .get("approval_status")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(());
    };

    let approval_id = ApprovalId::new(approval_id);
    match approval_status {
        "pending" => {
            let event_id = sink.reserve_event_id();
            apply_pending_approval_update(snapshot, result, Some(event_id.clone()));
            let reason = meta
                .get("approval_reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("approval required")
                .to_string();
            sink.record_reserved(
                snapshot,
                Some(turn_id),
                event_id,
                RuntimeEventKind::ApprovalRequested {
                    approval_id,
                    tool_name: result.tool_name.clone(),
                    reason,
                },
            )?;
        }
        "approved" => {
            apply_pending_approval_update(snapshot, result, None);
            sink.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Approved,
                    note: None,
                },
            )?;
        }
        "denied" => {
            apply_pending_approval_update(snapshot, result, None);
            sink.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Denied,
                    note: None,
                },
            )?;
        }
        _ => return Ok(()),
    }

    Ok(())
}

fn apply_pending_approval_update(
    snapshot: &mut SessionSnapshot,
    result: &crate::types::ToolResult,
    requested_event_id: Option<EventId>,
) {
    let Some(meta) = result.meta.as_ref() else {
        return;
    };
    let Some(approval_id) = meta.get("approval_id").and_then(serde_json::Value::as_str) else {
        return;
    };
    let Some(approval_status) = meta
        .get("approval_status")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };

    let approval_id = ApprovalId::new(approval_id);
    snapshot
        .pending_approvals
        .retain(|entry| entry.approval_id != approval_id);

    if approval_status != "pending" {
        return;
    }

    let requested_event_id = requested_event_id
        .or_else(|| {
            meta.get("approval_event_id")
                .and_then(serde_json::Value::as_str)
                .map(EventId::new)
        })
        .unwrap_or_else(|| EventId::new("approval_evt_unknown"));
    let reason = meta
        .get("approval_reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("approval required")
        .to_string();

    snapshot.pending_approvals.push(PendingApproval {
        approval_id,
        requested_event_id,
        tool_name: result.tool_name.clone(),
        reason,
        status: ApprovalStatus::Pending,
    });
}
