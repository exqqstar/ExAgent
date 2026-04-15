use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::events::RuntimeEventKind;
use crate::session::{AgentRole, SessionSnapshot};
use crate::types::{MessageRole, SessionId, ToolResult, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildLifecycleStatus {
    Completed,
    Running,
    WaitingApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionSummary {
    pub session_id: SessionId,
    pub parent_session_id: SessionId,
    pub root_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by_turn_id: Option<TurnId>,
    pub agent_role: AgentRole,
    pub status: ChildLifecycleStatus,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectedOutputKind {
    AssistantText,
    ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectedOutput {
    pub kind: CollectedOutputKind,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectedChildSession {
    pub child: ChildSessionSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_useful_output: Option<CollectedOutput>,
}

pub fn inspect_children(
    workspace_root: &Path,
    parent_session_id: &SessionId,
) -> Result<Vec<ChildSessionSummary>> {
    crate::transcript::direct_child_session_ids(workspace_root, parent_session_id)?
        .into_iter()
        .map(|child_session_id| inspect_child_session(workspace_root, &child_session_id))
        .collect()
}

pub fn collect_session(
    workspace_root: &Path,
    session_id: &SessionId,
) -> Result<CollectedChildSession> {
    let child = inspect_child_session(workspace_root, session_id)?;
    let snapshot = crate::transcript::read_session_snapshot(workspace_root, session_id)?;

    Ok(CollectedChildSession {
        child,
        latest_useful_output: latest_useful_output(workspace_root, session_id, &snapshot)?,
    })
}

fn inspect_child_session(
    workspace_root: &Path,
    session_id: &SessionId,
) -> Result<ChildSessionSummary> {
    let snapshot = crate::transcript::read_session_snapshot(workspace_root, session_id)?;
    let parent_session_id = snapshot
        .parent_session_id
        .clone()
        .ok_or_else(|| anyhow!("session {} is not a child session", session_id.as_str()))?;
    let paths = crate::transcript::session_paths(workspace_root, session_id);
    let status = derive_lifecycle_status(&snapshot);

    Ok(ChildSessionSummary {
        session_id: snapshot.session_id,
        parent_session_id,
        root_session_id: snapshot.root_session_id,
        spawned_by_turn_id: snapshot.spawned_by_turn_id,
        agent_role: snapshot.agent_role,
        status,
        snapshot_path: paths.snapshot_path,
        events_path: paths.events_path,
    })
}

fn derive_lifecycle_status(snapshot: &SessionSnapshot) -> ChildLifecycleStatus {
    if !snapshot.pending_approvals.is_empty() {
        ChildLifecycleStatus::WaitingApproval
    } else if !snapshot.open_exec_sessions.is_empty() {
        ChildLifecycleStatus::Running
    } else {
        ChildLifecycleStatus::Completed
    }
}

fn latest_useful_output(
    workspace_root: &Path,
    session_id: &SessionId,
    snapshot: &SessionSnapshot,
) -> Result<Option<CollectedOutput>> {
    if let Some(content) = latest_assistant_text(snapshot) {
        return Ok(Some(CollectedOutput {
            kind: CollectedOutputKind::AssistantText,
            content,
            tool_name: None,
            tool_call_id: None,
        }));
    }

    if let Some(result) = latest_tool_result(workspace_root, session_id)? {
        return Ok(Some(CollectedOutput {
            kind: CollectedOutputKind::ToolResult,
            content: result.content,
            tool_name: Some(result.tool_name),
            tool_call_id: Some(result.tool_call_id),
        }));
    }

    Ok(None)
}

fn latest_assistant_text(snapshot: &SessionSnapshot) -> Option<String> {
    snapshot.conversation.iter().rev().find_map(|message| {
        if message.role == MessageRole::Assistant && !message.content.trim().is_empty() {
            Some(message.content.clone())
        } else {
            None
        }
    })
}

fn latest_tool_result(workspace_root: &Path, session_id: &SessionId) -> Result<Option<ToolResult>> {
    Ok(
        crate::transcript::read_session_events(workspace_root, session_id)?
            .into_iter()
            .rev()
            .find_map(|event| match event.kind {
                RuntimeEventKind::ToolResult { result } => Some(result),
                _ => None,
            }),
    )
}
