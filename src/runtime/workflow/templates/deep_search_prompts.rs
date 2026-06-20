use schemars::{schema_for, JsonSchema};
use serde::Serialize;
use serde_json::{json, Value};

use crate::runtime::workflow::agent_runner::build_schema_prompt;

use super::deep_search::{
    ClaimVerdict, DeepSearchReport, RankedClaim, ScopeOutput, SearchAngle, SearchOutput,
    SelectedSource, SourceExtractOutput,
};

pub fn scope_prompt(question: &str) -> String {
    build_schema_prompt(
        &format!(
            "Turn the research question into concise search angles.\nQuestion: {question}\nUse 3-6 angles with specific search queries."
        ),
        "ScopeOutput",
        &schema_json::<ScopeOutput>(),
    )
}

pub fn search_prompt(question: &str, angle: &SearchAngle) -> String {
    build_schema_prompt(
        &format!(
            "List candidate sources for one research angle.\nQuestion: {question}\nAngle: {}\nQuery: {}\nReturn only source-like web results.",
            angle.label, angle.query
        ),
        "SearchOutput",
        &schema_json::<SearchOutput>(),
    )
}

pub fn extract_prompt(question: &str, source: &SelectedSource, source_text: &str) -> String {
    build_schema_prompt(
        &format!(
            "Extract verifiable claims from this source.\nQuestion: {question}\nSource URL: {}\nSource title: {}\nSource text:\n{}",
            source.url,
            source.title,
            trim_for_prompt(source_text, 8_000)
        ),
        "SourceExtractOutput",
        &schema_json::<SourceExtractOutput>(),
    )
}

pub fn verify_prompt(question: &str, claim: &RankedClaim, evidence_text: &str) -> String {
    build_schema_prompt(
        &format!(
            "Check whether the evidence refutes the claim.\nQuestion: {question}\nClaim id: {}\nClaim: {}\nOriginal source: {}\nEvidence text:\n{}",
            claim.id,
            claim.claim.claim,
            claim.source_url,
            trim_for_prompt(evidence_text, 6_000)
        ),
        "ClaimVerdict",
        &schema_json::<ClaimVerdict>(),
    )
}

pub fn synthesize_prompt(
    question: &str,
    survived: &[RankedClaim],
    killed: &[RankedClaim],
) -> String {
    let survived_claims = claims_for_prompt(survived);
    let killed_claims = claims_for_prompt(killed);
    build_schema_prompt(
        &format!(
            "Write a concise research report.\nQuestion: {question}\nSurviving unrefuted claims JSON: {survived_claims}\nRefuted claims JSON: {killed_claims}"
        ),
        "DeepSearchReport",
        &schema_json::<DeepSearchReport>(),
    )
}

fn schema_json<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).unwrap_or_else(|_| json!({}))
}

fn trim_for_prompt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut end = 0;
    for (count, (index, ch)) in text.char_indices().enumerate() {
        if count >= max_chars {
            break;
        }
        end = index + ch.len_utf8();
    }
    format!("{}...", &text[..end])
}

fn claims_for_prompt(claims: &[RankedClaim]) -> String {
    #[derive(Serialize)]
    struct ClaimPrompt<'a> {
        id: &'a str,
        claim: &'a str,
        source_url: &'a str,
    }

    let claims: Vec<_> = claims
        .iter()
        .map(|claim| ClaimPrompt {
            id: &claim.id,
            claim: &claim.claim.claim,
            source_url: &claim.source_url,
        })
        .collect();
    serde_json::to_string(&claims).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::trim_for_prompt;

    #[test]
    fn trim_for_prompt_handles_empty_short_and_unicode_text() {
        assert_eq!(trim_for_prompt("", 8), "");
        assert_eq!(trim_for_prompt("short", 8), "short");
        assert_eq!(trim_for_prompt("研究工作流设计", 4), "研究工作...");
    }
}
