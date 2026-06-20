use std::marker::PhantomData;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::config::ThinkingMode;
use crate::model::resolved::ModelRef;
use crate::runtime::agent_profile::AgentType;
use crate::runtime::workflow::json_repair::{AgentJsonParseFailure, AgentJsonRepair};
use crate::types::ThreadId;

#[derive(Debug, Clone, PartialEq)]
pub struct AgentJsonRequest {
    pub label: String,
    pub prompt: String,
    pub schema_hint: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentJsonResponse {
    pub text: String,
    pub value: Value,
    pub tokens_used: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowAgentRequest<TSchema> {
    pub label: String,
    pub phase: String,
    pub prompt: String,
    pub agent_type: AgentType,
    pub schema_name: String,
    pub schema_json: Value,
    pub model: Option<ModelRef>,
    pub thinking_mode: Option<ThinkingMode>,
    pub schema: PhantomData<TSchema>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowAgentResult<T> {
    pub thread_id: Option<ThreadId>,
    pub text: String,
    pub parsed: T,
    pub tokens_used: Option<i64>,
}

#[async_trait]
pub trait AgentJsonRunner: Send + Sync {
    async fn run_json(&self, request: AgentJsonRequest) -> anyhow::Result<AgentJsonResponse>;
}

pub async fn parse_agent_json_response<T>(
    text: &str,
    repair: Option<&dyn AgentJsonRepair>,
) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    parse_agent_json_response_for_schema("AgentJsonResponse", text, repair).await
}

pub async fn parse_agent_json_response_for_schema<T>(
    schema_name: &str,
    text: &str,
    repair: Option<&dyn AgentJsonRepair>,
) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    match parse_json_object(text) {
        Ok(parsed) => Ok(parsed),
        Err(error) => {
            let failure = AgentJsonParseFailure::new(schema_name, text, error.to_string());
            let Some(repair) = repair else {
                return Err(failure.into());
            };
            let repaired = repair.repair_json(failure).await?;
            parse_json_object(&repaired).map_err(|repair_error| {
                AgentJsonParseFailure::new(schema_name, repaired, repair_error.to_string()).into()
            })
        }
    }
}

pub fn parse_json_object<T: DeserializeOwned>(text: &str) -> anyhow::Result<T> {
    let candidates = json_object_candidates(text)?;
    if candidates.is_empty() {
        return Err(anyhow::anyhow!("no JSON object found in text"));
    }
    if candidates.len() > 1 {
        return Err(anyhow::anyhow!(
            "multiple top-level JSON objects found; expected exactly one"
        ));
    }

    let candidate = &candidates[0];
    let value: Value = serde_json::from_str(candidate.source)
        .map_err(|err| anyhow::anyhow!("invalid JSON object candidate: {err}"))?;
    if !value.is_object() {
        return Err(anyhow::anyhow!("JSON value is not an object"));
    }
    Ok(serde_json::from_value(value)?)
}

pub fn build_schema_prompt(prompt: &str, schema_name: &str, schema_json: &Value) -> String {
    let compact_schema = compact_json(schema_json);

    format!(
        "{prompt}\n\nSchema name: {schema_name}\nSchema JSON: {compact_schema}\n\nReturn exactly one JSON object matching this schema. Do not include Markdown. If evidence is missing, return an empty array or a low-confidence verdict instead of inventing facts."
    )
}

fn compact_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JsonObjectCandidate<'a> {
    source: &'a str,
}

