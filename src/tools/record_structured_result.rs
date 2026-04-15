use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::result_contract::{
    JudgeRecommendation, StructuredResultPayload, StructuredSessionResult,
    STRUCTURED_RESULT_SCHEMA_VERSION,
};
use crate::session::AgentRole;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RecordStructuredResultArgs {
    pub summary: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    pub payload: StructuredResultPayloadArgs,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructuredResultPayloadArgs {
    Spec {
        goals: Vec<String>,
        non_goals: Vec<String>,
        acceptance_criteria: Vec<String>,
        contract_boundaries: Vec<String>,
    },
    Test {
        regression_risks: Vec<String>,
        test_matrix: Vec<String>,
        coverage_gaps: Vec<String>,
    },
    Judge {
        scope_issues: Vec<String>,
        missing_criteria: Vec<String>,
        blockers: Vec<String>,
        recommendation: JudgeRecommendation,
    },
}

impl From<StructuredResultPayloadArgs> for StructuredResultPayload {
    fn from(value: StructuredResultPayloadArgs) -> Self {
        match value {
            StructuredResultPayloadArgs::Spec {
                goals,
                non_goals,
                acceptance_criteria,
                contract_boundaries,
            } => StructuredResultPayload::Spec {
                goals,
                non_goals,
                acceptance_criteria,
                contract_boundaries,
            },
            StructuredResultPayloadArgs::Test {
                regression_risks,
                test_matrix,
                coverage_gaps,
            } => StructuredResultPayload::Test {
                regression_risks,
                test_matrix,
                coverage_gaps,
            },
            StructuredResultPayloadArgs::Judge {
                scope_issues,
                missing_criteria,
                blockers,
                recommendation,
            } => StructuredResultPayload::Judge {
                scope_issues,
                missing_criteria,
                blockers,
                recommendation,
            },
        }
    }
}

pub struct RecordStructuredResultTool;

#[async_trait]
impl Tool for RecordStructuredResultTool {
    fn name(&self) -> &'static str {
        "record_structured_result"
    }

    fn description(&self) -> &'static str {
        "Persist a structured result contract for spec, test, or judge sessions"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(RecordStructuredResultArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let args: RecordStructuredResultArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                };
            }
        };

        match record_structured_result(&args, ctx) {
            Ok(result) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: "Recorded structured result".into(),
                meta: Some(json!({
                    "schema_version": result.schema_version,
                    "agent_role": result.agent_role,
                    "session_id": result.session_id,
                    "source_turn_id": result.source_turn_id,
                })),
            },
            Err(err) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
            },
        }
    }
}

fn record_structured_result(
    args: &RecordStructuredResultArgs,
    ctx: &ToolContext,
) -> anyhow::Result<StructuredSessionResult> {
    let session_id = ctx
        .session_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("record_structured_result requires a runtime session_id"))?;
    let snapshot =
        crate::transcript::read_session_snapshot(&ctx.config.workspace_root, session_id)?;

    match snapshot.agent_role {
        AgentRole::Spec | AgentRole::Test | AgentRole::Judge => {}
        _ => {
            return Err(anyhow::anyhow!(
                "session role {:?} cannot record a structured result in P2",
                snapshot.agent_role
            ));
        }
    }

    let result = StructuredSessionResult {
        schema_version: STRUCTURED_RESULT_SCHEMA_VERSION.into(),
        agent_role: snapshot.agent_role.clone(),
        session_id: snapshot.session_id.clone(),
        parent_session_id: snapshot.parent_session_id.clone(),
        source_turn_id: ctx.turn_id.clone(),
        summary: args.summary.clone(),
        assumptions: args.assumptions.clone(),
        risks: args.risks.clone(),
        open_questions: args.open_questions.clone(),
        payload: args.payload.clone().into(),
    };
    result.validate_role()?;
    crate::transcript::record_structured_result(
        &ctx.config.workspace_root,
        &snapshot.session_id,
        ctx.turn_id.as_ref(),
        result.clone(),
    )?;

    Ok(result)
}
