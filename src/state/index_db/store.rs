use std::path::{Path, PathBuf};

use sqlx::Row;

use crate::types::ThreadId;

use super::{time, IndexDb, ProjectRecord, ThreadRecord};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectUpsert {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq, Eq)]
pub struct ThreadListFilter {
    pub project_id: String,
    pub include_archived: bool,
    pub search: Option<String>,
}

pub(crate) fn project_id_from_path(path: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.display().to_string().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("project_{hash:016x}")
}

impl IndexDb {
    pub async fn upsert_project(&self, input: ProjectUpsert) -> anyhow::Result<ProjectRecord> {
        let path = tokio::fs::canonicalize(input.path).await?;
        let now = time::now_unix_millis();
        let id = project_id_from_path(&path);
        sqlx::query(
            r#"
INSERT INTO projects (id, name, path, created_at, last_opened_at)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(path) DO UPDATE SET
  name = excluded.name,
  last_opened_at = excluded.last_opened_at
            "#,
        )
        .bind(&id)
        .bind(input.name)
        .bind(path.display().to_string())
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        self.project_by_id(&id).await
    }

    pub async fn project_by_id(&self, project_id: &str) -> anyhow::Result<ProjectRecord> {
        let row = sqlx::query("SELECT id, name, path FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_one(self.pool())
            .await?;
        Ok(ProjectRecord {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            path: PathBuf::from(row.try_get::<String, _>("path")?),
        })
    }

    pub async fn touch_project(&self, project_id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE projects SET last_opened_at = ? WHERE id = ?")
            .bind(time::now_unix_millis())
            .bind(project_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn list_projects(&self) -> anyhow::Result<Vec<ProjectRecord>> {
        let rows = sqlx::query(
            "SELECT id, name, path FROM projects ORDER BY last_opened_at DESC, name ASC",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProjectRecord {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    path: PathBuf::from(row.try_get::<String, _>("path")?),
                })
            })
            .collect()
    }

    pub async fn list_threads(
        &self,
        filter: ThreadListFilter,
    ) -> anyhow::Result<Vec<ThreadRecord>> {
        let mut sql = String::from(
            "SELECT id, project_id, rollout_path, user_title, fallback_title, preview, title_source, archived_at, pinned, status, created_at, updated_at, last_opened_at FROM threads WHERE project_id = ?",
        );
        if !filter.include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        if filter
            .search
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            sql.push_str(" AND (instr(COALESCE(user_title, ''), ?) > 0 OR instr(fallback_title, ?) > 0 OR instr(preview, ?) > 0)");
        }
        sql.push_str(" ORDER BY pinned DESC, updated_at DESC, created_at DESC");

        let mut query = sqlx::query(&sql).bind(filter.project_id);
        if let Some(search) = filter.search.filter(|value| !value.trim().is_empty()) {
            let search = search.trim().to_string();
            query = query.bind(search.clone()).bind(search.clone()).bind(search);
        }

        let rows = query.fetch_all(self.pool()).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ThreadRecord {
                    id: ThreadId::new(row.try_get::<String, _>("id")?),
                    project_id: row.try_get("project_id")?,
                    rollout_path: PathBuf::from(row.try_get::<String, _>("rollout_path")?),
                    user_title: row.try_get("user_title")?,
                    fallback_title: row.try_get("fallback_title")?,
                    preview: row.try_get("preview")?,
                    title_source: row.try_get("title_source")?,
                    archived_at: row.try_get("archived_at")?,
                    pinned: row.try_get::<i64, _>("pinned")? != 0,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    last_opened_at: row.try_get("last_opened_at")?,
                })
            })
            .collect()
    }

    pub async fn rename_thread(
        &self,
        thread_id: &crate::types::ThreadId,
        title: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET user_title = ?, title_source = 'user' WHERE id = ?")
            .bind(title.trim())
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_thread_pinned(
        &self,
        thread_id: &crate::types::ThreadId,
        pinned: bool,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET pinned = ? WHERE id = ?")
            .bind(if pinned { 1_i64 } else { 0_i64 })
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn archive_thread(&self, thread_id: &crate::types::ThreadId) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET archived_at = ? WHERE id = ?")
            .bind(time::now_unix_millis())
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn unarchive_thread(&self, thread_id: &crate::types::ThreadId) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET archived_at = NULL WHERE id = ?")
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }
}
