use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::app_server::protocol::{
    validate_thread_goal_objective, ThreadGoal, ThreadGoalMode, ThreadGoalStatus,
};
use crate::registry::ToolContext;
use crate::runtime::goal::{CreateGoalOptions, GoalToolApi};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub(crate) struct GetGoalTool {
    api: Arc<GoalToolApi>,
}

#[derive(Clone)]
pub(crate) struct CreateGoalTool {
    api: Arc<GoalToolApi>,
    forge_modes_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct UpdateGoalTool {
    api: Arc<GoalToolApi>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetGoalArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateGoalArgs {
    objective: String,
    token_budget: Option<i64>,
    #[serde(default)]
    mode: CreateGoalModeArg,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CreateGoalModeArg {
    #[default]
    Standard,
    Reviewed,
    Intensive,
}

impl From<CreateGoalModeArg> for ThreadGoalMode {
    fn from(mode: CreateGoalModeArg) -> Self {
        match mode {
            CreateGoalModeArg::Standard => ThreadGoalMode::Standard,
            CreateGoalModeArg::Reviewed => ThreadGoalMode::Reviewed,
            CreateGoalModeArg::Intensive => ThreadGoalMode::Intensive,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateGoalArgs {
    status: UpdateGoalStatusArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum UpdateGoalStatusArg {
    Complete,
    Blocked,
}

impl GetGoalTool {
    pub(crate) fn new(api: Arc<GoalToolApi>) -> Self {
        Self { api }
    }
}

impl CreateGoalTool {
    pub(crate) fn new_with_forge_modes(api: Arc<GoalToolApi>, forge_modes_enabled: bool) -> Self {
        Self {
            api,
            forge_modes_enabled,
        }
    }
}

impl UpdateGoalTool {
    pub(crate) fn new(api: Arc<GoalToolApi>) -> Self {
        Self { api }
    }
}

#[async_trait]
impl ToolHandler for GetGoalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "get_goal",
            "Get the current structured thread goal.",
            serde_json::to_value(schemars::schema_for!(GetGoalArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let Some(thread_id) = ctx.thread_id.as_ref() else {
            return error(call.id, call.name, "thread context missing");
        };
        match self.api.get_goal(thread_id).await {
            Ok(goal) => success_json(call.id, call.name, json!({ "goal": goal })),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

#[async_trait]
impl ToolHandler for CreateGoalTool {
    fn spec(&self) -> ToolSpec {
        let mut schema = serde_json::to_value(schemars::schema_for!(CreateGoalArgs)).unwrap();
        if !self.forge_modes_enabled {
            if let Some(properties) = schema
                .get_mut("properties")
                .and_then(|properties| properties.as_object_mut())
            {
                properties.remove("mode");
            }
            if let Some(required) = schema
                .get_mut("required")
                .and_then(|required| required.as_array_mut())
            {
                required.retain(|value| value.as_str() != Some("mode"));
            }
        }
        ToolSpec::function(
            "create_goal",
            "Create a structured thread goal when explicitly instructed.",
            schema,
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: CreateGoalArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        if let Err(err) = validate_thread_goal_objective(&args.objective) {
            return error(call.id, call.name, err);
        }
        if args.token_budget.is_some_and(|budget| budget <= 0) {
            return error(call.id, call.name, "token_budget must be positive");
        }
        let Some(thread_id) = ctx.thread_id.as_ref() else {
            return error(call.id, call.name, "thread context missing");
        };
        let options = CreateGoalOptions {
            mode: if self.forge_modes_enabled {
                args.mode.into()
            } else {
                ThreadGoalMode::Standard
            },
        };
        match self
            .api
            .create_goal_with_options(thread_id, args.objective, args.token_budget, options)
            .await
        {
            Ok(goal) => success_json(call.id, call.name, json!({ "goal": goal })),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

#[async_trait]
impl ToolHandler for UpdateGoalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "update_goal",
            "Mark the current structured thread goal complete or strictly blocked.",
            json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete", "blocked"]
                    }
                },
                "required": ["status"],
                "additionalProperties": false
            }),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: UpdateGoalArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => return error(call.id, call.name, err.to_string()),
        };
        let Some(thread_id) = ctx.thread_id.as_ref() else {
            return error(call.id, call.name, "thread context missing");
        };
        let status = match args.status {
            UpdateGoalStatusArg::Complete => ThreadGoalStatus::Complete,
            UpdateGoalStatusArg::Blocked => ThreadGoalStatus::Blocked,
        };
        match self.api.update_goal(thread_id, status).await {
            Ok(goal) => success_json(
                call.id,
                call.name,
                json!({
                    "goal": goal,
                    "message": update_goal_message(&goal),
                }),
            ),
            Err(err) => error(call.id, call.name, err.to_string()),
        }
    }
}

fn update_goal_message(goal: &ThreadGoal) -> &'static str {
    match goal.status {
        ThreadGoalStatus::Complete => {
            "Goal marked complete. Report final token and time usage to the user."
        }
        ThreadGoalStatus::Blocked => "Goal marked blocked. Explain the blocking condition.",
        _ => "Goal updated.",
    }
}

fn success_json(tool_call_id: String, tool_name: String, value: serde_json::Value) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Success,
        content: value.to_string(),
        meta: Some(value),
        parts: Vec::new(),
    })
}

fn error(tool_call_id: String, tool_name: String, content: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content: content.into(),
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::ThreadGoalMode;
    use crate::config::AgentConfig;
    use crate::exec_session::ExecSessionManager;
    use crate::policy::PolicyManager;
    use crate::runtime::agent_profile::AgentToolPolicy;
    use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
    use crate::runtime::goal::runtime::GoalRuntime;
    use crate::tools::registry::ToolContext;
    use crate::types::{ThreadId, ToolCall};

    #[tokio::test]
    async fn update_goal_schema_only_allows_complete_and_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let api = Arc::new(GoalToolApi::new(Arc::new(GoalRuntime::new(db))));
        let schema = UpdateGoalTool::new(api).spec().to_internal_schema();
        let allowed = schema["input_schema"]["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(allowed, vec!["complete", "blocked"]);
    }

    #[tokio::test]
    async fn create_goal_schema_exposes_mode_only_when_forge_enabled() {
        let (_dir, api, _ctx, _thread_id, _db) = fixture().await;

        let hidden = CreateGoalTool::new_with_forge_modes(api.clone(), false)
            .spec()
            .to_internal_schema();
        assert!(hidden["input_schema"]["properties"]
            .as_object()
            .unwrap()
            .get("mode")
            .is_none());
        assert!(hidden["input_schema"]["properties"]
            .as_object()
            .unwrap()
            .get("intensive")
            .is_none());

        let visible = CreateGoalTool::new_with_forge_modes(api, true)
            .spec()
            .to_internal_schema();
        let properties = visible["input_schema"]["properties"].as_object().unwrap();
        assert!(properties.get("intensive").is_none());
        assert!(properties.get("mode").is_some());
        let schema_text = visible.to_string();
        for mode in ["standard", "reviewed", "intensive"] {
            assert!(
                schema_text.contains(mode),
                "create_goal schema should expose {mode} mode"
            );
        }
    }

    #[tokio::test]
    async fn create_goal_mode_is_persisted_only_when_forge_enabled() {
        let (_dir, api, ctx, thread_id, db) = fixture().await;
        let tool = CreateGoalTool::new_with_forge_modes(api.clone(), true);

        let outcome = tool
            .handle(invocation("thread carefully", Some("intensive")), &ctx)
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let goal_id = outcome.model_result.meta.as_ref().unwrap()["goal"]["goal_id"]
            .as_str()
            .unwrap();
        assert_eq!(
            ForgeGoalModeStore::new(db.clone())
                .mode_for_goal(&thread_id, goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Intensive
        );

        let second_thread = ThreadId::new("thread_goal_plain");
        insert_thread(&db, &second_thread).await;
        let plain_ctx = ToolContext {
            thread_id: Some(second_thread.clone()),
            ..ctx
        };
        let disabled_tool = CreateGoalTool::new_with_forge_modes(api, false);
        let outcome = disabled_tool
            .handle(invocation("standard goal", Some("intensive")), &plain_ctx)
            .await;
        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        let goal_id = outcome.model_result.meta.as_ref().unwrap()["goal"]["goal_id"]
            .as_str()
            .unwrap();
        assert_eq!(
            ForgeGoalModeStore::new(db)
                .mode_for_goal(&second_thread, goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Standard
        );
    }

    async fn fixture() -> (
        tempfile::TempDir,
        Arc<GoalToolApi>,
        ToolContext,
        ThreadId,
        crate::index_db::IndexDb,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let thread_id = ThreadId::new("thread_goal_intensive");
        insert_thread(&db, &thread_id).await;
        let api = Arc::new(GoalToolApi::new(Arc::new(GoalRuntime::new(db.clone()))));
        let ctx = ToolContext {
            config: AgentConfig {
                workspace_root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
                ..AgentConfig::default()
            },
            thread_id: Some(thread_id.clone()),
            turn_id: None,
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: AgentToolPolicy::all(),
            inbox: None,
            goal_api: Some(api.clone()),
        };
        (dir, api, ctx, thread_id, db)
    }

    async fn insert_thread(db: &crate::index_db::IndexDb, thread_id: &ThreadId) {
        let workspace = std::env::temp_dir().join("exagent-goal-tool-tests");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let project = db
            .upsert_project(crate::index_db::ProjectUpsert {
                name: "Forge".to_string(),
                path: workspace,
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
        .bind(format!("{}.jsonl", thread_id.as_str()))
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
    }

    fn invocation(objective: &str, mode: Option<&str>) -> ToolInvocation {
        let mut arguments = serde_json::json!({
            "objective": objective,
            "token_budget": 100
        });
        if let Some(mode) = mode {
            arguments["mode"] = serde_json::json!(mode);
        }
        ToolInvocation {
            invocation_id: "inv_create_goal".to_string(),
            call: ToolCall {
                id: "call_create_goal".to_string(),
                name: "create_goal".to_string(),
                arguments,
                thought_signature: None,
            },
        }
    }
}
