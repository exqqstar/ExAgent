use sqlx::Row;

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

    pub(crate) async fn set_intensive(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        intensive: bool,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO forge_goal_modes (
  thread_id,
  goal_id,
  intensive,
  updated_at_ms
) VALUES (?, ?, ?, ?)
ON CONFLICT(thread_id, goal_id) DO UPDATE SET
  intensive = excluded.intensive,
  updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(thread_id.as_str())
        .bind(goal_id)
        .bind(if intensive { 1_i64 } else { 0_i64 })
        .bind(now_unix_millis())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    pub(crate) async fn replace_for_thread_goal(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        intensive: bool,
    ) -> anyhow::Result<()> {
        self.clear_for_thread(thread_id).await?;
        if intensive {
            self.set_intensive(thread_id, goal_id, true).await?;
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

    pub(crate) async fn is_intensive(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
    ) -> anyhow::Result<bool> {
        let row = sqlx::query(
            r#"
SELECT intensive
FROM forge_goal_modes
WHERE thread_id = ? AND goal_id = ?
            "#,
        )
        .bind(thread_id.as_str())
        .bind(goal_id)
        .fetch_optional(self.db.pool())
        .await?;
        Ok(row
            .as_ref()
            .map(|row| row.try_get::<i64, _>("intensive").map(|value| value != 0))
            .transpose()?
            .unwrap_or(false))
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
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadGoalStatusRecord};

    #[tokio::test]
    async fn mode_defaults_false_and_can_replace_current_thread_goal() {
        let (dir, store, thread_id, first_goal_id) = fixture().await;
        let _keep_dir = dir;

        assert!(!store
            .is_intensive(&thread_id, &first_goal_id)
            .await
            .unwrap());

        store
            .set_intensive(&thread_id, &first_goal_id, true)
            .await
            .unwrap();
        assert!(store
            .is_intensive(&thread_id, &first_goal_id)
            .await
            .unwrap());

        store
            .replace_for_thread_goal(&thread_id, "goal_second", true)
            .await
            .unwrap();
        assert!(!store
            .is_intensive(&thread_id, &first_goal_id)
            .await
            .unwrap());
        assert!(store.is_intensive(&thread_id, "goal_second").await.unwrap());

        store.clear_for_thread(&thread_id).await.unwrap();
        assert!(!store.is_intensive(&thread_id, "goal_second").await.unwrap());
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
