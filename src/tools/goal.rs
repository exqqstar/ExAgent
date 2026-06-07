use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::app_server::protocol::{validate_thread_goal_objective, ThreadGoal, ThreadGoalStatus};
use crate::registry::ToolContext;
use crate::runtime::goal::GoalToolApi;
use crate::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolResult, ToolStatus};

#[derive(Clone)]
pub(crate) struct GetGoalTool {
    api: Arc<GoalToolApi>,
}

#[derive(Clone)]
pub(crate) struct CreateGoalTool {
    api: Arc<GoalToolApi>,
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
    pub(crate) fn new(api: Arc<GoalToolApi>) -> Self {
        Self { api }
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
        ToolSpec::function(
            "create_goal",
            "Create a structured thread goal when explicitly instructed.",
            serde_json::to_value(schemars::schema_for!(CreateGoalArgs)).unwrap(),
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
        match self
            .api
            .create_goal(thread_id, args.objective, args.token_budget)
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
        if let Some(turn_id) = ctx.turn_id.as_ref() {
            if let Err(err) = self.api.account_update_goal_tool(thread_id, turn_id).await {
                return error(call.id, call.name, err.to_string());
            }
        }
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
            )
            .with_effect(ToolRuntimeEffect::ThreadGoalUpdated { goal }),
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
    })
}

fn error(tool_call_id: String, tool_name: String, content: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content: content.into(),
        meta: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::goal::runtime::GoalRuntime;

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
}
