use sqlx::{Executor, SqlitePool};

pub const SCHEMA_VERSION: i64 = 4;

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
    add_column_if_missing(
        pool,
        "projects",
        "archived_at",
        "ALTER TABLE projects ADD COLUMN archived_at INTEGER",
    )
    .await?;
    add_column_if_missing(
        pool,
        "projects",
        "pinned",
        "ALTER TABLE projects ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
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
        r#"
CREATE TABLE IF NOT EXISTS thread_goals (
  thread_id TEXT PRIMARY KEY NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  goal_id TEXT NOT NULL,
  objective TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN (
    'active',
    'paused',
    'blocked',
    'usage_limited',
    'budget_limited',
    'complete'
  )),
  token_budget INTEGER,
  tokens_used INTEGER NOT NULL DEFAULT 0,
  time_used_seconds INTEGER NOT NULL DEFAULT 0,
  continuation_suppressed INTEGER NOT NULL DEFAULT 0,
  continuation_suppressed_after_turn_id TEXT,
  continuation_suppressed_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS thread_goal_turns (
  goal_id TEXT NOT NULL,
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  turn_id TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  finished_at_ms INTEGER,
  token_delta INTEGER NOT NULL DEFAULT 0,
  time_delta_seconds INTEGER NOT NULL DEFAULT 0,
  counted_autonomous_activity INTEGER NOT NULL DEFAULT 0,
  trigger TEXT NOT NULL CHECK(trigger IN ('user', 'goal_continuation', 'external_resume')),
  terminal_status TEXT,
  PRIMARY KEY(goal_id, turn_id)
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS forge_review_tickets (
  ticket_id TEXT PRIMARY KEY NOT NULL,
  goal_id TEXT NOT NULL,
  baseline_hash TEXT,
  status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected')),
  reviewed_hash TEXT,
  findings TEXT,
  reject_category TEXT CHECK(reject_category IS NULL OR reject_category IN (
    'retriable_gap',
    'needs_user',
    'external_blocker'
  )),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  ticket_order INTEGER NOT NULL
)
        "#,
    )
    .await?;
    add_column_if_missing(
        pool,
        "forge_review_tickets",
        "reject_category",
        r#"
ALTER TABLE forge_review_tickets
ADD COLUMN reject_category TEXT CHECK(reject_category IS NULL OR reject_category IN (
  'retriable_gap',
  'needs_user',
  'external_blocker'
))
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
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_thread_goal_turns_thread ON thread_goal_turns(thread_id, started_at_ms)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_forge_review_tickets_goal_order ON forge_review_tickets(goal_id, ticket_order)",
    )
    .await?;
    pool.execute(&*format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .await?;
    Ok(())
}

async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    sql: &str,
) -> sqlx::Result<()> {
    let escaped_table = table.replace('\'', "''");
    let escaped_column = column.replace('\'', "''");
    let exists: Option<(i64,)> = sqlx::query_as(&format!(
        "SELECT 1 FROM pragma_table_info('{escaped_table}') WHERE name = '{escaped_column}'"
    ))
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        pool.execute(sql).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn migrate_creates_thread_goal_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.sqlite");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();

        migrate(&pool).await.unwrap();

        for table in ["thread_goals", "thread_goal_turns", "forge_review_tickets"] {
            let exists: Option<(i64,)> =
                sqlx::query_as("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
                    .bind(table)
                    .fetch_optional(&pool)
                    .await
                    .unwrap();
            assert!(exists.is_some(), "{table} table should exist");
        }
    }
}
