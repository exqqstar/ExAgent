use std::path::{Path, PathBuf};

use crate::state::rollout::{RolloutItem, RolloutStore};
use crate::types::{MessageRole, ThreadId};

use super::{time, IndexDb, ReindexReport};

pub async fn reindex_project(
    db: &IndexDb,
    project_id: &str,
    project_path: &Path,
) -> anyhow::Result<ReindexReport> {
    let threads_root = project_path.join(".exagent").join("threads");
    if !tokio::fs::try_exists(&threads_root).await.unwrap_or(false) {
        return Ok(ReindexReport {
            scanned_threads: 0,
            indexed_threads: 0,
            stale_threads: 0,
        });
    }

    let mut scanned_threads = 0;
    let mut indexed_threads = 0;
    let mut entries = tokio::fs::read_dir(&threads_root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let rollout_path = entry.path().join("rollout.jsonl");
        if !tokio::fs::try_exists(&rollout_path).await.unwrap_or(false) {
            continue;
        }
        scanned_threads += 1;
        if let Some(summary) = summarize_rollout(&rollout_path).await? {
            upsert_thread_summary(db, project_id, rollout_path, summary).await?;
            indexed_threads += 1;
        }
    }

    Ok(ReindexReport {
        scanned_threads,
        indexed_threads,
        stale_threads: 0,
    })
}

struct RolloutSummary {
    thread_id: ThreadId,
    fallback_title: String,
    preview: String,
    created_at: i64,
    updated_at: i64,
}

async fn summarize_rollout(path: &Path) -> anyhow::Result<Option<RolloutSummary>> {
    let items = RolloutStore::read_items(path).await?;
    let Some(meta) = items.iter().find_map(|item| match item {
        RolloutItem::ThreadMeta(meta) => Some(meta),
        _ => None,
    }) else {
        return Ok(None);
    };
    let first_user = items.iter().find_map(|item| match item {
        RolloutItem::ResponseItem(message) if message.role == MessageRole::User => {
            Some(message.content.clone())
        }
        _ => None,
    });
    let title = first_user
        .as_deref()
        .map(shorten_title)
        .unwrap_or_else(|| short_thread_id(meta.thread_id.as_str()));
    let preview = first_user.unwrap_or_else(|| title.clone());
    let updated_at = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(time::now_unix_millis);
    Ok(Some(RolloutSummary {
        thread_id: meta.thread_id.clone(),
        fallback_title: title,
        preview,
        created_at: updated_at,
        updated_at,
    }))
}

fn shorten_title(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(80).collect()
}

fn short_thread_id(id: &str) -> String {
    format!("Session {}", id.chars().take(12).collect::<String>())
}

async fn upsert_thread_summary(
    db: &IndexDb,
    project_id: &str,
    rollout_path: PathBuf,
    summary: RolloutSummary,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'rollout', 0, 'idle', ?, ?)
ON CONFLICT(id) DO UPDATE SET
  project_id = excluded.project_id,
  rollout_path = excluded.rollout_path,
  fallback_title = excluded.fallback_title,
  preview = excluded.preview,
  updated_at = excluded.updated_at
        "#,
    )
    .bind(summary.thread_id.as_str())
    .bind(project_id)
    .bind(rollout_path.display().to_string())
    .bind(summary.fallback_title)
    .bind(summary.preview)
    .bind(summary.created_at)
    .bind(summary.updated_at)
    .execute(db.pool())
    .await?;
    Ok(())
}
