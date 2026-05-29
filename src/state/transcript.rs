//! Generic JSON file helpers plus thread id generation.
//!
//! Runtime state is restored from `.exagent/threads/<thread_id>/rollout.jsonl`.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::ThreadId;

static THREAD_COUNTER: AtomicU64 = AtomicU64::new(0);

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

pub fn new_thread_id() -> ThreadId {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = THREAD_COUNTER.fetch_add(1, Ordering::Relaxed);
    ThreadId::new(format!("thread-{ts}-{counter}"))
}
