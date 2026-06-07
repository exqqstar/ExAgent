use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::bail;
use sqlx::Row;

use crate::types::{ThreadId, TurnId};

use super::{time, IndexDb};

static GOAL_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGoalRecord {
    pub thread_id: ThreadId,
    pub goal_id: String,
    pub objective: String,
    pub status: ThreadGoalStatusRecord,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub continuation_suppressed: bool,
    pub continuation_suppressed_after_turn_id: Option<TurnId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadGoalStatusRecord {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalUpdate {
    pub objective: Option<String>,
    pub status: Option<ThreadGoalStatusRecord>,
    pub token_budget: Option<Option<i64>>,
    pub expected_goal_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalAccountingMode {
    ActiveOnly,
    AnyCurrentGoal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalAccountingOutcome {
    Updated(ThreadGoalRecord),
    Unchanged(Option<ThreadGoalRecord>),
}

impl ThreadGoalStatusRecord {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
        }
    }

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "usage_limited" => Ok(Self::UsageLimited),
            "budget_limited" => Ok(Self::BudgetLimited),
            "complete" => Ok(Self::Complete),
            _ => bail!("unknown thread goal status: {value}"),
        }
    }
}

impl IndexDb {
    pub async fn get_thread_goal(
        &self,
        thread_id: &ThreadId,
    ) -> anyhow::Result<Option<ThreadGoalRecord>> {
        let row = sqlx::query(
            r#"
SELECT
  thread_id,
  goal_id,
  objective,
  status,
  token_budget,
  tokens_used,
  time_used_seconds,
  continuation_suppressed,
  continuation_suppressed_after_turn_id,
  created_at_ms,
  updated_at_ms
FROM thread_goals
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.as_str())
        .fetch_optional(self.pool())
        .await?;
        row.map(|row| thread_goal_record_from_row(&row)).transpose()
    }

