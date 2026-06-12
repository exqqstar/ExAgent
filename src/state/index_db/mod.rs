mod goal_store;
mod reindex;
mod schema;
mod store;
mod time;

use std::path::{Path, PathBuf};

use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

pub use goal_store::{
    GoalAccountingMode, GoalAccountingOutcome, GoalUpdate, ThreadGoalRecord, ThreadGoalStatusRecord,
};
pub use store::{ProjectUpsert, ThreadListFilter};

#[derive(Clone)]
pub struct IndexDb {
    pool: SqlitePool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub archived_at: Option<i64>,
    pub pinned: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: crate::types::ThreadId,
    pub project_id: String,
    pub rollout_path: PathBuf,
    pub user_title: Option<String>,
    pub fallback_title: String,
    pub preview: String,
    pub title_source: String,
    pub archived_at: Option<i64>,
    pub pinned: bool,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_opened_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_parent_thread_id: Option<crate::types::ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_point_turn_id: Option<crate::types::TurnId>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ReindexReport {
    pub scanned_threads: usize,
    pub indexed_threads: usize,
    pub stale_threads: usize,
}

impl IndexDb {
    pub async fn open(path: impl AsRef<Path>) -> sqlx::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(sqlx::Error::Io)?;
        }

        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        schema::migrate(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn schema_version(&self) -> sqlx::Result<i64> {
        let row = sqlx::query("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await?;
        row.try_get(0)
    }

    pub async fn reindex_project(
        &self,
        project_id: &str,
        project_path: &Path,
    ) -> anyhow::Result<ReindexReport> {
        reindex::reindex_project(self, project_id, project_path).await
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
