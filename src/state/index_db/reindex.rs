use std::path::{Path, PathBuf};

use crate::session::ThreadSource;
use crate::state::rollout::ThreadMeta;
use crate::types::ThreadId;

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
            if summary.thread_source == ThreadSource::Subagent {
                delete_thread_summary(db, project_id, &summary.thread_id).await?;
                continue;
            }
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
    thread_source: ThreadSource,
    fallback_title: String,
    preview: String,
    created_at: i64,
    updated_at: i64,
}

async fn summarize_rollout(path: &Path) -> anyhow::Result<Option<RolloutSummary>> {
    let contents = tokio::fs::read_to_string(path).await?;
    let mut meta = None;
    let mut first_user = None;

    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("thread_meta") if meta.is_none() => {
                let Some(payload) = value.get("payload").cloned() else {
                    continue;
                };
                match serde_json::from_value::<ThreadMeta>(payload) {
                    Ok(thread_meta) => meta = Some(thread_meta),
                    Err(_) => return Ok(None),
                }
            }
            Some("response_item") if first_user.is_none() => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("role").and_then(serde_json::Value::as_str) == Some("user") {
                    first_user = payload
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
            _ => {}
        }
    }

    let Some(meta) = meta else {
        return Ok(None);
    };
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
        thread_source: meta.thread_source.clone(),
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

async fn delete_thread_summary(
    db: &IndexDb,
    project_id: &str,
    thread_id: &ThreadId,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM threads WHERE project_id = ? AND id = ?")
        .bind(project_id)
        .bind(thread_id.as_str())
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    use crate::config::PermissionProfile;
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadListFilter};
    use serde_json::json;

    use crate::state::rollout::{ResponseItem, RolloutItem, RolloutStore, ThreadMeta};
    use crate::types::{ConversationMessage, TurnId};

    async fn temp_db() -> (tempfile::TempDir, IndexDb) {
        let dir = tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .expect("open index db");
        (dir, db)
    }

    fn write_rollout(
        workspace_root: &Path,
        thread_id: &str,
        thread_source: ThreadSource,
        user_message: &str,
    ) {
        let thread_id = ThreadId::new(thread_id);
        let rollout_path = workspace_root
            .join(".exagent")
            .join("threads")
            .join(thread_id.as_str())
            .join("rollout.jsonl");
        let meta = ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: workspace_root.to_path_buf(),
            initial_cwd: workspace_root.to_path_buf(),
            permission_profile: PermissionProfile::default(),
            thread_source,
            lineage: None,
            created_at: "2026-06-06T00:00:00Z".to_string(),
        };
        RolloutStore::new(rollout_path)
            .append_items_blocking(&[
                RolloutItem::ThreadMeta(meta),
                RolloutItem::ResponseItem(ResponseItem::for_turn(
                    TurnId::new("turn_1"),
                    ConversationMessage::user(user_message),
                )),
            ])
            .expect("write rollout");
    }

    async fn write_legacy_rollout_without_turn_context_turn_id(
        workspace_root: &Path,
        thread_id: &str,
        user_message: &str,
    ) {
        let thread_dir = workspace_root
            .join(".exagent")
            .join("threads")
            .join(thread_id);
        tokio::fs::create_dir_all(&thread_dir)
            .await
            .expect("create legacy rollout dir");
        let lines = [
            json!({
                "type": "thread_meta",
                "payload": {
                    "thread_id": thread_id,
                    "workspace_root": workspace_root,
                    "initial_cwd": workspace_root,
                    "thread_source": "user",
                    "created_at": "2026-06-06T00:00:00Z",
                }
            }),
            json!({
                "type": "turn_context",
                "payload": {
                    "workspace_root": workspace_root,
                    "cwd": workspace_root,
                    "model": {"provider_id": "deepseek", "model_id": "deepseek-v4-pro"},
                    "policy_mode": "off",
                    "permission_profile": "full_access",
                    "command_timeout_secs": 30,
                    "max_output_bytes": 8192,
                    "current_utc_date": "2026-06-05",
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "role": "user",
                    "content": user_message,
                }
            }),
        ]
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("serialize legacy rollout line"))
        .collect::<Vec<_>>()
        .join("\n");
        tokio::fs::write(thread_dir.join("rollout.jsonl"), format!("{lines}\n"))
            .await
            .expect("write legacy rollout");
    }

    async fn insert_stale_thread_row(db: &IndexDb, project_id: &str, thread_id: &str) {
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id)
        .bind(project_id)
        .bind(format!("/tmp/{thread_id}/rollout.jsonl"))
        .bind(format!("{thread_id} stale title"))
        .bind(format!("{thread_id} stale preview"))
        .execute(db.pool())
        .await
        .expect("insert stale thread row");
    }

    #[tokio::test]
    async fn reindex_project_omits_subagent_threads_from_desktop_index() {
        let (dir, db) = temp_db().await;
        let workspace_root = dir.path().join("project");
        tokio::fs::create_dir_all(&workspace_root)
            .await
            .expect("create project root");
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Project".to_string(),
                path: workspace_root.clone(),
            })
            .await
            .expect("upsert project");

        write_rollout(
            &workspace_root,
            "thread_user",
            ThreadSource::User,
            "Root user prompt",
        );
        write_rollout(
            &workspace_root,
            "thread_child",
            ThreadSource::Subagent,
            "Child agent prompt",
        );
        insert_stale_thread_row(&db, &project.id, "thread_child").await;

        let report = reindex_project(&db, &project.id, &workspace_root)
            .await
            .expect("reindex project");
        let threads = db
            .list_threads(ThreadListFilter {
                project_id: project.id,
                include_archived: true,
                search: None,
            })
            .await
            .expect("list threads");

        assert_eq!(report.scanned_threads, 2);
        assert_eq!(report.indexed_threads, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id.as_str(), "thread_user");
        assert_eq!(threads[0].preview, "Root user prompt");
    }

    #[tokio::test]
    async fn reindex_project_indexes_legacy_turn_context_without_turn_id() {
        let (dir, db) = temp_db().await;
        let workspace_root = dir.path().join("project");
        tokio::fs::create_dir_all(&workspace_root)
            .await
            .expect("create project root");
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Project".to_string(),
                path: workspace_root.clone(),
            })
            .await
            .expect("upsert project");

        write_legacy_rollout_without_turn_context_turn_id(
            &workspace_root,
            "thread_legacy_context",
            "Legacy prompt without explicit turn id",
        )
        .await;

        let report = reindex_project(&db, &project.id, &workspace_root)
            .await
            .expect("reindex project");
        let threads = db
            .list_threads(ThreadListFilter {
                project_id: project.id,
                include_archived: true,
                search: None,
            })
            .await
            .expect("list threads");

        assert_eq!(report.scanned_threads, 1);
        assert_eq!(report.indexed_threads, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id.as_str(), "thread_legacy_context");
        assert_eq!(threads[0].preview, "Legacy prompt without explicit turn id");
    }
}
