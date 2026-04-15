use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::session::AgentRole;
use crate::types::{EventId, SessionId, TurnId};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub session_dir: PathBuf,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read JSON file at {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse JSON file at {}", path.display()))
}

pub fn read_json_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(vec![]);
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read JSONL file at {}", path.display()))?;

    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("Failed to parse JSONL line"))
        .collect()
}

pub fn new_session_id() -> SessionId {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    SessionId::new(format!("session-{ts}-{counter}"))
}

pub fn session_paths(workspace_root: &Path, session_id: &SessionId) -> SessionPaths {
    let session_dir = workspace_root
        .join(".exagent")
        .join("sessions")
        .join(session_id.as_str());
    SessionPaths {
        snapshot_path: session_dir.join("snapshot.json"),
        events_path: session_dir.join("events.jsonl"),
        session_dir,
    }
}

pub fn read_session_events(
    workspace_root: &Path,
    session_id: &SessionId,
) -> Result<Vec<RuntimeEvent>> {
    let paths = session_paths(workspace_root, session_id);
    read_json_lines(&paths.events_path)
}

pub fn replay_session(workspace_root: &Path, session_id: &SessionId) -> Result<Vec<RuntimeEvent>> {
    read_session_events(workspace_root, session_id)
}

pub fn append_runtime_event(
    workspace_root: &Path,
    session_id: &SessionId,
    turn_id: Option<&TurnId>,
    kind: RuntimeEventKind,
) -> Result<RuntimeEvent> {
    let next_event_id = EventId::new(format!(
        "evt_{}",
        read_session_events(workspace_root, session_id)?.len() + 1
    ));
    let event = RuntimeEvent {
        event_id: next_event_id,
        session_id: session_id.clone(),
        turn_id: turn_id.cloned(),
        kind,
    };
    let paths = session_paths(workspace_root, session_id);
    append_json_line(&paths.events_path, &event)?;
    Ok(event)
}

pub fn record_session_spawn(
    workspace_root: &Path,
    parent_session_id: &SessionId,
    child_session_id: &SessionId,
    agent_role: AgentRole,
    spawned_by_turn_id: Option<&TurnId>,
) -> Result<()> {
    let parent_event_kind = RuntimeEventKind::SessionSpawned {
        child_session_id: child_session_id.clone(),
        parent_session_id: parent_session_id.clone(),
        agent_role: agent_role.clone(),
        spawned_by_turn_id: spawned_by_turn_id.cloned(),
    };
    append_runtime_event(
        workspace_root,
        parent_session_id,
        spawned_by_turn_id,
        parent_event_kind,
    )?;

    let child_event_kind = RuntimeEventKind::SessionSpawned {
        child_session_id: child_session_id.clone(),
        parent_session_id: parent_session_id.clone(),
        agent_role,
        spawned_by_turn_id: spawned_by_turn_id.cloned(),
    };
    append_runtime_event(workspace_root, child_session_id, None, child_event_kind)?;

    Ok(())
}
