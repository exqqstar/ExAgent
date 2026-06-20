use std::fmt;

use serde::{Deserialize, Serialize};

use crate::app_server::protocol::WorkflowPresetId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(transparent)]
pub struct WorkflowRunId(String);

impl WorkflowRunId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkflowRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for WorkflowRunId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for WorkflowRunId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeepSearchLimits {
    pub max_angles: usize,
    pub max_sources: usize,
    pub max_claims: usize,
    pub votes_per_claim: usize,
    pub refutations_required: usize,
    pub max_concurrency: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_secs: Option<u64>,
}

impl DeepSearchLimits {
    pub fn for_preset(preset_id: WorkflowPresetId) -> Self {
        match preset_id {
            WorkflowPresetId::Quick => Self {
                max_angles: 3,
                max_sources: 8,
                max_claims: 8,
                votes_per_claim: 2,
                refutations_required: 2,
                max_concurrency: 5,
                token_budget: Some(80_000),
                max_runtime_secs: Some(120),
            },
            WorkflowPresetId::Standard => Self {
                max_angles: 4,
                max_sources: 12,
                max_claims: 12,
                votes_per_claim: 2,
                refutations_required: 2,
                max_concurrency: 6,
                token_budget: Some(160_000),
                max_runtime_secs: Some(300),
            },
            WorkflowPresetId::Deep => Self {
                max_angles: 5,
                max_sources: 15,
                max_claims: 20,
                votes_per_claim: 3,
                refutations_required: 2,
                max_concurrency: 8,
                token_budget: Some(320_000),
                max_runtime_secs: Some(600),
            },
        }
    }

    pub fn planned_agent_calls(&self) -> usize {
        1 // scope
            + self.max_sources
            + (self.max_claims * self.votes_per_claim)
            + 1 // synthesize
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowLimits {
    DeepSearch(DeepSearchLimits),
}

impl WorkflowLimits {
    pub fn deep_search_for_preset(preset_id: WorkflowPresetId) -> Self {
        Self::DeepSearch(DeepSearchLimits::for_preset(preset_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::WorkflowPresetId;
    use serde_json::json;

    #[test]
    fn workflow_run_id_is_transparent_string_id() {
        let id = WorkflowRunId::from("workflow_1".to_string());

        assert_eq!(id.as_str(), "workflow_1");
        assert_eq!(id.to_string(), "workflow_1");
        assert_eq!(serde_json::to_value(&id).unwrap(), json!("workflow_1"));
        assert_eq!(
            serde_json::from_value::<WorkflowRunId>(json!("workflow_1")).unwrap(),
            id
        );
    }

    #[test]
    fn deep_search_preset_limits_match_planned_agent_calls() {
        let cases = [
            (
                WorkflowPresetId::Quick,
                3,
                8,
                8,
                2,
                2,
                5,
                Some(80_000),
                Some(120),
                26,
            ),
            (
                WorkflowPresetId::Standard,
                4,
                12,
                12,
                2,
                2,
                6,
                Some(160_000),
                Some(300),
                38,
            ),
            (
                WorkflowPresetId::Deep,
                5,
                15,
                20,
                3,
                2,
                8,
                Some(320_000),
                Some(600),
                77,
            ),
        ];

        for (
            preset,
            max_angles,
            max_sources,
            max_claims,
            votes_per_claim,
            refutations_required,
            max_concurrency,
            token_budget,
            max_runtime_secs,
            planned_calls,
        ) in cases
        {
            let limits = DeepSearchLimits::for_preset(preset);
            assert_eq!(limits.max_angles, max_angles);
            assert_eq!(limits.max_sources, max_sources);
            assert_eq!(limits.max_claims, max_claims);
            assert_eq!(limits.votes_per_claim, votes_per_claim);
            assert_eq!(limits.refutations_required, refutations_required);
            assert_eq!(limits.max_concurrency, max_concurrency);
            assert_eq!(limits.token_budget, token_budget);
            assert_eq!(limits.max_runtime_secs, max_runtime_secs);
            assert_eq!(limits.planned_agent_calls(), planned_calls);
            assert_eq!(
                WorkflowLimits::deep_search_for_preset(preset),
                WorkflowLimits::DeepSearch(limits)
            );
        }
    }
}
