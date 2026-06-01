use sqlx::{Executor, SqlitePool};

pub const SCHEMA_VERSION: i64 = 1;

pub async fn migrate(pool: &SqlitePool) -> sqlx::Result<()> {
    pool.execute("PRAGMA foreign_keys = ON").await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_opened_at INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY NOT NULL,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  rollout_path TEXT NOT NULL,
  user_title TEXT,
  fallback_title TEXT NOT NULL,
  preview TEXT NOT NULL,
  title_source TEXT NOT NULL,
  archived_at INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_opened_at INTEGER,
  UNIQUE(project_id, id)
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS thread_changed_files (
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  last_seen_at INTEGER NOT NULL,
  PRIMARY KEY(thread_id, path)
)
        "#,
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_threads_project_visible ON threads(project_id, archived_at, pinned, updated_at)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_threads_search ON threads(project_id, user_title, fallback_title, preview)",
    )
    .await?;
    pool.execute(&*format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .await?;
    Ok(())
}
