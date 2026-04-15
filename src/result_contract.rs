use anyhow::{bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::AgentRole;
use crate::types::{SessionId, TurnId};

pub const STRUCTURED_RESULT_SCHEMA_VERSION: &str = "phase3_p2/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgeRecommendation {
    Approve,
    Revise,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructuredResultPayload {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredSessionResult {
    pub schema_version: String,
    pub agent_role: AgentRole,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<TurnId>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<String>,
    pub payload: StructuredResultPayload,
}

impl StructuredSessionResult {
    pub fn validate_role(&self) -> Result<()> {
        if self.schema_version != STRUCTURED_RESULT_SCHEMA_VERSION {
            bail!(
                "unsupported structured result schema version: {}",
                self.schema_version
            );
        }

        let expected_role = match &self.payload {
            StructuredResultPayload::Spec { .. } => AgentRole::Spec,
            StructuredResultPayload::Test { .. } => AgentRole::Test,
            StructuredResultPayload::Judge { .. } => AgentRole::Judge,
        };

        if self.agent_role != expected_role {
            bail!(
                "structured result payload kind does not match session role: expected {:?}, got {:?}",
                expected_role,
                self.agent_role
            );
        }

        Ok(())
    }
}
