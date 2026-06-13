use sqlx::Row;

use crate::app_server::protocol::ThreadGoalMode;
use crate::index_db::IndexDb;
use crate::types::ThreadId;

#[derive(Clone)]
pub(crate) struct ForgeGoalModeStore {
    db: IndexDb,
}

impl ForgeGoalModeStore {
    pub(crate) fn new(db: IndexDb) -> Self {
        Self { db }
    }

    pub(crate) async fn set_mode(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        mode: ThreadGoalMode,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO forge_goal_modes (
  thread_id,
  goal_id,
  intensive,
  mode,
  updated_at_ms
) VALUES (?, ?, ?, ?, ?)
ON CONFLICT(thread_id, goal_id) DO UPDATE SET
  intensive = excluded.intensive,
  mode = excluded.mode,
  updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(thread_id.as_str())
        .bind(goal_id)
        .bind(if mode.is_intensive() { 1_i64 } else { 0_i64 })
        .bind(mode.as_str())
        .bind(now_unix_millis())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn set_intensive(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        intensive: bool,
    ) -> anyhow::Result<()> {
        self.set_mode(thread_id, goal_id, ThreadGoalMode::from(intensive))
            .await
    }

    pub(crate) async fn replace_for_thread_goal(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        mode: impl Into<ThreadGoalMode>,
    ) -> anyhow::Result<()> {
        let mode = mode.into();
        self.clear_for_thread(thread_id).await?;
        if mode != ThreadGoalMode::Standard {
            self.set_mode(thread_id, goal_id, mode).await?;
        }
        Ok(())
    }

    pub(crate) async fn clear_for_thread(&self, thread_id: &ThreadId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM forge_goal_modes WHERE thread_id = ?")
            .bind(thread_id.as_str())
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn mode_for_goal(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
    ) -> anyhow::Result<ThreadGoalMode> {
        let row = sqlx::query(
            r#"
SELECT mode, intensive
FROM forge_goal_modes
WHERE thread_id = ? AND goal_id = ?
            "#,
        )
        .bind(thread_id.as_str())
        .bind(goal_id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(ThreadGoalMode::Standard);
        };
        let intensive = row.try_get::<i64, _>("intensive")? != 0;
        let mode = row.try_get::<String, _>("mode").ok();
        match mode.as_deref() {
            Some("reviewed") => Ok(ThreadGoalMode::Reviewed),
            Some("intensive") => Ok(ThreadGoalMode::Intensive),
            Some("standard") if intensive => Ok(ThreadGoalMode::Intensive),
            Some("standard") => Ok(ThreadGoalMode::Standard),
            _ => Ok(if intensive {
                ThreadGoalMode::Intensive
            } else {
                ThreadGoalMode::Standard
            }),
        }
    }

    pub(crate) async fn is_intensive(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
    ) -> anyhow::Result<bool> {
        Ok(self.mode_for_goal(thread_id, goal_id).await?.is_intensive())
    }
}

fn now_unix_millis() -> i64 {
    let now = ::time::OffsetDateTime::now_utc();
    now.unix_timestamp()
        .saturating_mul(1_000)
        .saturating_add(i64::from(now.millisecond()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::ThreadGoalMode;
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadGoalStatusRecord};

    #[tokio::test]
    async fn mode_defaults_standard_and_can_replace_current_thread_goal() {
        let (dir, store, thread_id, first_goal_id) = fixture().await;
        let _keep_dir = dir;

        assert_eq!(
            store
                .mode_for_goal(&thread_id, &first_goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Standard
        );

        store
            .set_mode(&thread_id, &first_goal_id, ThreadGoalMode::Reviewed)
            .await
            .unwrap();
        assert_eq!(
            store
                .mode_for_goal(&thread_id, &first_goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Reviewed
        );

        store
            .replace_for_thread_goal(&thread_id, "goal_second", ThreadGoalMode::Intensive)
            .await
            .unwrap();
        assert_eq!(
            store
                .mode_for_goal(&thread_id, &first_goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Standard
        );
        assert_eq!(
            store
                .mode_for_goal(&thread_id, "goal_second")
                .await
                .unwrap(),
            ThreadGoalMode::Intensive
        );

        store.clear_for_thread(&thread_id).await.unwrap();
        assert_eq!(
            store
                .mode_for_goal(&thread_id, "goal_second")
                .await
                .unwrap(),
            ThreadGoalMode::Standard
        );
    }

    #[tokio::test]
    async fn legacy_intensive_boolean_rows_map_to_mode() {
        let (dir, store, thread_id, first_goal_id) = fixture().await;
        let _keep_dir = dir;

        sqlx::query(
            r#"
INSERT INTO forge_goal_modes (
  thread_id,
  goal_id,
  intensive,
  updated_at_ms
) VALUES (?, ?, 1, 123)
ON CONFLICT(thread_id, goal_id) DO UPDATE SET
  intensive = excluded.intensive,
  updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(thread_id.as_str())
        .bind(&first_goal_id)
        .execute(store.db.pool())
        .await
        .unwrap();

        assert_eq!(
            store
                .mode_for_goal(&thread_id, &first_goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Intensive
        );
    }

    async fn fixture() -> (tempfile::TempDir, ForgeGoalModeStore, ThreadId, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let workspace = dir.path().join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Goal Modes".into(),
                path: workspace,
            })
            .await
            .unwrap();
        let thread_id = ThreadId::new("thread_goal_modes");
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
        .bind(project.id)
        .bind("rollout.jsonl")
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
        db.replace_thread_goal(
            &thread_id,
            "ship intensive mode",
            ThreadGoalStatusRecord::Active,
            None,
        )
        .await
        .unwrap();
        let goal_id = db
            .get_thread_goal(&thread_id)
            .await
            .unwrap()
            .unwrap()
            .goal_id;
        (dir, ForgeGoalModeStore::new(db), thread_id, goal_id)
    }
}
