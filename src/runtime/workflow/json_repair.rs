use async_trait::async_trait;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentJsonParseFailure {
    pub schema_name: String,
    pub raw_text: String,
    pub error: String,
}

impl AgentJsonParseFailure {
    pub fn new(
        schema_name: impl Into<String>,
        raw_text: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            schema_name: schema_name.into(),
            raw_text: raw_text.into(),
            error: error.into(),
        }
    }
}

impl fmt::Display for AgentJsonParseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to parse {} JSON: {}",
            self.schema_name, self.error
        )
    }
}

impl Error for AgentJsonParseFailure {}

#[async_trait]
pub trait AgentJsonRepair: Send + Sync {
    async fn repair_json(&self, failure: AgentJsonParseFailure) -> anyhow::Result<String>;
}

pub fn json_repair_prompt(failure: &AgentJsonParseFailure) -> String {
    format!(
        "Repair this workflow agent JSON response.\n\
Schema name: {schema_name}\n\
Parse error: {error}\n\n\
Original response:\n{raw_text}\n\n\
Return exactly one valid JSON object. Preserve the original meaning and evidence. \
Do not invent facts, URLs, quotes, or verdicts. Do not include Markdown.",
        schema_name = failure.schema_name,
        error = failure.error,
        raw_text = failure.raw_text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_prompt_preserves_failure_context_and_guardrails() {
        let failure = AgentJsonParseFailure::new("ScopeOutput", "{bad", "expected value");

        let prompt = json_repair_prompt(&failure);

        assert!(prompt.contains("Schema name: ScopeOutput"));
        assert!(prompt.contains("Parse error: expected value"));
        assert!(prompt.contains("Original response:\n{bad"));
        assert!(prompt.contains("Do not invent facts"));
        assert!(prompt.contains("Do not include Markdown"));
    }
}