    pub async fn replace_thread_goal(
        &self,
        thread_id: &ThreadId,
        objective: &str,
        status: ThreadGoalStatusRecord,
        token_budget: Option<i64>,
    ) -> anyhow::Result<ThreadGoalRecord> {
        let now = time::now_unix_millis();
        let goal_id = new_goal_id(now);
        sqlx::query(
            r#"
INSERT INTO thread_goals (
  thread_id,
  goal_id,
  objective,
  status,
  token_budget,
  tokens_used,
  time_used_seconds,
  continuation_suppressed,
  continuation_suppressed_after_turn_id,
  continuation_suppressed_at_ms,
  created_at_ms,
  updated_at_ms
)
VALUES (?, ?, ?, ?, ?, 0, 0, 0, NULL, NULL, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
  goal_id = excluded.goal_id,
  objective = excluded.objective,
  status = excluded.status,
  token_budget = excluded.token_budget,
  tokens_used = 0,
  time_used_seconds = 0,
  continuation_suppressed = 0,
  continuation_suppressed_after_turn_id = NULL,
  continuation_suppressed_at_ms = NULL,
  created_at_ms = excluded.created_at_ms,
  updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(thread_id.as_str())
        .bind(&goal_id)
        .bind(objective)
        .bind(status.as_str())
        .bind(token_budget)
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(self
            .get_thread_goal(thread_id)
            .await?
            .expect("replace_thread_goal should create a goal"))
    }

    pub async fn insert_thread_goal(
        &self,
        thread_id: &ThreadId,
        objective: &str,
        token_budget: Option<i64>,
    ) -> anyhow::Result<Option<ThreadGoalRecord>> {
        let now = time::now_unix_millis();
        let goal_id = new_goal_id(now);
        let result = sqlx::query(
            r#"
INSERT OR IGNORE INTO thread_goals (
  thread_id,
  goal_id,
  objective,
  status,
  token_budget,
  tokens_used,
  time_used_seconds,
  continuation_suppressed,
  continuation_suppressed_after_turn_id,
  continuation_suppressed_at_ms,
  created_at_ms,
  updated_at_ms
)
VALUES (?, ?, ?, 'active', ?, 0, 0, 0, NULL, NULL, ?, ?)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(goal_id)
        .bind(objective)
        .bind(token_budget)
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_thread_goal(thread_id).await
    }

    pub async fn update_thread_goal(
        &self,
        thread_id: &ThreadId,
        update: GoalUpdate,
    ) -> anyhow::Result<Option<ThreadGoalRecord>> {
        let Some(current) = self.get_thread_goal(thread_id).await? else {
            return Ok(None);
        };
        if update
            .expected_goal_id
            .as_deref()
            .is_some_and(|expected| expected != current.goal_id)
        {
            return Ok(None);
        }

        let objective = update.objective.unwrap_or(current.objective);
        let status = update.status.unwrap_or(current.status);
        let token_budget = update.token_budget.unwrap_or(current.token_budget);
        let now = time::now_unix_millis();
        sqlx::query(
            r#"
UPDATE thread_goals
SET objective = ?,
    status = ?,
    token_budget = ?,
    updated_at_ms = ?
WHERE thread_id = ?
            "#,
        )
        .bind(objective)
        .bind(status.as_str())
        .bind(token_budget)
        .bind(now)
        .bind(thread_id.as_str())
        .execute(self.pool())
        .await?;
        self.get_thread_goal(thread_id).await
    }

    pub async fn delete_thread_goal(&self, thread_id: &ThreadId) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM thread_goals WHERE thread_id = ?")
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn account_thread_goal_usage(
        &self,
        thread_id: &ThreadId,
        time_delta_seconds: i64,
        token_delta: i64,
        mode: GoalAccountingMode,
        expected_goal_id: Option<&str>,
    ) -> anyhow::Result<GoalAccountingOutcome> {
        let Some(current) = self.get_thread_goal(thread_id).await? else {
            return Ok(GoalAccountingOutcome::Unchanged(None));
        };
        if expected_goal_id.is_some_and(|expected| expected != current.goal_id) {
            return Ok(GoalAccountingOutcome::Unchanged(Some(current)));
        }
        if mode == GoalAccountingMode::ActiveOnly
            && current.status != ThreadGoalStatusRecord::Active
        {
            return Ok(GoalAccountingOutcome::Unchanged(Some(current)));
        }

        let token_delta = token_delta.max(0);
        let time_delta_seconds = time_delta_seconds.max(0);
        let tokens_used = current.tokens_used.saturating_add(token_delta);
        let time_used_seconds = current.time_used_seconds.saturating_add(time_delta_seconds);
        let status = if current
            .token_budget
            .is_some_and(|budget| tokens_used >= budget)
        {
            ThreadGoalStatusRecord::BudgetLimited
        } else {
            current.status
        };
        let now = time::now_unix_millis();
        sqlx::query(
            r#"
UPDATE thread_goals
SET tokens_used = ?,
    time_used_seconds = ?,
    status = ?,
    updated_at_ms = ?
WHERE thread_id = ? AND goal_id = ?
            "#,
        )
        .bind(tokens_used)
        .bind(time_used_seconds)
        .bind(status.as_str())
        .bind(now)
        .bind(thread_id.as_str())
        .bind(&current.goal_id)
        .execute(self.pool())
        .await?;
        Ok(GoalAccountingOutcome::Updated(
            self.get_thread_goal(thread_id)
                .await?
                .expect("accounted goal should still exist"),
        ))
    }

    pub async fn suppress_thread_goal_continuation(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        after_turn_id: &TurnId,
    ) -> anyhow::Result<Option<ThreadGoalRecord>> {
        let now = time::now_unix_millis();
        let result = sqlx::query(
            r#"
UPDATE thread_goals
SET continuation_suppressed = 1,
    continuation_suppressed_after_turn_id = ?,
    continuation_suppressed_at_ms = ?,
    updated_at_ms = ?
WHERE thread_id = ? AND goal_id = ? AND status = 'active'
            "#,
        )
        .bind(after_turn_id.as_str())
        .bind(now)
        .bind(now)
        .bind(thread_id.as_str())
        .bind(goal_id)
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_thread_goal(thread_id).await
    }

    pub async fn reset_thread_goal_continuation_suppression(
        &self,
        thread_id: &ThreadId,
        expected_goal_id: Option<&str>,
    ) -> anyhow::Result<Option<ThreadGoalRecord>> {
        let Some(current) = self.get_thread_goal(thread_id).await? else {
            return Ok(None);
        };
        if expected_goal_id.is_some_and(|expected| expected != current.goal_id) {
            return Ok(None);
        }

        let now = time::now_unix_millis();
        sqlx::query(
            r#"
UPDATE thread_goals
SET continuation_suppressed = 0,
    continuation_suppressed_after_turn_id = NULL,
    continuation_suppressed_at_ms = NULL,
    updated_at_ms = ?
WHERE thread_id = ? AND goal_id = ?
            "#,
        )
        .bind(now)
        .bind(thread_id.as_str())
        .bind(&current.goal_id)
        .execute(self.pool())
        .await?;
        self.get_thread_goal(thread_id).await
    }
}

fn new_goal_id(now_ms: i64) -> String {
    let counter = GOAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("goal_{now_ms}_{counter}")
}

fn thread_goal_record_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<ThreadGoalRecord> {
    Ok(ThreadGoalRecord {
        thread_id: ThreadId::new(row.try_get::<String, _>("thread_id")?),
        goal_id: row.try_get("goal_id")?,
        objective: row.try_get("objective")?,
        status: ThreadGoalStatusRecord::from_str(row.try_get::<String, _>("status")?.as_str())?,
        token_budget: row.try_get("token_budget")?,
        tokens_used: row.try_get("tokens_used")?,
        time_used_seconds: row.try_get("time_used_seconds")?,
        continuation_suppressed: row.try_get::<i64, _>("continuation_suppressed")? != 0,
        continuation_suppressed_after_turn_id: row
            .try_get::<Option<String>, _>("continuation_suppressed_after_turn_id")?
            .map(TurnId::new),
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::index_db::{IndexDb, ProjectRecord, ProjectUpsert};
    use crate::types::{ThreadId, TurnId};

    use super::{GoalAccountingMode, GoalAccountingOutcome, GoalUpdate, ThreadGoalStatusRecord};

    async fn temp_db() -> (tempfile::TempDir, IndexDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        (dir, db)
    }

    async fn add_project(db: &IndexDb, root: &Path, name: &str) -> ProjectRecord {
        tokio::fs::create_dir_all(root).await.unwrap();
        db.upsert_project(ProjectUpsert {
            name: name.into(),
            path: root.into(),
        })
        .await
        .unwrap()
    }

    async fn insert_thread(db: &IndexDb, project_id: &str, thread_id: &ThreadId) {
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project_id)
        .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
        .bind(format!("{} title", thread_id.as_str()))
        .bind(format!("{} preview", thread_id.as_str()))
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn db_with_thread(thread_id: &ThreadId) -> (tempfile::TempDir, IndexDb) {
        let (dir, db) = temp_db().await;
        let project = add_project(&db, &dir.path().join("alpha"), "Alpha").await;
        insert_thread(&db, &project.id, thread_id).await;
        (dir, db)
    }

    #[tokio::test]
    async fn creates_active_goal() {
        let thread_id = ThreadId::new("thread_goal_create");
        let (_dir, db) = db_with_thread(&thread_id).await;

        let goal = db
            .insert_thread_goal(&thread_id, "ship goal store", Some(100))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(goal.thread_id, thread_id);
        assert_eq!(goal.objective, "ship goal store");
        assert_eq!(goal.status, ThreadGoalStatusRecord::Active);
        assert_eq!(goal.token_budget, Some(100));
        assert_eq!(goal.tokens_used, 0);
        assert_eq!(goal.time_used_seconds, 0);
        assert!(!goal.continuation_suppressed);
        assert!(goal.continuation_suppressed_after_turn_id.is_none());

        assert_eq!(
            db.get_thread_goal(&goal.thread_id).await.unwrap(),
            Some(goal)
        );
    }

    #[tokio::test]
    async fn rejects_duplicate_insert() {
        let thread_id = ThreadId::new("thread_goal_duplicate");
        let (_dir, db) = db_with_thread(&thread_id).await;

        let first = db
            .insert_thread_goal(&thread_id, "first goal", None)
            .await
            .unwrap();
        let duplicate = db
            .insert_thread_goal(&thread_id, "second goal", None)
            .await
            .unwrap();

        assert!(first.is_some());
        assert!(duplicate.is_none());
        assert_eq!(
            db.get_thread_goal(&thread_id)
                .await
                .unwrap()
                .unwrap()
                .objective,
            "first goal"
        );
    }

    #[tokio::test]
    async fn replaces_complete_goal_with_new_goal_id() {
        let thread_id = ThreadId::new("thread_goal_replace");
        let (_dir, db) = db_with_thread(&thread_id).await;
        let original = db
            .replace_thread_goal(
                &thread_id,
                "old goal",
                ThreadGoalStatusRecord::Complete,
                Some(10),
            )
            .await
            .unwrap();
        db.account_thread_goal_usage(&thread_id, 5, 9, GoalAccountingMode::AnyCurrentGoal, None)
            .await
            .unwrap();

        let replacement = db
            .replace_thread_goal(&thread_id, "new goal", ThreadGoalStatusRecord::Active, None)
            .await
            .unwrap();

        assert_ne!(replacement.goal_id, original.goal_id);
        assert_eq!(replacement.objective, "new goal");
        assert_eq!(replacement.status, ThreadGoalStatusRecord::Active);
        assert_eq!(replacement.tokens_used, 0);
        assert_eq!(replacement.time_used_seconds, 0);
        assert_eq!(replacement.token_budget, None);
    }

    #[tokio::test]
    async fn updates_paused_to_active() {
        let thread_id = ThreadId::new("thread_goal_update");
        let (_dir, db) = db_with_thread(&thread_id).await;
        let paused = db
            .replace_thread_goal(
                &thread_id,
                "paused goal",
                ThreadGoalStatusRecord::Paused,
                None,
            )
            .await
            .unwrap();

        let updated = db
            .update_thread_goal(
                &thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(ThreadGoalStatusRecord::Active),
                    token_budget: Some(Some(200)),
                    expected_goal_id: Some(paused.goal_id.clone()),
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(updated.goal_id, paused.goal_id);
        assert_eq!(updated.status, ThreadGoalStatusRecord::Active);
        assert_eq!(updated.token_budget, Some(200));
    }

    #[tokio::test]
    async fn clears_goal() {
        let thread_id = ThreadId::new("thread_goal_clear");
        let (_dir, db) = db_with_thread(&thread_id).await;
        db.insert_thread_goal(&thread_id, "temporary goal", None)
            .await
            .unwrap()
            .unwrap();

        assert!(db.delete_thread_goal(&thread_id).await.unwrap());
        assert!(db.get_thread_goal(&thread_id).await.unwrap().is_none());
        assert!(!db.delete_thread_goal(&thread_id).await.unwrap());
    }

    #[tokio::test]
    async fn accounts_usage_into_budget_limited() {
        let thread_id = ThreadId::new("thread_goal_budget");
        let (_dir, db) = db_with_thread(&thread_id).await;
        let goal = db
            .insert_thread_goal(&thread_id, "budgeted goal", Some(10))
            .await
            .unwrap()
            .unwrap();

        let outcome = db
            .account_thread_goal_usage(
                &thread_id,
                -5,
                12,
                GoalAccountingMode::ActiveOnly,
                Some(&goal.goal_id),
            )
            .await
            .unwrap();

        let GoalAccountingOutcome::Updated(updated) = outcome else {
            panic!("expected usage update");
        };
        assert_eq!(updated.tokens_used, 12);
        assert_eq!(updated.time_used_seconds, 0);
        assert_eq!(updated.status, ThreadGoalStatusRecord::BudgetLimited);
    }

    #[tokio::test]
    async fn suppresses_after_empty_goal_continuation_turn() {
        let thread_id = ThreadId::new("thread_goal_suppress");
        let after_turn_id = TurnId::new("turn_empty_goal_continuation");
        let (_dir, db) = db_with_thread(&thread_id).await;
        let goal = db
            .insert_thread_goal(&thread_id, "continue until done", None)
            .await
            .unwrap()
            .unwrap();

        let suppressed = db
            .suppress_thread_goal_continuation(&thread_id, &goal.goal_id, &after_turn_id)
            .await
            .unwrap()
            .unwrap();

        assert!(suppressed.continuation_suppressed);
        assert_eq!(
            suppressed.continuation_suppressed_after_turn_id,
            Some(after_turn_id)
        );
    }

    #[tokio::test]
    async fn resets_suppression_after_external_resume_or_edit() {
        let thread_id = ThreadId::new("thread_goal_reset_suppression");
        let after_turn_id = TurnId::new("turn_empty_goal_continuation");
        let (_dir, db) = db_with_thread(&thread_id).await;
        let goal = db
            .insert_thread_goal(&thread_id, "resume after activity", None)
            .await
            .unwrap()
            .unwrap();
        db.suppress_thread_goal_continuation(&thread_id, &goal.goal_id, &after_turn_id)
            .await
            .unwrap();

        assert!(db
            .reset_thread_goal_continuation_suppression(&thread_id, Some("wrong_goal"))
            .await
            .unwrap()
            .is_none());

        let reset = db
            .reset_thread_goal_continuation_suppression(&thread_id, Some(&goal.goal_id))
            .await
            .unwrap()
            .unwrap();
        assert!(!reset.continuation_suppressed);
        assert!(reset.continuation_suppressed_after_turn_id.is_none());

        let reset_again = db
            .reset_thread_goal_continuation_suppression(&thread_id, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!reset_again.continuation_suppressed);
    }
}