fn json_object_candidates(text: &str) -> anyhow::Result<Vec<JsonObjectCandidate<'_>>> {
    let mut candidates = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut blocked_json_depth = 0usize;
    let mut blocked_json_string = false;
    let mut blocked_json_escaped = false;

    for (index, ch) in text.char_indices() {
        if blocked_json_depth > 0 {
            if blocked_json_string {
                if blocked_json_escaped {
                    blocked_json_escaped = false;
                } else if ch == '\\' {
                    blocked_json_escaped = true;
                } else if ch == '"' {
                    blocked_json_string = false;
                }
            } else {
                match ch {
                    '"' => blocked_json_string = true,
                    '[' | '{' => blocked_json_depth += 1,
                    ']' | '}' => blocked_json_depth = blocked_json_depth.saturating_sub(1),
                    _ => {}
                }
            }
            continue;
        }

        if start.is_none() {
            if ch == '{' {
                start = Some(index);
                depth = 1;
                in_string = false;
                escaped = false;
            } else if ch == '[' {
                blocked_json_depth = 1;
            }
            continue;
        }

        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let start = start.take().expect("candidate has a start index");
                    candidates.push(JsonObjectCandidate {
                        source: &text[start..index + ch.len_utf8()],
                    });
                }
            }
            _ => {}
        }
    }

    if start.is_some() {
        return Err(anyhow::anyhow!("unterminated JSON object candidate"));
    }

    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde::Deserialize;
    use serde_json::json;
    use std::marker::PhantomData;

    use crate::config::ThinkingMode;
    use crate::model::resolved::ModelRef;
    use crate::runtime::agent_profile::AgentType;
    use crate::types::ThreadId;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Answer {
        answer: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct NestedAnswer {
        answer: String,
        meta: serde_json::Value,
    }

    struct TrailingCommaRepair;

    #[async_trait]
    impl AgentJsonRepair for TrailingCommaRepair {
        async fn repair_json(&self, failure: AgentJsonParseFailure) -> anyhow::Result<String> {
            assert_eq!(failure.schema_name, "AgentJsonResponse");
            assert!(failure.error.contains("invalid JSON object candidate"));
            Ok(failure.raw_text.replace(",}", "}"))
        }
    }

    #[tokio::test]
    async fn parsing_helper_uses_one_repair_attempt() {
        let parsed: Answer =
            parse_agent_json_response("{\"answer\":\"yes\",}", Some(&TrailingCommaRepair))
                .await
                .expect("parse repaired json");

        assert_eq!(
            parsed,
            Answer {
                answer: "yes".to_string()
            }
        );
    }

    struct BadRepair;

    #[async_trait]
    impl AgentJsonRepair for BadRepair {
        async fn repair_json(&self, _failure: AgentJsonParseFailure) -> anyhow::Result<String> {
            Ok("{still bad".to_string())
        }
    }

    #[tokio::test]
    async fn parse_failure_without_repair_is_typed() {
        let error = parse_agent_json_response_for_schema::<Answer>("Answer", "{bad", None)
            .await
            .expect_err("invalid json should fail");
        let failure = error
            .downcast_ref::<AgentJsonParseFailure>()
            .expect("typed parse failure");

        assert_eq!(failure.schema_name, "Answer");
        assert_eq!(failure.raw_text, "{bad");
        assert!(failure.error.contains("unterminated JSON object"));
    }

    #[tokio::test]
    async fn parse_failure_after_repair_is_typed_with_repaired_text() {
        let error =
            parse_agent_json_response_for_schema::<Answer>("Answer", "{bad", Some(&BadRepair))
                .await
                .expect_err("bad repaired json should fail");
        let failure = error
            .downcast_ref::<AgentJsonParseFailure>()
            .expect("typed repaired parse failure");

        assert_eq!(failure.schema_name, "Answer");
        assert_eq!(failure.raw_text, "{still bad");
        assert!(failure.error.contains("unterminated JSON object"));
    }

    #[test]
    fn request_and_response_hold_json_payloads() {
        let request = AgentJsonRequest {
            label: "vote".to_string(),
            prompt: "Return JSON".to_string(),
            schema_hint: Some(json!({"type": "object"})),
        };
        let response = AgentJsonResponse {
            text: "{\"ok\":true}".to_string(),
            value: json!({"ok": true}),
            tokens_used: Some(10),
        };

        assert_eq!(request.label, "vote");
        assert_eq!(response.value["ok"], true);
        assert_eq!(response.tokens_used, Some(10));
    }

    #[test]
    fn parse_json_object_accepts_full_json_object() {
        let parsed: Answer =
            parse_json_object("{\"answer\":\"yes\"}").expect("parse full json object");

        assert_eq!(
            parsed,
            Answer {
                answer: "yes".to_string()
            }
        );
    }

    #[test]
    fn parse_json_object_accepts_fenced_json_object() {
        let parsed: Answer =
            parse_json_object("```json\n{\"answer\":\"yes\"}\n```").expect("parse fenced json");

        assert_eq!(parsed.answer, "yes");
    }

    #[test]
    fn parse_json_object_accepts_surrounding_prose_with_one_object() {
        let parsed: Answer = parse_json_object(
            "The answer follows:\n{\"answer\":\"yes with {braces} in a string\"}\nThanks.",
        )
        .expect("parse json object in prose");

        assert_eq!(parsed.answer, "yes with {braces} in a string");
    }

    #[test]
    fn parse_json_object_accepts_nested_objects() {
        let parsed: NestedAnswer = parse_json_object(
            r#"{"answer":"yes","meta":{"source":{"url":"https://example.com"}}}"#,
        )
        .expect("parse nested json object");

        assert_eq!(parsed.answer, "yes");
        assert_eq!(parsed.meta["source"]["url"], "https://example.com");
    }

    #[test]
    fn parse_json_object_respects_escaped_quotes_and_backslashes() {
        let parsed: Answer =
            parse_json_object(r#"{"answer":"quote: \"yes\" path: C:\\tmp\\file"}"#)
                .expect("parse escaped string content");

        assert_eq!(parsed.answer, r#"quote: "yes" path: C:\tmp\file"#);
    }

    #[test]
    fn parse_json_object_rejects_multiple_objects() {
        let error = parse_json_object::<Answer>("{\"answer\":\"yes\"}\n{\"answer\":\"no\"}")
            .expect_err("reject multiple objects");

        assert!(
            error
                .to_string()
                .contains("multiple top-level JSON objects"),
            "{error}"
        );
    }

    #[test]
    fn parse_json_object_rejects_valid_object_plus_malformed_candidate() {
        let error = parse_json_object::<Answer>("{\"answer\":\"yes\"}\n{not json")
            .expect_err("reject malformed extra object-looking content");

        assert!(
            error
                .to_string()
                .contains("unterminated JSON object candidate"),
            "{error}"
        );
    }

    #[test]
    fn parse_json_object_rejects_object_nested_inside_array() {
        let error = parse_json_object::<Answer>("[{\"answer\":\"yes\"}]")
            .expect_err("reject object inside non-object JSON value");

        assert!(
            error.to_string().contains("no JSON object found"),
            "{error}"
        );
    }

    #[test]
    fn parse_json_object_rejects_object_nested_inside_spaced_array() {
        let error = parse_json_object::<Answer>("[ {\"answer\":\"yes\"} ]")
            .expect_err("reject object inside spaced array");

        assert!(
            error.to_string().contains("no JSON object found"),
            "{error}"
        );
    }

    #[test]
    fn parse_json_object_skips_array_noise_before_real_object() {
        let parsed: Answer =
            parse_json_object("Noise [ {\"ignored\":true} ] then {\"answer\":\"yes\"}")
                .expect("parse object after array noise");

        assert_eq!(parsed.answer, "yes");
    }

    #[test]
    fn parse_json_object_rejects_prose_only() {
        let error =
            parse_json_object::<Answer>("no structured answer here").expect_err("reject prose");

        assert!(
            error.to_string().contains("no JSON object found"),
            "{error}"
        );
    }

    #[test]
    fn schema_prompt_contains_instruction_and_compact_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        });

        let prompt = build_schema_prompt("Decide.", "Answer", &schema);

        assert!(prompt.contains("Decide."));
        assert!(prompt.contains("Schema name: Answer"));
        assert!(prompt.contains("Return exactly one JSON object matching this schema"));
        assert!(prompt.contains("Do not include Markdown"));
        assert!(prompt.contains("instead of inventing facts"));
        assert!(prompt.contains(
            r#"{"properties":{"answer":{"type":"string"}},"required":["answer"],"type":"object"}"#
        ));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Verdict {
        verdict: String,
    }

    #[test]
    fn typed_workflow_agent_request_and_result_hold_expected_values() {
        let request = WorkflowAgentRequest::<Verdict> {
            label: "review-source".to_string(),
            phase: "evidence_review".to_string(),
            prompt: "Review the evidence.".to_string(),
            agent_type: AgentType::Reviewer,
            schema_name: "Verdict".to_string(),
            schema_json: json!({"type": "object"}),
            model: Some(ModelRef::new("openai", "gpt-test")),
            thinking_mode: Some(ThinkingMode::High),
            schema: PhantomData,
        };
        let result = WorkflowAgentResult {
            thread_id: Some(ThreadId::new("thread_child")),
            text: "{\"verdict\":\"low_confidence\"}".to_string(),
            parsed: Verdict {
                verdict: "low_confidence".to_string(),
            },
            tokens_used: Some(42),
        };

        assert_eq!(request.label, "review-source");
        assert_eq!(request.phase, "evidence_review");
        assert_eq!(request.agent_type, AgentType::Reviewer);
        assert_eq!(request.model, Some(ModelRef::new("openai", "gpt-test")));
        assert_eq!(request.thinking_mode, Some(ThinkingMode::High));
        assert_eq!(
            result.thread_id.as_ref().map(ThreadId::as_str),
            Some("thread_child")
        );
        assert_eq!(result.parsed.verdict, "low_confidence");
        assert_eq!(result.tokens_used, Some(42));
    }
}
