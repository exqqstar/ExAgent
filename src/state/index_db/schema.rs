use sqlx::{Executor, SqlitePool};

pub const SCHEMA_VERSION: i64 = 8;

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
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS forge_open_questions (
  question_id TEXT PRIMARY KEY NOT NULL,
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  goal_id TEXT NOT NULL,
  question TEXT NOT NULL,
  blocks_what TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('open', 'resolved')),
  answer TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  question_order INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS forge_goal_modes (
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  goal_id TEXT NOT NULL,
  intensive INTEGER NOT NULL DEFAULT 0,
  mode TEXT NOT NULL DEFAULT 'standard' CHECK(mode IN ('standard', 'reviewed', 'intensive')),
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY(thread_id, goal_id)
)
        "#,
    )
    .await?;
    add_column_if_missing(
        pool,
        "forge_goal_modes",
        "mode",
        r#"
ALTER TABLE forge_goal_modes
ADD COLUMN mode TEXT NOT NULL DEFAULT 'standard'
        "#,
    )
    .await?;
    pool.execute(
        r#"
UPDATE forge_goal_modes
SET mode = CASE
  WHEN intensive != 0 THEN 'intensive'
  WHEN mode NOT IN ('standard', 'reviewed', 'intensive') THEN 'standard'
  ELSE mode
END
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
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_forge_open_questions_goal_status ON forge_open_questions(goal_id, status, question_order)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_forge_open_questions_thread_status ON forge_open_questions(thread_id, status, question_order)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_forge_goal_modes_thread ON forge_goal_modes(thread_id)",
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS memory_observations (
  id TEXT PRIMARY KEY NOT NULL,
  scope TEXT NOT NULL CHECK(scope IN ('global','project','thread')),
  project_id TEXT,
  thread_id TEXT NOT NULL,
  turn_id TEXT,
  event_id TEXT,
  source_tool_call_id TEXT,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  narrative TEXT NOT NULL,
  files_json TEXT NOT NULL,
  code_refs_json TEXT NOT NULL,
  concepts_json TEXT NOT NULL,
  importance INTEGER NOT NULL,
  confidence REAL NOT NULL,
  auto_inject_eligible INTEGER NOT NULL,
  suspicious_injection INTEGER NOT NULL DEFAULT 0,
  privacy_flags_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS memory_entries (
  id TEXT PRIMARY KEY NOT NULL,
  scope TEXT NOT NULL CHECK(scope IN ('global','project','thread')),
  project_id TEXT,
  thread_id TEXT,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  content TEXT NOT NULL,
	  files_json TEXT NOT NULL,
	  code_refs_json TEXT NOT NULL,
	  concepts_json TEXT NOT NULL,
	  source_observation_ids_json TEXT NOT NULL,
	  source_refs_json TEXT NOT NULL DEFAULT '[]',
	  confidence REAL NOT NULL,
  strength INTEGER NOT NULL,
  pinned INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('candidate','active','superseded','rejected','archived','deleted')),
  inactive_reason TEXT,
  supersedes_id TEXT,
  suspicious_injection INTEGER NOT NULL DEFAULT 0,
  privacy_flags_json TEXT NOT NULL DEFAULT '{}',
  created_by TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  last_used_at_ms INTEGER,
  use_count INTEGER NOT NULL
)
	        "#,
	    )
	    .await?;
    add_column_if_missing(
        pool,
        "memory_entries",
        "source_refs_json",
        "ALTER TABLE memory_entries ADD COLUMN source_refs_json TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS memory_projection_cursors (
  thread_id TEXT PRIMARY KEY NOT NULL,
  rollout_path TEXT NOT NULL,
  last_event_index INTEGER NOT NULL,
  last_projected_at_ms INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS memory_audit_events (
  id TEXT PRIMARY KEY NOT NULL,
  memory_id TEXT NOT NULL,
  action TEXT NOT NULL,
  actor TEXT NOT NULL,
  detail_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE VIRTUAL TABLE IF NOT EXISTS memory_entries_fts USING fts5(
  id UNINDEXED, scope UNINDEXED, project_id UNINDEXED, thread_id UNINDEXED,
  title, content, files, concepts, tokenize='unicode61'
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE VIRTUAL TABLE IF NOT EXISTS memory_observations_fts USING fts5(
  id UNINDEXED, scope UNINDEXED, project_id UNINDEXED, thread_id UNINDEXED,
  title, narrative, files, concepts, tokenize='unicode61'
)
        "#,
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_memory_entries_scope_project_status ON memory_entries(scope, project_id, status, updated_at_ms)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_memory_entries_thread ON memory_entries(thread_id, status)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_memory_observations_thread_turn ON memory_observations(thread_id, turn_id)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_memory_observations_project_kind ON memory_observations(project_id, kind, created_at_ms)",
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

        for table in [
            "thread_goals",
            "thread_goal_turns",
            "forge_review_tickets",
            "forge_open_questions",
            "forge_goal_modes",
        ] {
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
