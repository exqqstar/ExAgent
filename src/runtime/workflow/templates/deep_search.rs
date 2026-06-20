use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::runtime::workflow::agent_runner::{
    parse_json_object, AgentJsonRequest, AgentJsonRunner,
};
use crate::runtime::workflow::types::DeepSearchLimits;
use crate::runtime::workflow::{
    NoopWorkflowProgressSink, WorkflowCancellation, WorkflowProgressSink,
};

use super::deep_search_prompts::{
    extract_prompt, scope_prompt, search_prompt, synthesize_prompt, verify_prompt,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SearchAngle {
    pub label: String,
    pub query: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ScopeOutput {
    pub question: String,
    pub summary: String,
    pub angles: Vec<SearchAngle>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchRelevance {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SearchResultItem {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub relevance: SearchRelevance,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SearchOutput {
    pub results: Vec<SearchResultItem>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimImportance {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExtractedClaim {
    pub claim: String,
    pub quote: String,
    pub importance: ClaimImportance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceQuality {
    Primary,
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SourceExtractOutput {
    pub source_url: String,
    pub source_title: String,
    pub source_quality: SourceQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_date: Option<String>,
    pub claims: Vec<ExtractedClaim>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClaimVerdict {
    pub refuted: bool,
    pub evidence: String,
    pub confidence: VerdictConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeepSearchReport {
    pub summary: String,
    pub findings: Vec<String>,
    pub caveats: Vec<String>,
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeepSearchStats {
    pub sources_fetched: usize,
    pub claims_extracted: usize,
    pub claims_verified: usize,
    pub survived: usize,
    pub killed: usize,
    pub url_dupes: usize,
    pub budget_dropped: usize,
    pub approval_blocked: usize,
}

impl DeepSearchStats {
    pub fn to_template_stats(&self) -> Value {
        json!({
            "sources_fetched": self.sources_fetched,
            "claims_extracted": self.claims_extracted,
            "claims_verified": self.claims_verified,
            "survived": self.survived,
            "killed": self.killed,
            "url_dupes": self.url_dupes,
            "budget_dropped": self.budget_dropped,
            "approval_blocked": self.approval_blocked,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeepSearchPhaseCounts {
    pub scoped_angles: usize,
    pub searched_angles: usize,
    pub search_results: usize,
    pub sources_selected: usize,
    pub sources_extracted: usize,
    pub claims_ranked: usize,
    pub verdict_votes: usize,
    pub synthesized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeepSearchRunResult {
    pub report: DeepSearchReport,
    pub stats: DeepSearchStats,
    pub selected_sources: Vec<SelectedSource>,
    pub ranked_claims: Vec<RankedClaim>,
    pub aggregated_verdicts: AggregatedVerdicts,
    pub phase_counts: DeepSearchPhaseCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSearchPhaseId {
    Scope,
    Search,
    Extract,
    Verify,
    Synthesize,
}

impl DeepSearchPhaseId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Search => "search",
            Self::Extract => "extract",
            Self::Verify => "verify",
            Self::Synthesize => "synthesize",
        }
    }
}

#[derive(Debug)]
pub struct DeepSearchPhaseError {
    phase_id: DeepSearchPhaseId,
    context: String,
    source: anyhow::Error,
}

impl DeepSearchPhaseError {
    pub fn new(
        phase_id: DeepSearchPhaseId,
        context: impl Into<String>,
        source: anyhow::Error,
    ) -> Self {
        Self {
            phase_id,
            context: context.into(),
            source,
        }
    }

    pub fn phase_id(&self) -> DeepSearchPhaseId {
        self.phase_id
    }
}

impl fmt::Display for DeepSearchPhaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deep search {} phase failed{}: {}",
            self.phase_id.as_str(),
            self.context,
            self.source
        )
    }
}

impl Error for DeepSearchPhaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeepSearchCancelled {
    phase_id: DeepSearchPhaseId,
}

impl DeepSearchCancelled {
    pub fn new(phase_id: DeepSearchPhaseId) -> Self {
        Self { phase_id }
    }

    pub fn phase_id(&self) -> DeepSearchPhaseId {
        self.phase_id
    }
}

impl fmt::Display for DeepSearchCancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deep search cancelled before scheduling {} work",
            self.phase_id.as_str()
        )
    }
}

impl Error for DeepSearchCancelled {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SelectedSource {
    pub angle_label: String,
    pub normalized_url: String,
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub relevance: SearchRelevance,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DedupeSearchResults {
    pub sources: Vec<SelectedSource>,
    pub url_dupes: usize,
    pub budget_dropped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RankedClaim {
    pub id: String,
    pub source_url: String,
    pub source_title: String,
    pub source_quality: SourceQuality,
    pub claim: ExtractedClaim,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClaimVoteSummary {
    pub claim: RankedClaim,
    pub refuted_votes: usize,
    pub non_refuting_or_weak_votes: usize,
    pub total_votes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AggregatedVerdicts {
    pub survived: Vec<ClaimVoteSummary>,
    pub killed: Vec<ClaimVoteSummary>,
    pub unverified: Vec<ClaimVoteSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AngledSearchResults<'a> {
    pub angle_label: &'a str,
    pub results: &'a [SearchResultItem],
}

#[derive(Clone)]
pub struct DeepSearchTemplateRunner {
    limits: DeepSearchLimits,
    runner: Arc<dyn AgentJsonRunner>,
    progress_sink: Arc<dyn WorkflowProgressSink>,
    cancellation: WorkflowCancellation,
    run_label: String,
}

impl DeepSearchTemplateRunner {
    pub fn new(limits: DeepSearchLimits, runner: Arc<dyn AgentJsonRunner>) -> Self {
        Self {
            limits,
            runner,
            progress_sink: Arc::new(NoopWorkflowProgressSink),
            cancellation: WorkflowCancellation::new(),
            run_label: "deep-search".to_string(),
        }
    }

    pub fn with_run_label(mut self, run_label: impl Into<String>) -> Self {
        let run_label = run_label.into();
        if !run_label.trim().is_empty() {
            self.run_label = run_label;
        }
        self
    }

    pub fn with_progress_sink(mut self, progress_sink: Arc<dyn WorkflowProgressSink>) -> Self {
        self.progress_sink = progress_sink;
        self
    }

    pub fn with_cancellation(mut self, cancellation: WorkflowCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub async fn run(&self, question: &str) -> anyhow::Result<DeepSearchRunResult> {
        let mut stats = DeepSearchStats::default();
        let mut phase_counts = DeepSearchPhaseCounts::default();

        self.declare_progress_phases().await;
        self.ensure_not_cancelled(DeepSearchPhaseId::Scope)?;
        self.progress_sink.start_phase("scope", "Scope", 1).await;
        let scope: ScopeOutput = self
            .run_agent("scope", scope_prompt(question))
            .await
            .map_err(|error| DeepSearchPhaseError::new(DeepSearchPhaseId::Scope, "", error))?;
        phase_counts.scoped_angles = scope.angles.len();
        self.progress_sink
            .update_phase_counts("scope", 1, 0, 0)
            .await;
        self.progress_sink.complete_phase("scope").await;

        let scoped_angles: Vec<_> = scope
            .angles
            .iter()
            .take(self.limits.max_angles)
            .cloned()
            .collect();

        self.ensure_not_cancelled(DeepSearchPhaseId::Search)?;
        if scoped_angles.is_empty() {
            self.progress_sink
                .update_phase_counts("search", 0, 0, self.limits.max_angles)
                .await;
            self.progress_sink.skip_phase("search").await;
        } else {
            self.progress_sink
                .start_phase("search", "Search", scoped_angles.len())
                .await;
        }
        let mut search_outputs = Vec::new();
        for (angle_index, angle) in scoped_angles.iter().enumerate() {
            self.ensure_not_cancelled(DeepSearchPhaseId::Search)?;
            let output: SearchOutput = self
                .run_agent(
                    &format!("search-{number:04}", number = angle_index + 1),
                    search_prompt(question, angle),
                )
                .await
                .map_err(|error| {
                    DeepSearchPhaseError::new(
                        DeepSearchPhaseId::Search,
                        format!(" for {}", angle.label),
                        error,
                    )
                })?;
            phase_counts.searched_angles += 1;
            phase_counts.search_results += output.results.len();
            self.progress_sink
                .update_phase_counts("search", phase_counts.searched_angles, 0, 0)
                .await;
            search_outputs.push((angle.label.clone(), output));
        }
        if phase_counts.searched_angles > 0 {
            self.progress_sink.complete_phase("search").await;
        }

        let groups: Vec<_> = search_outputs
            .iter()
            .map(|(angle_label, output)| AngledSearchResults {
                angle_label,
                results: &output.results,
            })
            .collect();
        let deduped = dedupe_search_results_by_angle(&groups, self.limits.max_sources);
        stats.url_dupes = deduped.url_dupes;
        stats.budget_dropped = deduped.budget_dropped;
        stats.sources_fetched = deduped.sources.len();
        phase_counts.sources_selected = deduped.sources.len();

        self.ensure_not_cancelled(DeepSearchPhaseId::Extract)?;
        if deduped.sources.is_empty() {
            self.progress_sink
                .update_phase_counts("extract", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("extract").await;
        } else {
            self.progress_sink
                .start_phase("extract", "Extract", deduped.sources.len())
                .await;
        }
        let mut extracted_claims = Vec::new();
        for (source_index, source) in deduped.sources.iter().enumerate() {
            self.ensure_not_cancelled(DeepSearchPhaseId::Extract)?;
            let source_extract: SourceExtractOutput = self
                .run_agent(
                    &format!("extract-{number:04}", number = source_index + 1),
                    extract_prompt(question, source, &source.snippet),
                )
                .await
                .map_err(|error| {
                    DeepSearchPhaseError::new(
                        DeepSearchPhaseId::Extract,
                        format!(" for {}", source.url),
                        error,
                    )
                })?;
            phase_counts.sources_extracted += 1;
            self.progress_sink
                .update_phase_counts("extract", phase_counts.sources_extracted, 0, 0)
                .await;

            let source_url = non_empty_or(&source_extract.source_url, &source.url);
            let source_title = non_empty_or(&source_extract.source_title, &source.title);
            for claim in source_extract.claims {
                let id = format!("claim-{number:04}", number = extracted_claims.len() + 1);
                extracted_claims.push(RankedClaim {
                    id,
                    source_url: source_url.clone(),
                    source_title: source_title.clone(),
                    source_quality: source_extract.source_quality,
                    claim,
                });
            }
        }
        if phase_counts.sources_extracted > 0 {
            self.progress_sink.complete_phase("extract").await;
        }
        stats.claims_extracted = extracted_claims.len();

        let ranked_claims = rank_claims(extracted_claims, self.limits.max_claims);
        stats.budget_dropped += stats.claims_extracted.saturating_sub(ranked_claims.len());
        phase_counts.claims_ranked = ranked_claims.len();

        self.ensure_not_cancelled(DeepSearchPhaseId::Verify)?;
        if ranked_claims.is_empty() {
            self.progress_sink
                .update_phase_counts("verify", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("verify").await;
            self.progress_sink
                .update_phase_counts("synthesize", 0, 0, 1)
                .await;
            self.progress_sink.skip_phase("synthesize").await;
            let report = no_claims_report(question, &stats);
            return Ok(DeepSearchRunResult {
                report,
                stats,
                selected_sources: deduped.sources,
                ranked_claims,
                aggregated_verdicts: AggregatedVerdicts::default(),
                phase_counts,
            });
        }

        let planned_verdict_votes = ranked_claims.len() * self.limits.votes_per_claim;
        if planned_verdict_votes == 0 {
            self.progress_sink
                .update_phase_counts("verify", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("verify").await;
        } else {
            self.progress_sink
                .start_phase("verify", "Verify", planned_verdict_votes)
                .await;
        }
        let mut verdicts_by_claim_id: BTreeMap<String, Vec<ClaimVerdict>> = BTreeMap::new();
        for (claim_index, claim) in ranked_claims.iter().enumerate() {
            for vote_index in 0..self.limits.votes_per_claim {
                self.ensure_not_cancelled(DeepSearchPhaseId::Verify)?;
                let verdict: ClaimVerdict = self
                    .run_agent(
                        &format!(
                            "verify-{claim_number:04}-vote-{vote_number:04}",
                            claim_number = claim_index + 1,
                            vote_number = vote_index + 1
                        ),
                        verify_prompt(question, claim, &verification_evidence_text(claim)),
                    )
                    .await
                    .map_err(|error| {
                        DeepSearchPhaseError::new(
                            DeepSearchPhaseId::Verify,
                            format!(" for {}", claim.id),
                            error,
                        )
                    })?;
                phase_counts.verdict_votes += 1;
                self.progress_sink
                    .update_phase_counts("verify", phase_counts.verdict_votes, 0, 0)
                    .await;
                verdicts_by_claim_id
                    .entry(claim.id.clone())
                    .or_default()
                    .push(verdict);
            }
        }
        if phase_counts.verdict_votes > 0 {
            self.progress_sink.complete_phase("verify").await;
        }

        stats.claims_verified = verdicts_by_claim_id
            .values()
            .filter(|verdicts| !verdicts.is_empty())
            .count();
        let aggregated_verdicts = aggregate_verdicts(
            &ranked_claims,
            &verdicts_by_claim_id,
            self.limits.refutations_required,
        );
        stats.survived = aggregated_verdicts.survived.len();
        stats.killed = aggregated_verdicts.killed.len();

        self.ensure_not_cancelled(DeepSearchPhaseId::Synthesize)?;
        if !ranked_claims.is_empty()
            && aggregated_verdicts.survived.is_empty()
            && aggregated_verdicts.killed.len() == ranked_claims.len()
        {
            self.progress_sink
                .update_phase_counts("synthesize", 0, 0, 1)
                .await;
            self.progress_sink.skip_phase("synthesize").await;
            let report = all_refuted_report(question, &aggregated_verdicts.killed, &stats);
            return Ok(DeepSearchRunResult {
                report,
                stats,
                selected_sources: deduped.sources,
                ranked_claims,
                aggregated_verdicts,
                phase_counts,
            });
        }

        let survived_claims = summaries_to_claims(&aggregated_verdicts.survived);
        let killed_claims = summaries_to_claims(&aggregated_verdicts.killed);
        self.progress_sink
            .start_phase("synthesize", "Synthesize", 1)
            .await;
        let synthesis = self
            .run_agent::<DeepSearchReport>(
                "synthesize",
                synthesize_prompt(question, &survived_claims, &killed_claims),
            )
            .await;

        let report = match synthesis {
            Ok(report) => {
                phase_counts.synthesized = true;
                self.progress_sink
                    .update_phase_counts("synthesize", 1, 0, 0)
                    .await;
                self.progress_sink.complete_phase("synthesize").await;
                report
            }
            Err(error) => {
                self.progress_sink
                    .update_phase_counts("synthesize", 0, 0, 1)
                    .await;
                self.progress_sink.skip_phase("synthesize").await;
                synthesis_failed_report(
                    question,
                    &aggregated_verdicts.survived,
                    &aggregated_verdicts.killed,
                    &stats,
                    &error.to_string(),
                )
            }
        };

        self.ensure_not_cancelled(DeepSearchPhaseId::Synthesize)?;
        Ok(DeepSearchRunResult {
            report,
            stats,
            selected_sources: deduped.sources,
            ranked_claims,
            aggregated_verdicts,
            phase_counts,
        })
    }

    async fn declare_progress_phases(&self) {
        self.progress_sink.declare_phase("scope", "Scope", 1).await;
        self.progress_sink
            .declare_phase("search", "Search", self.limits.max_angles)
            .await;
        self.progress_sink
            .declare_phase("extract", "Extract", self.limits.max_sources)
            .await;
        self.progress_sink
            .declare_phase(
                "verify",
                "Verify",
                self.limits.max_claims * self.limits.votes_per_claim,
            )
            .await;
        self.progress_sink
            .declare_phase("synthesize", "Synthesize", 1)
            .await;
    }

    fn ensure_not_cancelled(&self, phase_id: DeepSearchPhaseId) -> anyhow::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(DeepSearchCancelled::new(phase_id).into());
        }
        Ok(())
    }

    async fn run_agent<T>(&self, label: &str, prompt: String) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .runner
            .run_json(AgentJsonRequest {
                label: format!("{}:{label}", self.run_label),
                prompt,
                schema_hint: Some(json!({"type": "object"})),
            })
            .await?;
        if response.value.is_object() {
            if let Ok(parsed) = serde_json::from_value(response.value.clone()) {
                return Ok(parsed);
            }
        }
        parse_json_object(&response.text)
    }
}

pub async fn run_deep_search(
    question: &str,
    limits: DeepSearchLimits,
    runner: Arc<dyn AgentJsonRunner>,
) -> anyhow::Result<DeepSearchRunResult> {
    DeepSearchTemplateRunner::new(limits, runner)
        .run(question)
        .await
}

pub fn normalize_source_url(source: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(source).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    let host = url.host_str()?.to_ascii_lowercase();
    let normalized_host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    url.set_host(Some(&normalized_host)).ok()?;
    url.set_query(None);
    url.set_fragment(None);

    let path = url.path().trim_end_matches('/').to_string();
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path
    };
    url.set_path(&path);

    Some(url.to_string().trim_end_matches('/').to_string())
}

pub fn dedupe_search_results(
    angle_label: &str,
    results: &[SearchResultItem],
    max_sources: usize,
) -> DedupeSearchResults {
    dedupe_search_results_by_angle(
        &[AngledSearchResults {
            angle_label,
            results,
        }],
        max_sources,
    )
}

pub fn dedupe_search_results_by_angle(
    groups: &[AngledSearchResults<'_>],
    max_sources: usize,
) -> DedupeSearchResults {
    let mut seen = BTreeSet::new();
    let mut output = DedupeSearchResults::default();

    for group in groups {
        for result in group.results {
            let Some(normalized_url) = normalize_source_url(&result.url) else {
                continue;
            };

            if !seen.insert(normalized_url.clone()) {
                output.url_dupes += 1;
                continue;
            }

            if output.sources.len() >= max_sources {
                output.budget_dropped += 1;
                continue;
            }

            output.sources.push(SelectedSource {
                angle_label: group.angle_label.to_string(),
                normalized_url,
                url: result.url.clone(),
                title: result.title.clone(),
                snippet: result.snippet.clone(),
                relevance: result.relevance,
            });
        }
    }

    output
}

pub fn rank_claims(mut claims: Vec<RankedClaim>, max_claims: usize) -> Vec<RankedClaim> {
    claims.sort_by_key(|claim| {
        (
            Reverse(claim_rank_key(claim)),
            claim.id.clone(),
            claim.source_url.clone(),
        )
    });
    claims.truncate(max_claims);
    claims
}

pub fn aggregate_verdicts(
    claims: &[RankedClaim],
    verdicts_by_claim_id: &BTreeMap<String, Vec<ClaimVerdict>>,
    refutations_required: usize,
) -> AggregatedVerdicts {
    let threshold = refutations_required.max(1);
    let mut aggregated = AggregatedVerdicts::default();

    for claim in claims {
        let verdicts = verdicts_by_claim_id
            .get(&claim.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let refuted_votes = verdicts
            .iter()
            .filter(|verdict| verdict.refuted && verdict.confidence != VerdictConfidence::Low)
            .count();
        let non_refuting_or_weak_votes = verdicts
            .iter()
            .filter(|verdict| !verdict.refuted || verdict.confidence == VerdictConfidence::Low)
            .count();

        let summary = ClaimVoteSummary {
            claim: claim.clone(),
            refuted_votes,
            non_refuting_or_weak_votes,
            total_votes: verdicts.len(),
        };

        if refuted_votes >= threshold {
            aggregated.killed.push(summary);
        } else if non_refuting_or_weak_votes > 0 {
            aggregated.survived.push(summary);
        } else {
            aggregated.unverified.push(summary);
        }
    }

    aggregated
}

pub fn no_claims_report(question: &str, stats: &DeepSearchStats) -> DeepSearchReport {
    DeepSearchReport {
        summary: format!("No verifiable claims were extracted for: {question}"),
        findings: Vec::new(),
        caveats: vec![format!(
            "Deep search extracted {} claims from {} fetched sources.",
            stats.claims_extracted, stats.sources_fetched
        )],
        open_questions: stats_open_questions(stats),
    }
}

pub fn all_refuted_report(
    question: &str,
    killed: &[ClaimVoteSummary],
    stats: &DeepSearchStats,
) -> DeepSearchReport {
    DeepSearchReport {
        summary: format!("All candidate claims were refuted for: {question}"),
        findings: killed
            .iter()
            .map(|summary| {
                format!(
                    "Refuted: {} ({} refuting votes)",
                    summary.claim.claim.claim, summary.refuted_votes
                )
            })
            .collect(),
        caveats: vec![format!(
            "No claims survived verification; {} of {} verified claims were killed.",
            stats.killed, stats.claims_verified
        )],
        open_questions: stats_open_questions(stats),
    }
}

pub fn synthesis_failed_report(
    question: &str,
    survived: &[ClaimVoteSummary],
    killed: &[ClaimVoteSummary],
    stats: &DeepSearchStats,
    error: &str,
) -> DeepSearchReport {
    DeepSearchReport {
        summary: format!("Synthesis failed for: {question}"),
        findings: survived
            .iter()
            .map(|summary| summary.claim.claim.claim.clone())
            .collect(),
        caveats: vec![format!(
            "Report synthesis failed after {} survived and {} refuted claims: {error}",
            survived.len().max(stats.survived),
            killed.len().max(stats.killed)
        )],
        open_questions: stats_open_questions(stats),
    }
}

fn claim_rank_key(claim: &RankedClaim) -> (u8, u8) {
    (
        importance_rank(claim.claim.importance),
        source_quality_rank(claim.source_quality),
    )
}

fn importance_rank(importance: ClaimImportance) -> u8 {
    match importance {
        ClaimImportance::Critical => 4,
        ClaimImportance::High => 3,
        ClaimImportance::Medium => 2,
        ClaimImportance::Low => 1,
    }
}

fn source_quality_rank(source_quality: SourceQuality) -> u8 {
    match source_quality {
        SourceQuality::Primary => 5,
        SourceQuality::High => 4,
        SourceQuality::Medium => 3,
        SourceQuality::Low => 2,
        SourceQuality::Unknown => 1,
    }
}

fn stats_open_questions(stats: &DeepSearchStats) -> Vec<String> {
    let mut questions = Vec::new();
    if stats.budget_dropped > 0 {
        questions.push(format!(
            "Review additional omitted candidates; budget dropped {} items across source and claim limits.",
            stats.budget_dropped
        ));
    }
    if stats.approval_blocked > 0 {
        questions.push(format!(
            "Resolve approval blockers for {} skipped actions.",
            stats.approval_blocked
        ));
    }
    if questions.is_empty() {
        questions.push(
            "Run another pass with broader source coverage if the answer needs higher confidence."
                .to_string(),
        );
    }
    questions
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn verification_evidence_text(claim: &RankedClaim) -> String {
    format!(
        "Original quote: {}\nSource title: {}\nSource URL: {}",
        claim.claim.quote, claim.source_title, claim.source_url
    )
}

fn summaries_to_claims(summaries: &[ClaimVoteSummary]) -> Vec<RankedClaim> {
    summaries
        .iter()
        .map(|summary| summary.claim.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use crate::runtime::workflow::agent_runner::{
        AgentJsonRequest, AgentJsonResponse, AgentJsonRunner,
    };
    use crate::runtime::workflow::progress::{
        RecordingWorkflowProgressSink, WorkflowProgressEvent,
    };
    use crate::runtime::workflow::types::DeepSearchLimits;
    use crate::runtime::workflow::WorkflowCancellation;

    fn result(url: &str, title: &str) -> SearchResultItem {
        SearchResultItem {
            url: url.to_string(),
            title: title.to_string(),
            snippet: "snippet".to_string(),
            relevance: SearchRelevance::High,
        }
    }

    #[tokio::test]
    async fn template_runner_happy_path_synthesizes_report_and_stats() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![
                SearchAngle {
                    label: "history".to_string(),
                    query: "history query".to_string(),
                    rationale: "why".to_string(),
                },
                SearchAngle {
                    label: "ignored".to_string(),
                    query: "ignored query".to_string(),
                    rationale: "over cap".to_string(),
                },
            ])),
            json_text(&SearchOutput {
                results: vec![
                    result("https://example.com/a", "A"),
                    result("https://example.com/b", "B"),
                ],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/a".to_string(),
                source_title: "A".to_string(),
                source_quality: SourceQuality::Primary,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Claim A".to_string(),
                    quote: "Quote A".to_string(),
                    importance: ClaimImportance::Critical,
                }],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/b".to_string(),
                source_title: "B".to_string(),
                source_quality: SourceQuality::High,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Claim B".to_string(),
                    quote: "Quote B".to_string(),
                    importance: ClaimImportance::High,
                }],
            }),
            json_text(&verdict(false, VerdictConfidence::High)),
            json_text(&verdict(false, VerdictConfidence::Medium)),
            json_text(&verdict(true, VerdictConfidence::High)),
            json_text(&verdict(true, VerdictConfidence::Medium)),
            json_text(&DeepSearchReport {
                summary: "Synthesized answer".to_string(),
                findings: vec!["Finding".to_string()],
                caveats: vec!["Caveat".to_string()],
                open_questions: Vec::new(),
            }),
        ]));
        let limits = limits(1, 2, 2, 2, 2);

        let result = DeepSearchTemplateRunner::new(limits, runner.clone())
            .with_run_label("unit")
            .run("What changed?")
            .await
            .expect("run deep search");

        assert_eq!(result.report.summary, "Synthesized answer");
        assert_eq!(result.stats.sources_fetched, 2);
        assert_eq!(result.stats.claims_extracted, 2);
        assert_eq!(result.stats.claims_verified, 2);
        assert_eq!(result.stats.survived, 1);
        assert_eq!(result.stats.to_template_stats()["survived"], json!(1));
        assert!(result.stats.to_template_stats().get("confirmed").is_none());
        assert_eq!(result.stats.killed, 1);
        assert_eq!(result.selected_sources.len(), 2);
        assert_eq!(result.ranked_claims.len(), 2);
        assert_eq!(result.phase_counts.scoped_angles, 2);
        assert_eq!(result.phase_counts.searched_angles, 1);
        assert_eq!(result.phase_counts.search_results, 2);
        assert_eq!(result.phase_counts.sources_selected, 2);
        assert_eq!(result.phase_counts.sources_extracted, 2);
        assert_eq!(result.phase_counts.claims_ranked, 2);
        assert_eq!(result.phase_counts.verdict_votes, 4);
        assert!(result.phase_counts.synthesized);
        assert_eq!(
            result.aggregated_verdicts.survived[0].claim.claim.claim,
            "Claim A"
        );
        assert_eq!(
            result.aggregated_verdicts.killed[0].claim.claim.claim,
            "Claim B"
        );

        let labels = runner.labels();
        assert_eq!(
            labels,
            vec![
                "unit:scope",
                "unit:search-0001",
                "unit:extract-0001",
                "unit:extract-0002",
                "unit:verify-0001-vote-0001",
                "unit:verify-0001-vote-0002",
                "unit:verify-0002-vote-0001",
                "unit:verify-0002-vote-0002",
                "unit:synthesize",
            ]
        );
    }

    #[tokio::test]
    async fn template_runner_prefers_structured_json_value_before_text_fallback() {
        let scoped = scope(Vec::new());
        let runner = Arc::new(MockJsonRunner::new_agent_responses(vec![
            AgentJsonResponse {
                text: "not json".to_string(),
                value: serde_json::to_value(&scoped).expect("serialize scope"),
                tokens_used: None,
            },
        ]));

        let parsed: ScopeOutput = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner)
            .run_agent("scope", "ignored".to_string())
            .await
            .expect("parse structured value");

        assert_eq!(parsed, scoped);
    }

    #[tokio::test]
    async fn template_runner_tags_search_failure_with_typed_phase_despite_extract_word_in_label() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "extract phase bait".to_string(),
                query: "search query".to_string(),
                rationale: "why".to_string(),
            }])),
            "not json".to_string(),
        ]));

        let error = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner)
            .run("What changed?")
            .await
            .expect_err("invalid search output should fail");

        let phase_error = error
            .downcast_ref::<DeepSearchPhaseError>()
            .expect("template failure should carry typed phase error");
        assert_eq!(phase_error.phase_id(), DeepSearchPhaseId::Search);
        assert_eq!(phase_error.phase_id().as_str(), "search");
        assert!(error
            .to_string()
            .contains("deep search search phase failed"));
        assert!(error.to_string().contains("extract phase bait"));
    }

    #[tokio::test]
    async fn template_runner_returns_no_claims_fallback_without_synthesis() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "current".to_string(),
                query: "current query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SearchOutput {
                results: vec![result("https://example.com/source", "Source")],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
        ]));

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 4, 2, 2), runner.clone())
            .run("What changed?")
            .await
            .expect("run deep search");

        assert!(result.report.summary.contains("No verifiable claims"));
        assert_eq!(result.stats.sources_fetched, 1);
        assert_eq!(result.stats.claims_extracted, 0);
        assert_eq!(result.stats.claims_verified, 0);
        assert_eq!(result.phase_counts.verdict_votes, 0);
        assert!(!result.phase_counts.synthesized);
        assert!(!runner
            .labels()
            .iter()
            .any(|label| label.contains("synthesize")));
    }

    #[tokio::test]
    async fn template_runner_reports_progress_and_skips_later_phases_on_no_claims() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "current".to_string(),
                query: "current query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SearchOutput {
                results: vec![result("https://example.com/source", "Source")],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
        ]));
        let progress = RecordingWorkflowProgressSink::new();

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 4, 2, 2), runner)
            .with_progress_sink(Arc::new(progress.clone()))
            .run("What changed?")
            .await
            .expect("run deep search");

        assert!(result.report.summary.contains("No verifiable claims"));
        assert_eq!(
            progress.events().await,
            vec![
                WorkflowProgressEvent::Declared {
                    id: "scope".to_string(),
                    label: "Scope".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Declared {
                    id: "search".to_string(),
                    label: "Search".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Declared {
                    id: "extract".to_string(),
                    label: "Extract".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Declared {
                    id: "verify".to_string(),
                    label: "Verify".to_string(),
                    planned_count: 8,
                },
                WorkflowProgressEvent::Declared {
                    id: "synthesize".to_string(),
                    label: "Synthesize".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Started {
                    id: "scope".to_string(),
                    label: "Scope".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "scope".to_string(),
                    completed_count: 1,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Completed {
                    id: "scope".to_string(),
                },
                WorkflowProgressEvent::Started {
                    id: "search".to_string(),
                    label: "Search".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "search".to_string(),
                    completed_count: 1,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Completed {
                    id: "search".to_string(),
                },
                WorkflowProgressEvent::Started {
                    id: "extract".to_string(),
                    label: "Extract".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "extract".to_string(),
                    completed_count: 1,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Completed {
                    id: "extract".to_string(),
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "verify".to_string(),
                    completed_count: 0,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Skipped {
                    id: "verify".to_string(),
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "synthesize".to_string(),
                    completed_count: 0,
                    failed_count: 0,
                    skipped_count: 1,
                },
                WorkflowProgressEvent::Skipped {
                    id: "synthesize".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn template_runner_returns_all_refuted_fallback_without_synthesis() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "audit".to_string(),
                query: "audit query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SearchOutput {
                results: vec![result("https://example.com/source", "Source")],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::High,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Refuted claim".to_string(),
                    quote: "Quote".to_string(),
                    importance: ClaimImportance::High,
                }],
            }),
            json_text(&verdict(true, VerdictConfidence::High)),
        ]));

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner.clone())
            .run("What changed?")
            .await
            .expect("run deep search");

        assert!(result
            .report
            .summary
            .contains("All candidate claims were refuted"));
        assert_eq!(result.stats.claims_verified, 1);
        assert_eq!(result.stats.survived, 0);
        assert_eq!(result.stats.killed, 1);
        assert_eq!(result.phase_counts.verdict_votes, 1);
        assert!(!result.phase_counts.synthesized);
        assert!(!runner
            .labels()
            .iter()
            .any(|label| label.contains("synthesize")));
    }

    #[tokio::test]
    async fn template_runner_returns_synthesis_failed_fallback_on_parse_failure() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "audit".to_string(),
                query: "audit query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SearchOutput {
                results: vec![result("https://example.com/source", "Source")],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::High,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Surviving claim".to_string(),
                    quote: "Quote".to_string(),
                    importance: ClaimImportance::High,
                }],
            }),
            json_text(&verdict(false, VerdictConfidence::High)),
            "not json".to_string(),
        ]));

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner.clone())
            .run("What changed?")
            .await
            .expect("run deep search");

        assert!(result.report.summary.contains("Synthesis failed"));
        assert!(result
            .report
            .findings
            .contains(&"Surviving claim".to_string()));
        assert_eq!(result.stats.survived, 1);
        assert_eq!(result.phase_counts.verdict_votes, 1);
        assert!(!result.phase_counts.synthesized);
        assert_eq!(
            runner.labels().last().map(String::as_str),
            Some("deep-search:synthesize")
        );
    }

    #[tokio::test]
    async fn template_runner_dedupes_globally_and_limits_source_extraction() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![
                SearchAngle {
                    label: "first".to_string(),
                    query: "first query".to_string(),
                    rationale: "why".to_string(),
                },
                SearchAngle {
                    label: "second".to_string(),
                    query: "second query".to_string(),
                    rationale: "why".to_string(),
                },
            ])),
            json_text(&SearchOutput {
                results: vec![
                    result("https://www.example.com/a/?utm=1", "A"),
                    result("https://example.com/b", "B"),
                ],
            }),
            json_text(&SearchOutput {
                results: vec![
                    result("https://example.com/a", "A duplicate"),
                    result("https://example.com/c", "C"),
                    result("https://example.com/d", "D"),
                ],
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/a".to_string(),
                source_title: "A".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/b".to_string(),
                source_title: "B".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
        ]));

        let result = DeepSearchTemplateRunner::new(limits(2, 2, 4, 2, 2), runner.clone())
            .run("What changed?")
            .await
            .expect("run deep search");

        assert_eq!(
            result
                .selected_sources
                .iter()
                .map(|source| source.url.as_str())
                .collect::<Vec<_>>(),
            vec!["https://www.example.com/a/?utm=1", "https://example.com/b"]
        );
        assert_eq!(result.stats.url_dupes, 1);
        assert_eq!(result.stats.budget_dropped, 2);
        assert_eq!(
            runner
                .labels()
                .iter()
                .filter(|label| label.contains(":extract-"))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn template_runner_cancellation_after_search_prevents_later_agents() {
        let cancellation = WorkflowCancellation::new();
        let runner = Arc::new(CancellingJsonRunner::new(
            cancellation.clone(),
            "unit:search-0001",
            vec![
                json_text(&scope(vec![
                    SearchAngle {
                        label: "first".to_string(),
                        query: "first query".to_string(),
                        rationale: "why".to_string(),
                    },
                    SearchAngle {
                        label: "second".to_string(),
                        query: "second query".to_string(),
                        rationale: "why".to_string(),
                    },
                ])),
                json_text(&SearchOutput {
                    results: vec![result("https://example.com/a", "A")],
                }),
                json_text(&SearchOutput {
                    results: vec![result("https://example.com/b", "B")],
                }),
                json_text(&SourceExtractOutput {
                    source_url: "https://example.com/a".to_string(),
                    source_title: "A".to_string(),
                    source_quality: SourceQuality::Medium,
                    publish_date: None,
                    claims: Vec::new(),
                }),
            ],
        ));

        let error = DeepSearchTemplateRunner::new(limits(2, 2, 4, 2, 2), runner.clone())
            .with_run_label("unit")
            .with_cancellation(cancellation)
            .run("What changed?")
            .await
            .expect_err("cancellation should stop template execution");

        let cancelled = error
            .downcast_ref::<DeepSearchCancelled>()
            .expect("template should return a cancellation error");
        assert_eq!(cancelled.phase_id(), DeepSearchPhaseId::Search);
        assert_eq!(runner.labels(), vec!["unit:scope", "unit:search-0001"]);
    }

    #[test]
    fn normalize_source_url_dedupes_www_and_trailing_slash() {
        assert_eq!(
            normalize_source_url("https://www.example.com/a/"),
            Some("https://example.com/a".to_string())
        );
        assert_eq!(
            normalize_source_url("https://example.com/a"),
            Some("https://example.com/a".to_string())
        );
    }

    #[test]
    fn normalize_source_url_keeps_different_paths_distinct() {
        assert_ne!(
            normalize_source_url("https://example.com/a"),
            normalize_source_url("https://example.com/b")
        );
    }

    #[test]
    fn normalize_source_url_strips_query_and_fragment_for_source_dedupe() {
        assert_eq!(
            normalize_source_url("https://example.com/a?utm_source=x#section"),
            Some("https://example.com/a".to_string())
        );
    }

    #[test]
    fn normalize_source_url_documents_case_root_and_port_behavior() {
        assert_eq!(
            normalize_source_url("HTTP://WWW.Example.COM/"),
            Some("http://example.com".to_string())
        );
        assert_eq!(
            normalize_source_url("https://example.com"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            normalize_source_url("https://example.com/"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            normalize_source_url("https://example.com:443/"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            normalize_source_url("https://example.com:8443/"),
            Some("https://example.com:8443".to_string())
        );
        assert_ne!(
            normalize_source_url("https://example.com/A"),
            normalize_source_url("https://example.com/a")
        );
    }

    #[test]
    fn normalize_source_url_rejects_invalid_and_non_http_urls() {
        assert_eq!(normalize_source_url("not a url"), None);
        assert_eq!(normalize_source_url("file:///tmp/source.txt"), None);
    }

    #[test]
    fn dedupe_search_results_counts_duplicates_and_budget_dropped() {
        let output = dedupe_search_results(
            "history",
            &[
                result("https://www.example.com/a/", "A"),
                result("https://example.com/a?ref=1", "A duplicate"),
                result("https://example.com/b", "B"),
                result("https://example.com/c", "C"),
            ],
            2,
        );

        assert_eq!(output.sources.len(), 2);
        assert_eq!(output.sources[0].angle_label, "history");
        assert_eq!(output.url_dupes, 1);
        assert_eq!(output.budget_dropped, 1);
    }

    #[test]
    fn dedupe_search_results_by_angle_counts_cross_angle_dupes_globally() {
        let first = vec![
            result("https://example.com/a", "A"),
            result("https://example.com/b", "B"),
        ];
        let second = vec![
            result("https://www.example.com/a/?utm=1", "A again"),
            result("https://example.com/c", "C"),
        ];

        let output = dedupe_search_results_by_angle(
            &[
                AngledSearchResults {
                    angle_label: "history",
                    results: &first,
                },
                AngledSearchResults {
                    angle_label: "current",
                    results: &second,
                },
            ],
            2,
        );

        assert_eq!(output.sources.len(), 2);
        assert_eq!(output.sources[0].angle_label, "history");
        assert_eq!(output.sources[1].angle_label, "history");
        assert_eq!(output.url_dupes, 1);
        assert_eq!(output.budget_dropped, 1);
    }

    #[test]
    fn rank_claims_orders_by_importance_then_source_quality_and_caps() {
        let claims = vec![
            ranked_claim("low", ClaimImportance::Low, SourceQuality::High),
            ranked_claim("medium", ClaimImportance::Medium, SourceQuality::High),
            ranked_claim("critical", ClaimImportance::Critical, SourceQuality::Medium),
            ranked_claim("high", ClaimImportance::High, SourceQuality::High),
        ];

        let ranked = rank_claims(claims, 3);

        let ids: Vec<_> = ranked.iter().map(|claim| claim.id.as_str()).collect();
        assert_eq!(ids, vec!["critical", "high", "medium"]);
    }

    #[test]
    fn rank_claims_uses_stable_ascending_tiebreakers() {
        let claims = vec![
            ranked_claim("b", ClaimImportance::High, SourceQuality::High),
            ranked_claim("a", ClaimImportance::High, SourceQuality::High),
        ];

        let ranked = rank_claims(claims, 2);

        let ids: Vec<_> = ranked.iter().map(|claim| claim.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn aggregate_verdicts_kills_threshold_refuted_claim_and_survives_mixed_vote_claim() {
        let claims = vec![
            ranked_claim("killed", ClaimImportance::High, SourceQuality::High),
            ranked_claim("survivor", ClaimImportance::High, SourceQuality::High),
            ranked_claim("unverified", ClaimImportance::High, SourceQuality::High),
        ];
        let mut verdicts = BTreeMap::new();
        verdicts.insert(
            "killed".to_string(),
            vec![
                verdict(true, VerdictConfidence::High),
                verdict(true, VerdictConfidence::Medium),
                verdict(false, VerdictConfidence::Low),
            ],
        );
        verdicts.insert(
            "survivor".to_string(),
            vec![
                verdict(true, VerdictConfidence::Low),
                verdict(false, VerdictConfidence::Medium),
            ],
        );

        let aggregated = aggregate_verdicts(&claims, &verdicts, 2);

        assert_eq!(aggregated.killed.len(), 1);
        assert_eq!(aggregated.killed[0].claim.id, "killed");
        assert_eq!(aggregated.killed[0].refuted_votes, 2);
        let aggregated_json = serde_json::to_value(&aggregated).expect("serialize verdicts");
        assert!(aggregated_json.get("confirmed").is_none());
        assert_eq!(aggregated_json["survived"][0]["claim"]["id"], "survivor");
        assert_eq!(
            aggregated_json["survived"][0]["non_refuting_or_weak_votes"],
            2
        );
        assert_eq!(aggregated.unverified.len(), 1);
        assert_eq!(aggregated.unverified[0].claim.id, "unverified");
        assert_eq!(aggregated.unverified[0].total_votes, 0);
    }

    #[test]
    fn fallback_reports_include_caveats_and_stats_signal() {
        let stats = DeepSearchStats {
            sources_fetched: 2,
            claims_extracted: 0,
            budget_dropped: 3,
            ..DeepSearchStats::default()
        };

        let report = no_claims_report("What changed?", &stats);

        assert!(report
            .caveats
            .iter()
            .any(|caveat| caveat.contains("0 claims")));
        assert!(report
            .open_questions
            .iter()
            .any(|question| question.contains("budget dropped 3")));
        assert!(!report
            .open_questions
            .iter()
            .any(|question| question.contains("candidate sources")));
        assert_eq!(
            stats.to_template_stats(),
            json!({
                "sources_fetched": 2,
                "claims_extracted": 0,
                "claims_verified": 0,
                "survived": 0,
                "killed": 0,
                "url_dupes": 0,
                "budget_dropped": 3,
                "approval_blocked": 0
            })
        );
    }

    #[test]
    fn prompt_helpers_include_schema_instruction() {
        let prompt = super::super::deep_search_prompts::scope_prompt("What changed?");

        assert!(prompt.contains("Schema name: ScopeOutput"));
        assert!(prompt.contains("Return exactly one JSON object"));
    }

    fn ranked_claim(
        id: &str,
        importance: ClaimImportance,
        source_quality: SourceQuality,
    ) -> RankedClaim {
        RankedClaim {
            id: id.to_string(),
            source_url: format!("https://example.com/{id}"),
            source_title: id.to_string(),
            source_quality,
            claim: ExtractedClaim {
                claim: id.to_string(),
                quote: "quote".to_string(),
                importance,
            },
        }
    }

    fn verdict(refuted: bool, confidence: VerdictConfidence) -> ClaimVerdict {
        ClaimVerdict {
            refuted,
            evidence: "evidence".to_string(),
            confidence,
            counter_source: None,
        }
    }

    fn scope(angles: Vec<SearchAngle>) -> ScopeOutput {
        ScopeOutput {
            question: "What changed?".to_string(),
            summary: "Scoped".to_string(),
            angles,
        }
    }

    fn limits(
        max_angles: usize,
        max_sources: usize,
        max_claims: usize,
        votes_per_claim: usize,
        refutations_required: usize,
    ) -> DeepSearchLimits {
        DeepSearchLimits {
            max_angles,
            max_sources,
            max_claims,
            votes_per_claim,
            refutations_required,
            max_concurrency: 1,
            token_budget: None,
        }
    }

    fn json_text<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("serialize mock response")
    }

    struct MockJsonRunner {
        responses: Mutex<VecDeque<AgentJsonResponse>>,
        calls: Mutex<Vec<AgentJsonRequest>>,
    }

    impl MockJsonRunner {
        fn new(responses: Vec<String>) -> Self {
            let responses: Vec<_> = responses
                .into_iter()
                .map(agent_response_from_text)
                .collect();
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn new_agent_responses(responses: Vec<AgentJsonResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn labels(&self) -> Vec<String> {
            self.calls
                .lock()
                .expect("lock calls")
                .iter()
                .map(|call| call.label.clone())
                .collect()
        }
    }

    struct CancellingJsonRunner {
        responses: Mutex<VecDeque<AgentJsonResponse>>,
        calls: Mutex<Vec<AgentJsonRequest>>,
        cancellation: WorkflowCancellation,
        cancel_after_label: String,
    }

    impl CancellingJsonRunner {
        fn new(
            cancellation: WorkflowCancellation,
            cancel_after_label: impl Into<String>,
            responses: Vec<String>,
        ) -> Self {
            let responses: Vec<_> = responses
                .into_iter()
                .map(agent_response_from_text)
                .collect();
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                calls: Mutex::new(Vec::new()),
                cancellation,
                cancel_after_label: cancel_after_label.into(),
            }
        }

        fn labels(&self) -> Vec<String> {
            self.calls
                .lock()
                .expect("lock calls")
                .iter()
                .map(|call| call.label.clone())
                .collect()
        }
    }

    #[async_trait]
    impl AgentJsonRunner for CancellingJsonRunner {
        async fn run_json(&self, request: AgentJsonRequest) -> anyhow::Result<AgentJsonResponse> {
            let label = request.label.clone();
            self.calls.lock().expect("lock calls").push(request);
            let response = self
                .responses
                .lock()
                .expect("lock responses")
                .pop_front()
                .expect("mock response available");
            if label == self.cancel_after_label {
                self.cancellation.cancel();
            }
            Ok(response)
        }
    }

    #[async_trait]
    impl AgentJsonRunner for MockJsonRunner {
        async fn run_json(&self, request: AgentJsonRequest) -> anyhow::Result<AgentJsonResponse> {
            self.calls.lock().expect("lock calls").push(request);
            let text = self
                .responses
                .lock()
                .expect("lock responses")
                .pop_front()
                .expect("mock response available");
            Ok(text)
        }
    }

    fn agent_response_from_text(text: String) -> AgentJsonResponse {
        let value = serde_json::from_str(&text).unwrap_or_else(|_| json!(null));
        AgentJsonResponse {
            text,
            value,
            tokens_used: None,
        }
    }
}
