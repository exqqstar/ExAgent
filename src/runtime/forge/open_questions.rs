#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::bail;
use sqlx::Row;

use crate::index_db::IndexDb;
use crate::types::ThreadId;

static OPEN_QUESTION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenQuestion {
    pub(crate) question_id: String,
    pub(crate) thread_id: ThreadId,
    pub(crate) goal_id: String,
    pub(crate) question: String,
    pub(crate) blocks_what: String,
    pub(crate) status: OpenQuestionStatus,
    pub(crate) answer: Option<String>,
    pub(crate) created_at_ms: i64,
    pub(crate) updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenQuestionStatus {
    Open,
    Resolved,
}

#[derive(Clone)]
pub(crate) struct OpenQuestionStore {
    db: IndexDb,
}

impl OpenQuestionStore {
    pub(crate) fn new(db: IndexDb) -> Self {
        Self { db }
    }

    pub(crate) fn db(&self) -> IndexDb {
        self.db.clone()
    }

    pub(crate) async fn record_question(
        &self,
        thread_id: ThreadId,
        goal_id: impl Into<String>,
        question: impl Into<String>,
        blocks_what: impl Into<String>,
    ) -> anyhow::Result<OpenQuestion> {
        let goal_id = goal_id.into();
        let question = question.into();
        let blocks_what = blocks_what.into();
        let now = now_unix_millis();
        let question_order = OPEN_QUESTION_COUNTER.fetch_add(1, Ordering::Relaxed) as i64;
        let question_id = format!("oq_{now}_{question_order}");
        sqlx::query(
            r#"
INSERT INTO forge_open_questions (
  question_id,
  thread_id,
  goal_id,
  question,
  blocks_what,
  status,
  answer,
  created_at_ms,
  updated_at_ms,
  question_order
) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)
            "#,
        )
        .bind(&question_id)
        .bind(thread_id.as_str())
        .bind(&goal_id)
        .bind(&question)
        .bind(&blocks_what)
        .bind(OpenQuestionStatus::Open.as_str())
        .bind(now)
        .bind(now)
        .bind(question_order)
        .execute(self.db.pool())
        .await?;
        Ok(OpenQuestion {
            question_id,
            thread_id,
            goal_id,
            question,
            blocks_what,
            status: OpenQuestionStatus::Open,
            answer: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    pub(crate) async fn unresolved_for_goal(
        &self,
        goal_id: &str,
    ) -> anyhow::Result<Vec<OpenQuestion>> {
        let rows = sqlx::query(
            r#"
SELECT
  question_id,
  thread_id,
  goal_id,
  question,
  blocks_what,
  status,
  answer,
  created_at_ms,
  updated_at_ms
FROM forge_open_questions
WHERE goal_id = ? AND status = 'open'
ORDER BY question_order ASC
            "#,
        )
        .bind(goal_id)
        .fetch_all(self.db.pool())
        .await?;
        rows.iter().map(open_question_from_row).collect()
    }

    pub(crate) async fn resolve_question(
        &self,
        question_id: &str,
        answer: Option<String>,
    ) -> anyhow::Result<OpenQuestion> {
        let now = now_unix_millis();
        let result = sqlx::query(
            r#"
UPDATE forge_open_questions
SET status = 'resolved',
    answer = ?,
    updated_at_ms = ?
WHERE question_id = ?
            "#,
        )
        .bind(&answer)
        .bind(now)
        .bind(question_id)
        .execute(self.db.pool())
        .await?;
        if result.rows_affected() == 0 {
            bail!("unknown open question: {question_id}");
        }
        Ok(self
            .get_question(question_id)
            .await?
            .expect("resolved question should exist"))
    }

    pub(crate) async fn get_question(
        &self,
        question_id: &str,
    ) -> anyhow::Result<Option<OpenQuestion>> {
        let row = sqlx::query(
            r#"
SELECT
  question_id,
  thread_id,
  goal_id,
  question,
  blocks_what,
  status,
  answer,
  created_at_ms,
  updated_at_ms
FROM forge_open_questions
WHERE question_id = ?
            "#,
        )
        .bind(question_id)
        .fetch_optional(self.db.pool())
        .await?;
        row.as_ref().map(open_question_from_row).transpose()
    }
}

impl OpenQuestionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            _ => bail!("unknown open question status: {value}"),
        }
    }
}

fn open_question_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<OpenQuestion> {
    Ok(OpenQuestion {
        question_id: row.try_get("question_id")?,
        thread_id: ThreadId::new(row.try_get::<String, _>("thread_id")?),
        goal_id: row.try_get("goal_id")?,
        question: row.try_get("question")?,
        blocks_what: row.try_get("blocks_what")?,
        status: OpenQuestionStatus::from_str(row.try_get::<String, _>("status")?.as_str())?,
        answer: row.try_get("answer")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
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
    use crate::types::ThreadId;

    async fn store() -> (tempfile::TempDir, OpenQuestionStore, ThreadId, String) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Open Questions".into(),
                path: workspace.clone(),
            })
            .await
            .unwrap();
        let thread_id = ThreadId::new("thread_open_questions");
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
        .bind(workspace.join("rollout.jsonl").display().to_string())
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
        db.replace_thread_goal(
            &thread_id,
            "ship with deferred input",
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
        (dir, OpenQuestionStore::new(db), thread_id, goal_id)
    }

    #[tokio::test]
    async fn record_and_resolve_open_question_for_goal() {
        let (_dir, store, thread_id, goal_id) = store().await;

        let question = store
            .record_question(
                thread_id.clone(),
                goal_id.clone(),
                "Which rollout cohort should get this first?",
                "Release targeting",
            )
            .await
            .unwrap();

        assert!(question.question_id.starts_with("oq_"));
        assert_eq!(question.thread_id, thread_id);
        assert_eq!(question.goal_id, goal_id);
        assert_eq!(question.status, OpenQuestionStatus::Open);
        assert_eq!(
            store
                .unresolved_for_goal(&question.goal_id)
                .await
                .unwrap()
                .len(),
            1
        );

        store
            .resolve_question(&question.question_id, Some("Use beta users".to_string()))
            .await
            .unwrap();

        assert!(store
            .unresolved_for_goal(&question.goal_id)
            .await
            .unwrap()
            .is_empty());
    }
}
