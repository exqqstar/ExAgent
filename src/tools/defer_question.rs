use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::runtime::forge::open_questions::OpenQuestionStore;
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub(crate) struct DeferQuestionTool {
    store: OpenQuestionStore,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeferQuestionArgs {
    question: String,
    blocks_what: String,
}

impl DeferQuestionTool {
    pub(crate) fn new(store: OpenQuestionStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ToolHandler for DeferQuestionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "defer_question",
            "Record a user question that blocks final goal completion without pausing current work.",
            serde_json::to_value(schemars::schema_for!(DeferQuestionArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: true,
            requires_approval: false,
            parallel_safe: false,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<DeferQuestionArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let question = args.question.trim().to_string();
        if question.is_empty() {
            return error(call.id, call.name, "question must not be empty");
        }
        let blocks_what = args.blocks_what.trim().to_string();
        if blocks_what.is_empty() {
            return error(call.id, call.name, "blocks_what must not be empty");
        }
        let Some(thread_id) = ctx.thread_id.clone() else {
            return error(
                call.id,
                call.name,
                "defer_question requires a runtime thread_id",
            );
        };
        let goal = match self.store.db().get_thread_goal(&thread_id).await {
            Ok(Some(goal)) => goal,
            Ok(None) => return error(call.id, call.name, "defer_question requires an active goal"),
            Err(err) => return error(call.id, call.name, err.to_string()),
        };

        match self
            .store
            .record_question(
                thread_id,
                goal.goal_id.clone(),
                question.clone(),
                blocks_what.clone(),
            )
            .await
        {
            Ok(recorded) => {
                let value = json!({
                    "question_id": recorded.question_id,
                    "goal_id": recorded.goal_id,
                    "question": recorded.question,
                    "blocks_what": recorded.blocks_what,
                });
                ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Success,
                    content: value.to_string(),
                    meta: Some(value),
                    parts: Vec::new(),
                })
                .with_effect(ToolRuntimeEffect::OpenQuestionRecorded {
                    question_id: recorded.question_id,
                    goal_id: recorded.goal_id,
                    question: recorded.question,
                    blocks_what: recorded.blocks_what,
                })
            }
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

fn error(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    content: impl Into<String>,
) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        status: ToolStatus::Error,
        content: content.into(),
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::AgentConfig;
    use crate::exec_session::ExecSessionManager;
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadGoalStatusRecord};
    use crate::policy::{PolicyManager, PolicyMode};
    use crate::registry::ToolContext;
    use crate::runtime::agent_profile::{profile_for_type, AgentType};
    use crate::runtime::forge::open_questions::OpenQuestionStore;
    use crate::tools::{ToolHandler, ToolInvocation, ToolRuntimeEffect};
    use crate::types::{ThreadId, ToolCall, ToolStatus};

    async fn fixture() -> (tempfile::TempDir, OpenQuestionStore, ToolContext, String) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Defer Question".into(),
                path: workspace.clone(),
            })
            .await
            .unwrap();
        let thread_id = ThreadId::new("thread_defer_question");
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
        let store = OpenQuestionStore::new(db);
        let ctx = ToolContext {
            config: AgentConfig {
                workspace_root: workspace.clone(),
                cwd: workspace,
                policy_mode: PolicyMode::Off,
                forge_review_gate_enabled: true,
                ..AgentConfig::default()
            },
            thread_id: Some(thread_id),
            turn_id: None,
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: profile_for_type(Some(AgentType::Worker)).tool_policy,
            inbox: None,
            goal_api: None,
            memory_api: None,
        };
        (dir, store, ctx, goal_id)
    }

    #[tokio::test]
    async fn defer_question_records_open_question_without_waiting_for_user_input() {
        let (_dir, store, ctx, goal_id) = fixture().await;
        let tool = DeferQuestionTool::new(store.clone());

        let outcome = tool
            .handle(
                ToolInvocation {
                    invocation_id: "inv_defer_question".to_string(),
                    call: ToolCall {
                        id: "call_defer_question".to_string(),
                        name: "defer_question".to_string(),
                        arguments: serde_json::json!({
                            "question": "Which rollout cohort should get this first?",
                            "blocks_what": "Release targeting"
                        }),
                        thought_signature: None,
                    },
                },
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        assert!(outcome.effects.iter().any(|effect| {
            matches!(
                effect,
                ToolRuntimeEffect::OpenQuestionRecorded { question, blocks_what, .. }
                    if question == "Which rollout cohort should get this first?"
                        && blocks_what == "Release targeting"
            )
        }));
        assert!(!outcome
            .effects
            .iter()
            .any(|effect| matches!(effect, ToolRuntimeEffect::UserInputRequested { .. })));
        assert_eq!(store.unresolved_for_goal(&goal_id).await.unwrap().len(), 1);
    }
}
