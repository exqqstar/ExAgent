use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
use crate::runtime::forge::open_questions::{OpenQuestion, OpenQuestionStore};
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::tool_hooks::{ToolHooks, ToolInvocationContext};
use crate::tools::ToolRuntimeEffect;
use crate::types::ToolResult;
use crate::types::ToolStatus;
use crate::workspace_checkpoint::workspace_content_hash;

pub(crate) struct ForgeGateHooks {
    review_store: ReviewStore,
    question_store: OpenQuestionStore,
    mode_store: ForgeGoalModeStore,
}

impl ForgeGateHooks {
    pub(crate) fn new(review_store: ReviewStore, question_store: OpenQuestionStore) -> Self {
        let mode_store = ForgeGoalModeStore::new(review_store.db());
        Self {
            review_store,
            question_store,
            mode_store,
        }
    }
}

#[async_trait]
impl ToolHooks for ForgeGateHooks {
    async fn before_invocation(
        &self,
        _ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn approval_requested(
        &self,
        _ctx: &ToolInvocationContext,
        _approval_id: &crate::session::ApprovalId,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn before_handler_execution(
        &self,
        ctx: &ToolInvocationContext,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        if !is_complete_goal_update(ctx) {
            return Ok(Vec::new());
        }
        let Some(thread_id) = ctx.thread_id.as_ref() else {
            return Ok(Vec::new());
        };
        let Some(goal) = self.review_store.db().get_thread_goal(thread_id).await? else {
            return Ok(Vec::new());
        };
        let mode = self
            .mode_store
            .mode_for_goal(thread_id, &goal.goal_id)
            .await?;
        if !mode.is_review_gated() {
            return Ok(Vec::new());
        }
        let open_questions = self
            .question_store
            .unresolved_for_goal(&goal.goal_id)
            .await?;
        if !open_questions.is_empty() {
            return Ok(vec![ToolRuntimeEffect::ShortCircuit {
                result: open_questions_result(ctx, &goal.goal_id, &open_questions),
            }]);
        }
        let current_hash = workspace_content_hash(&ctx.workspace_root)?;
        if self
            .review_store
            .latest_fresh_approval(&goal.goal_id, current_hash.as_deref())
            .await?
            .is_some()
        {
            return Ok(Vec::new());
        }

        let ticket = self
            .review_store
            .mint_ticket(goal.goal_id.clone(), current_hash.clone())
            .await?;
        let meta = json!({
            "ticket_id": ticket.ticket_id,
            "goal_id": ticket.goal_id,
            "reviewed_hash": current_hash,
            "message": "goal completion requires fresh reviewer approval"
        });
        Ok(vec![ToolRuntimeEffect::ShortCircuit {
            result: ToolResult {
                tool_call_id: ctx.tool_call_id.clone(),
                tool_name: ctx.tool_name.clone(),
                status: ToolStatus::Error,
                content: format!(
                    "Goal completion requires fresh reviewer approval. Opened review ticket {}.",
                    ticket.ticket_id
                ),
                meta: Some(meta),
                parts: Vec::new(),
            },
        }])
    }

    async fn after_handler_completion(
        &self,
        _ctx: &ToolInvocationContext,
        _outcome: &crate::tools::ToolOutcome,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }

    async fn failed(
        &self,
        _ctx: &ToolInvocationContext,
        _message: &str,
    ) -> Result<Vec<ToolRuntimeEffect>> {
        Ok(Vec::new())
    }
}

fn open_questions_result(
    ctx: &ToolInvocationContext,
    goal_id: &str,
    questions: &[OpenQuestion],
) -> ToolResult {
    let summary = questions
        .iter()
        .map(|question| format!("{} ({})", question.question, question.blocks_what))
        .collect::<Vec<_>>()
        .join("; ");
    let meta = json!({
        "goal_id": goal_id,
        "open_questions": questions
            .iter()
            .map(|question| json!({
                "question_id": question.question_id,
                "question": question.question,
                "blocks_what": question.blocks_what,
            }))
            .collect::<Vec<_>>()
    });
    ToolResult {
        tool_call_id: ctx.tool_call_id.clone(),
        tool_name: ctx.tool_name.clone(),
        status: ToolStatus::Error,
        content: format!("Goal completion is blocked by unresolved open question(s): {summary}."),
        meta: Some(meta),
        parts: Vec::new(),
    }
}

fn is_complete_goal_update(ctx: &ToolInvocationContext) -> bool {
    ctx.tool_name == "update_goal"
        && ctx.arguments.get("status").and_then(|value| value.as_str()) == Some("complete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::ThreadGoalMode;
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadGoalStatusRecord};
    use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
    use crate::runtime::forge::open_questions::OpenQuestionStore;
    use crate::runtime::forge::review::{ReviewStore, ReviewVerdict};
    use crate::runtime::tool_hooks::{ToolHooks, ToolInvocationContext};
    use crate::tools::{ToolCapabilities, ToolRuntimeEffect};
    use crate::types::ThreadId;

    async fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        IndexDb,
        ReviewStore,
        OpenQuestionStore,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let db_dir = dir.path().join("db");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("done.txt"), "ready").unwrap();
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&workspace)
            .output()
            .unwrap();
        let db = IndexDb::open(db_dir.join("index.sqlite")).await.unwrap();
        let review_store = ReviewStore::new(db.clone());
        let question_store = OpenQuestionStore::new(db.clone());
        let thread_id = ThreadId::new("thread_gate");
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Forge Gate".into(),
                path: workspace.clone(),
            })
            .await
            .unwrap();
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
        .bind("thread_gate title")
        .bind("thread_gate preview")
        .execute(db.pool())
        .await
        .unwrap();
        db.replace_thread_goal(
            &thread_id,
            "ship gated goal",
            ThreadGoalStatusRecord::Active,
            None,
        )
        .await
        .unwrap();
        let goal = db.get_thread_goal(&thread_id).await.unwrap().unwrap();
        ForgeGoalModeStore::new(db.clone())
            .set_mode(&thread_id, &goal.goal_id, ThreadGoalMode::Reviewed)
            .await
            .unwrap();
        (dir, workspace, db, review_store, question_store)
    }

    fn complete_ctx(workspace_root: std::path::PathBuf) -> ToolInvocationContext {
        ToolInvocationContext {
            invocation_id: "inv_update_goal".to_string(),
            tool_call_id: "call_update_goal".to_string(),
            tool_name: "update_goal".to_string(),
            arguments: serde_json::json!({ "status": "complete" }),
            thread_id: Some(ThreadId::new("thread_gate")),
            workspace_root,
            capabilities: ToolCapabilities::mutating(false),
        }
    }

    #[tokio::test]
    async fn standard_goal_completion_is_not_gated() {
        let (_dir, workspace, db, review_store, question_store) = fixture().await;
        let hooks = ForgeGateHooks::new(review_store.clone(), question_store);
        let thread_id = ThreadId::new("thread_gate");
        let goal = db.get_thread_goal(&thread_id).await.unwrap().unwrap();
        ForgeGoalModeStore::new(db.clone())
            .set_mode(&thread_id, &goal.goal_id, ThreadGoalMode::Standard)
            .await
            .unwrap();

        let effects = hooks
            .before_handler_execution(&complete_ctx(workspace))
            .await
            .unwrap();

        assert!(effects.is_empty());
        assert!(review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn complete_without_fresh_approval_opens_ticket_and_short_circuits() {
        let (_dir, workspace, db, review_store, question_store) = fixture().await;
        let hooks = ForgeGateHooks::new(review_store.clone(), question_store);

        let effects = hooks
            .before_handler_execution(&complete_ctx(workspace))
            .await
            .unwrap();

        let goal = db
            .get_thread_goal(&ThreadId::new("thread_gate"))
            .await
            .unwrap()
            .unwrap();
        let ticket = review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .expect("review ticket");
        assert!(ticket.ticket_id.starts_with("rev_"));
        assert!(ticket.baseline_hash.is_some());
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                ToolRuntimeEffect::ShortCircuit { result }
                    if result.tool_name == "update_goal"
                        && result.content.contains(&ticket.ticket_id)
            )
        }));
    }

    #[tokio::test]
    async fn complete_with_fresh_approval_is_allowed() {
        let (_dir, workspace, db, review_store, question_store) = fixture().await;
        let hooks = ForgeGateHooks::new(review_store.clone(), question_store);
        let first = hooks
            .before_handler_execution(&complete_ctx(workspace.clone()))
            .await
            .unwrap();
        let ToolRuntimeEffect::ShortCircuit { result } = &first[0] else {
            panic!("expected short circuit");
        };
        let ticket_id = result.meta.as_ref().unwrap()["ticket_id"]
            .as_str()
            .unwrap()
            .to_string();
        let goal = db
            .get_thread_goal(&ThreadId::new("thread_gate"))
            .await
            .unwrap()
            .unwrap();
        let ticket = review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .unwrap();
        review_store
            .resolve_ticket(
                &ticket_id,
                ReviewVerdict::Approve,
                ticket.baseline_hash.clone(),
                None,
            )
            .await
            .unwrap();

        let effects = hooks
            .before_handler_execution(&complete_ctx(workspace))
            .await
            .unwrap();

        assert!(effects.is_empty());
    }

    #[tokio::test]
    async fn complete_after_workspace_change_requires_new_review() {
        let (_dir, workspace, db, review_store, question_store) = fixture().await;
        let hooks = ForgeGateHooks::new(review_store.clone(), question_store);
        let first = hooks
            .before_handler_execution(&complete_ctx(workspace.clone()))
            .await
            .unwrap();
        let ToolRuntimeEffect::ShortCircuit { result } = &first[0] else {
            panic!("expected short circuit");
        };
        let ticket_id = result.meta.as_ref().unwrap()["ticket_id"]
            .as_str()
            .unwrap()
            .to_string();
        let goal = db
            .get_thread_goal(&ThreadId::new("thread_gate"))
            .await
            .unwrap()
            .unwrap();
        let approved_ticket = review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .unwrap();
        review_store
            .resolve_ticket(
                &ticket_id,
                ReviewVerdict::Approve,
                approved_ticket.baseline_hash.clone(),
                None,
            )
            .await
            .unwrap();
        std::fs::write(workspace.join("done.txt"), "changed after approval").unwrap();

        let effects = hooks
            .before_handler_execution(&complete_ctx(workspace))
            .await
            .unwrap();

        let ToolRuntimeEffect::ShortCircuit { result } = &effects[0] else {
            panic!("expected stale approval short circuit");
        };
        let new_ticket_id = result.meta.as_ref().unwrap()["ticket_id"].as_str().unwrap();
        assert_ne!(new_ticket_id, ticket_id);
        let latest = review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(latest.baseline_hash, approved_ticket.baseline_hash);
    }

    #[tokio::test]
    async fn complete_with_fresh_approval_but_open_question_is_blocked_until_resolved() {
        let (_dir, workspace, db, review_store, question_store) = fixture().await;
        let hooks = ForgeGateHooks::new(review_store.clone(), question_store.clone());
        let first = hooks
            .before_handler_execution(&complete_ctx(workspace.clone()))
            .await
            .unwrap();
        let ToolRuntimeEffect::ShortCircuit { result } = &first[0] else {
            panic!("expected review short circuit");
        };
        let ticket_id = result.meta.as_ref().unwrap()["ticket_id"]
            .as_str()
            .unwrap()
            .to_string();
        let goal = db
            .get_thread_goal(&ThreadId::new("thread_gate"))
            .await
            .unwrap()
            .unwrap();
        let ticket = review_store
            .latest_ticket(&goal.goal_id)
            .await
            .unwrap()
            .unwrap();
        review_store
            .resolve_ticket(
                &ticket_id,
                ReviewVerdict::Approve,
                ticket.baseline_hash.clone(),
                None,
            )
            .await
            .unwrap();
        let question = question_store
            .record_question(
                ThreadId::new("thread_gate"),
                goal.goal_id.clone(),
                "Which customer should approve the wording?",
                "Release note copy",
            )
            .await
            .unwrap();

        let blocked = hooks
            .before_handler_execution(&complete_ctx(workspace.clone()))
            .await
            .unwrap();

        let ToolRuntimeEffect::ShortCircuit { result } = &blocked[0] else {
            panic!("expected open question short circuit");
        };
        assert!(result.content.contains("open question"));
        assert!(result.content.contains("Which customer"));

        question_store
            .resolve_question(&question.question_id, Some("Use Acme".to_string()))
            .await
            .unwrap();
        let allowed = hooks
            .before_handler_execution(&complete_ctx(workspace))
            .await
            .unwrap();
        assert!(allowed.is_empty());
    }
}
