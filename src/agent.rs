use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::exec_session::ExecSessionManager;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::config::AgentConfig;
use crate::llm::LlmClient;
use crate::policy::PolicyManager;
use crate::registry::{ToolContext, ToolRegistry};
use crate::session::{
    ApprovalId, ApprovalStatus, ExecSessionId, ExecSessionRef, ExecSessionStatus, PendingApproval,
    SessionSnapshot,
};
use crate::types::{AssistantTurn, ConversationMessage, EventId, MessageRole, SessionId, TurnId};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
}

pub struct AgentRunOutput {
    pub final_turn: AssistantTurn,
    pub session_id: SessionId,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
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

    pub async fn run(&self, user_prompt: &str) -> Result<AssistantTurn> {
        Ok(self.run_with_meta(user_prompt).await?.final_turn)
    }

    pub async fn run_with_meta(&self, user_prompt: &str) -> Result<AgentRunOutput> {
        let snapshot = SessionSnapshot {
            session_id: crate::transcript::new_session_id(),
            workspace_root: self.config.workspace_root.clone(),
            cwd: self.config.cwd.clone(),
            conversation: vec![ConversationMessage::user(user_prompt)],
            open_exec_sessions: vec![],
            latest_compaction: None,
            pending_approvals: vec![],
        };

        self.run_session(snapshot).await
    }

    pub async fn resume(&self, session_id: &SessionId, user_prompt: &str) -> Result<AgentRunOutput> {
        let paths = crate::transcript::session_paths(&self.config.workspace_root, session_id);
        let mut snapshot: SessionSnapshot = crate::transcript::read_json(&paths.snapshot_path)?;
        snapshot.conversation.push(ConversationMessage::user(user_prompt));
        crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;

        self.run_session(snapshot).await
    }

    async fn run_session(&self, mut snapshot: SessionSnapshot) -> Result<AgentRunOutput> {
        let session_id = snapshot.session_id.clone();
        let paths = crate::transcript::session_paths(&self.config.workspace_root, &session_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;

        let mut messages = snapshot.conversation.clone();

        let ctx = ToolContext {
            config: self.config.clone(),
            session_id: Some(session_id.clone()),
            exec_sessions: self.exec_sessions.clone(),
            policy: self.policy.clone(),
        };
        let mut next_turn_index = messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::Assistant))
            .count()
            + 1;
        let mut next_event_index =
            crate::transcript::read_session_events(&self.config.workspace_root, &session_id)?.len()
                + 1;

        for _ in 0..self.config.max_turns {
            let turn = self
                .llm
                .complete(&messages, &self.registry.schemas())
                .await?;
            let turn_id = TurnId::new(format!("turn_{next_turn_index}"));
            next_turn_index += 1;
            let assistant_event = RuntimeEvent {
                event_id: EventId::new(format!("evt_{next_event_index}")),
                session_id: session_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::AssistantTurn { turn: turn.clone() },
            };
            next_event_index += 1;
            crate::transcript::append_json_line(&paths.events_path, &assistant_event)?;

            if turn.text.is_some() || !turn.tool_calls.is_empty() {
                let assistant_message = ConversationMessage::assistant(
                    turn.text.clone(),
                    turn.tool_calls.clone(),
                );
                messages.push(assistant_message.clone());
                snapshot.conversation.push(assistant_message);
                crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;
            }

            if turn.tool_calls.is_empty() {
                return Ok(AgentRunOutput {
                    final_turn: turn,
                    session_id,
                    snapshot_path: paths.snapshot_path,
                    events_path: paths.events_path,
                });
            }

            for call in turn.tool_calls.clone() {
                let result = self.registry.execute(call, Some(&ctx)).await;
                apply_exec_session_update(&mut snapshot, &result);
                apply_pending_approval_update(&mut snapshot, &result);
                let tool_event = RuntimeEvent {
                    event_id: EventId::new(format!("evt_{next_event_index}")),
                    session_id: session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    kind: RuntimeEventKind::ToolResult {
                        result: result.clone(),
                    },
                };
                next_event_index += 1;
                crate::transcript::append_json_line(&paths.events_path, &tool_event)?;

                let tool_message = ConversationMessage::tool(
                    result.tool_call_id.clone(),
                    serde_json::to_string(&result)?,
                );
                messages.push(tool_message.clone());
                snapshot.conversation.push(tool_message);
                crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;
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
    let Some(exec_session_id) = meta.get("exec_session_id").and_then(serde_json::Value::as_str) else {
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

fn apply_pending_approval_update(snapshot: &mut SessionSnapshot, result: &crate::types::ToolResult) {
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

    let requested_event_id = meta
        .get("approval_event_id")
        .and_then(serde_json::Value::as_str)
        .map(EventId::new)
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
