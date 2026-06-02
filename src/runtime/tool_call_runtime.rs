use std::path::PathBuf;
use std::sync::Arc;

use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::policy::PolicyManager;
use crate::registry::{ToolContext, ToolRegistry};
use crate::session::{ApprovalId, ExecSessionId};
use crate::types::{ThreadId, ToolCall, ToolResult, TurnId};

pub(crate) struct ToolCallRuntime {
    config: AgentConfig,
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    thread_id: ThreadId,
    turn_id: TurnId,
    cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionOutcome {
    pub(crate) result: ToolResult,
    pub(crate) effects: Vec<ToolEffect>,
}

#[derive(Debug, Clone)]
pub(crate) enum ToolEffect {
    ExecSessionUpdate(ExecSessionUpdate),
    ApprovalUpdate(ApprovalUpdate),
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

impl ToolCallRuntime {
    pub(crate) fn new(
        config: AgentConfig,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
        policy: Arc<PolicyManager>,
        thread_id: ThreadId,
        turn_id: TurnId,
        cwd: PathBuf,
    ) -> Self {
        Self {
            config,
            registry,
            exec_sessions,
            policy,
            thread_id,
            turn_id,
            cwd,
        }
    }

    pub(crate) fn schemas(&self) -> Vec<serde_json::Value> {
        self.registry.schemas()
    }

    pub(crate) async fn execute(&self, call: ToolCall) -> ToolExecutionOutcome {
        let ctx = ToolContext {
            config: self.config.clone(),
            thread_id: Some(self.thread_id.clone()),
            turn_id: Some(self.turn_id.clone()),
            exec_sessions: self.exec_sessions.clone(),
            policy: self.policy.clone(),
        };
        let result = self.registry.execute(call, Some(&ctx)).await;
        let effects = tool_effects_from_result(&result, self.cwd.clone());
        ToolExecutionOutcome { result, effects }
    }
}

fn tool_effects_from_result(result: &ToolResult, fallback_cwd: PathBuf) -> Vec<ToolEffect> {
    let mut effects = Vec::new();
    if let Some(effect) = exec_session_effect_from_result(result, fallback_cwd) {
        effects.push(ToolEffect::ExecSessionUpdate(effect));
    }
    if let Some(effect) = approval_effect_from_result(result) {
        effects.push(ToolEffect::ApprovalUpdate(effect));
    }
    effects
}

fn exec_session_effect_from_result(
    result: &ToolResult,
    fallback_cwd: PathBuf,
) -> Option<ExecSessionUpdate> {
    let meta = result.meta.as_ref()?;
    let exec_session_id = meta
        .get("exec_session_id")
        .and_then(serde_json::Value::as_str)?;
    let lifecycle = meta.get("lifecycle").and_then(serde_json::Value::as_str)?;
    let exec_session_id = ExecSessionId::new(exec_session_id);

    if lifecycle != "running" {
        return Some(ExecSessionUpdate::NotRunning { exec_session_id });
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
        .unwrap_or(fallback_cwd);

    Some(ExecSessionUpdate::Running {
        exec_session_id,
        command,
        cwd,
    })
}

fn approval_effect_from_result(result: &ToolResult) -> Option<ApprovalUpdate> {
    let meta = result.meta.as_ref()?;
    let approval_id = meta
        .get("approval_id")
        .and_then(serde_json::Value::as_str)?;
    let approval_status = meta
        .get("approval_status")
        .and_then(serde_json::Value::as_str)?;
    let approval_id = ApprovalId::new(approval_id);

    match approval_status {
        "pending" => {
            let reason = meta
                .get("approval_reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("approval required")
                .to_string();
            Some(ApprovalUpdate::Requested {
                approval_id,
                tool_name: result.tool_name.clone(),
                reason,
            })
        }
        "approved" => Some(ApprovalUpdate::Approved {
            approval_id,
            note: None,
        }),
        "denied" => Some(ApprovalUpdate::Denied {
            approval_id,
            note: None,
        }),
        _ => None,
    }
}
