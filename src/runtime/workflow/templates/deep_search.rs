use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::runtime::workflow::agent_runner::{
    parse_agent_json_response_for_schema, AgentJsonRequest, AgentJsonRunner,
};
use crate::runtime::workflow::types::DeepSearchLimits;
use crate::runtime::workflow::{
    AgentJsonRepair, NoopWorkflowProgressSink, ScheduledAgentOutput, ScheduledAgentTask,
    UnavailableWorkflowSourceProvider, WorkflowCancellation, WorkflowFetchRequest,
    WorkflowFetchStatus, WorkflowProgressSink, WorkflowScheduleController, WorkflowScheduler,
    WorkflowSearchRequest, WorkflowSearchResult, WorkflowSourceProvider,
};

use super::deep_search_prompts::{extract_prompt, scope_prompt, synthesize_prompt, verify_prompt};

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
    pub search_failed: usize,
    pub search_skipped: usize,
    pub search_results: usize,
    pub sources_selected: usize,
    pub sources_extracted: usize,
    pub extract_failed: usize,
    pub extract_skipped: usize,
    pub claims_ranked: usize,
    pub verdict_votes: usize,
    pub verdict_failed: usize,
    pub verdict_skipped: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSearchStopReason {
    TokenBudgetExceeded,
    RuntimeExceeded,
}

impl DeepSearchStopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TokenBudgetExceeded => "token_budget_exceeded",
            Self::RuntimeExceeded => "runtime_exceeded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSearchStopped {
    phase_id: DeepSearchPhaseId,
    reason: DeepSearchStopReason,
    observed: i64,
    limit: i64,
}

impl DeepSearchStopped {
    pub fn new(
        phase_id: DeepSearchPhaseId,
        reason: DeepSearchStopReason,
        observed: i64,
        limit: i64,
    ) -> Self {
        Self {
            phase_id,
            reason,
            observed,
            limit,
        }
    }

    pub fn phase_id(&self) -> DeepSearchPhaseId {
        self.phase_id
    }

    pub fn reason(&self) -> DeepSearchStopReason {
        self.reason
    }

    pub fn observed(&self) -> i64 {
        self.observed
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }
}

impl fmt::Display for DeepSearchStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deep search stopped during {}: {} ({} > {})",
            self.phase_id.as_str(),
            self.reason.as_str(),
            self.observed,
            self.limit
        )
    }
}

impl Error for DeepSearchStopped {}

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
    pub failed_votes: usize,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchPhaseOutput {
    angle_label: String,
    output: SearchOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractPhaseOutput {
    source: SelectedSource,
    output: SourceExtractOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifyPhaseOutput {
    claim_id: String,
    verdict: ClaimVerdict,
}

#[derive(Debug)]
struct WorkflowBudgetGuard {
    started_at: Instant,
    token_budget: Option<i64>,
    max_runtime: Option<Duration>,
    tokens_used: std::sync::atomic::AtomicI64,
}

impl WorkflowBudgetGuard {
    fn new(limits: DeepSearchLimits) -> Self {
        Self {
            started_at: Instant::now(),
            token_budget: limits.token_budget,
            max_runtime: limits.max_runtime_secs.map(Duration::from_secs),
            tokens_used: std::sync::atomic::AtomicI64::new(0),
        }
    }

    fn add_tokens(&self, tokens: Option<i64>) {
        let Some(tokens) = tokens else {
            return;
        };
        self.tokens_used.fetch_add(tokens, Ordering::SeqCst);
    }

    fn ensure_within_limits(&self, phase_id: DeepSearchPhaseId) -> anyhow::Result<()> {
        if let Some(max_runtime) = self.max_runtime {
            if self.started_at.elapsed() >= max_runtime {
                return Err(DeepSearchStopped::new(
                    phase_id,
                    DeepSearchStopReason::RuntimeExceeded,
                    i64::try_from(self.started_at.elapsed().as_secs()).unwrap_or(i64::MAX),
                    i64::try_from(max_runtime.as_secs()).unwrap_or(i64::MAX),
                )
                .into());
            }
        }

        if let Some(token_budget) = self.token_budget {
            let tokens_used = self.tokens_used.load(Ordering::SeqCst);
            if tokens_used > token_budget {
                return Err(DeepSearchStopped::new(
                    phase_id,
                    DeepSearchStopReason::TokenBudgetExceeded,
                    tokens_used,
                    token_budget,
                )
                .into());
            }
        }

        Ok(())
    }
}

struct DeepSearchBudgetController {
    guard: Arc<WorkflowBudgetGuard>,
    phase_id: DeepSearchPhaseId,
}

impl DeepSearchBudgetController {
    fn new(guard: Arc<WorkflowBudgetGuard>, phase_id: DeepSearchPhaseId) -> Self {
        Self { guard, phase_id }
    }
}

impl WorkflowScheduleController for DeepSearchBudgetController {
    fn should_schedule(&self) -> bool {
        self.guard.ensure_within_limits(self.phase_id).is_ok()
    }

    fn record_task_tokens(&self, tokens_used: Option<i64>) -> bool {
        self.guard.add_tokens(tokens_used);
        self.guard.ensure_within_limits(self.phase_id).is_ok()
    }
}

struct ParsedAgentOutput<T> {
    value: T,
    tokens_used: Option<i64>,
}

#[derive(Clone)]
pub struct DeepSearchTemplateRunner {
    limits: DeepSearchLimits,
    runner: Arc<dyn AgentJsonRunner>,
    source_provider: Arc<dyn WorkflowSourceProvider>,
    json_repair: Option<Arc<dyn AgentJsonRepair>>,
    progress_sink: Arc<dyn WorkflowProgressSink>,
    cancellation: WorkflowCancellation,
    budget_guard: Arc<WorkflowBudgetGuard>,
    run_label: String,
}

impl DeepSearchTemplateRunner {
    pub fn new(limits: DeepSearchLimits, runner: Arc<dyn AgentJsonRunner>) -> Self {
        Self {
            limits,
            runner,
            source_provider: Arc::new(UnavailableWorkflowSourceProvider::new(
                "workflow source provider is not configured",
            )),
            json_repair: None,
            progress_sink: Arc::new(NoopWorkflowProgressSink),
            cancellation: WorkflowCancellation::new(),
            budget_guard: Arc::new(WorkflowBudgetGuard::new(limits)),
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

    pub fn with_source_provider(
        mut self,
        source_provider: Arc<dyn WorkflowSourceProvider>,
    ) -> Self {
        self.source_provider = source_provider;
        self
    }

    pub fn with_progress_sink(mut self, progress_sink: Arc<dyn WorkflowProgressSink>) -> Self {
        self.progress_sink = progress_sink;
        self
    }

    pub fn with_json_repair(mut self, json_repair: Arc<dyn AgentJsonRepair>) -> Self {
        self.json_repair = Some(json_repair);
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
        self.ensure_can_continue(DeepSearchPhaseId::Scope)?;
        self.progress_sink.start_phase("scope", "Scope", 1).await;
        let scope: ScopeOutput = self
            .run_agent_in_phase(
                DeepSearchPhaseId::Scope,
                "scope",
                "ScopeOutput",
                scope_prompt(question),
            )
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

        self.ensure_can_continue(DeepSearchPhaseId::Search)?;
        let search_budget =
            DeepSearchBudgetController::new(self.budget_guard.clone(), DeepSearchPhaseId::Search);
        let search_report = WorkflowScheduler::new(self.limits.max_concurrency)
            .run_phase_controlled(
                "search",
                "Search",
                scoped_angles
                    .iter()
                    .enumerate()
                    .map(|(angle_index, angle)| {
                        let label = format!("search-{number:04}", number = angle_index + 1);
                        self.scheduled_search_task(label, angle.clone())
                    })
                    .collect(),
                self.cancellation.clone(),
                self.progress_sink.as_ref(),
                &search_budget,
            )
            .await;
        phase_counts.searched_angles = search_report.outputs.len();
        phase_counts.search_failed = search_report.failed_agent_calls;
        phase_counts.search_skipped = search_report.skipped_agent_calls;
        phase_counts.search_results = search_report
            .outputs
            .iter()
            .map(|output| output.output.results.len())
            .sum();
        if self.cancellation.is_cancelled() {
            return Err(DeepSearchCancelled::new(DeepSearchPhaseId::Search).into());
        }
        self.ensure_can_continue(DeepSearchPhaseId::Search)?;

        let groups: Vec<_> = search_report
            .outputs
            .iter()
            .map(|output| AngledSearchResults {
                angle_label: &output.angle_label,
                results: &output.output.results,
            })
            .collect();
        let deduped = dedupe_search_results_by_angle(&groups, self.limits.max_sources);
        stats.url_dupes = deduped.url_dupes;
        stats.budget_dropped = deduped.budget_dropped;
        phase_counts.sources_selected = deduped.sources.len();

        self.ensure_can_continue(DeepSearchPhaseId::Extract)?;
        if deduped.sources.is_empty() {
            self.progress_sink
                .update_phase_counts("extract", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("extract").await;
            self.progress_sink
                .update_phase_counts("verify", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("verify").await;
            self.progress_sink
                .update_phase_counts("synthesize", 0, 0, 1)
                .await;
            self.progress_sink.skip_phase("synthesize").await;
            let report = no_sources_report(question, &stats);
            return Ok(DeepSearchRunResult {
                report,
                stats,
                selected_sources: deduped.sources,
                ranked_claims: Vec::new(),
                aggregated_verdicts: AggregatedVerdicts::default(),
                phase_counts,
            });
        }

        let fetched_sources = Arc::new(AtomicUsize::new(0));
        let approval_blocked = Arc::new(AtomicUsize::new(0));
        let extract_budget =
            DeepSearchBudgetController::new(self.budget_guard.clone(), DeepSearchPhaseId::Extract);
        let extract_report = WorkflowScheduler::new(self.limits.max_concurrency)
            .run_phase_controlled(
                "extract",
                "Extract",
                deduped
                    .sources
                    .iter()
                    .enumerate()
                    .map(|(source_index, source)| {
                        let label = format!("extract-{number:04}", number = source_index + 1);
                        self.scheduled_extract_task(
                            label,
                            question.to_string(),
                            source.clone(),
                            fetched_sources.clone(),
                            approval_blocked.clone(),
                        )
                    })
                    .collect(),
                self.cancellation.clone(),
                self.progress_sink.as_ref(),
                &extract_budget,
            )
            .await;
        phase_counts.sources_extracted = extract_report.outputs.len();
        phase_counts.extract_failed = extract_report.failed_agent_calls;
        phase_counts.extract_skipped = extract_report.skipped_agent_calls;
        stats.sources_fetched = fetched_sources.load(Ordering::SeqCst);
        stats.approval_blocked = approval_blocked.load(Ordering::SeqCst);
        if self.cancellation.is_cancelled() {
            return Err(DeepSearchCancelled::new(DeepSearchPhaseId::Extract).into());
        }
        self.ensure_can_continue(DeepSearchPhaseId::Extract)?;

        let mut extracted_claims = Vec::new();
        for ExtractPhaseOutput {
            source,
            output: source_extract,
        } in extract_report.outputs
        {
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
        stats.claims_extracted = extracted_claims.len();

        let ranked_claims = rank_claims(extracted_claims, self.limits.max_claims);
        stats.budget_dropped += stats.claims_extracted.saturating_sub(ranked_claims.len());
        phase_counts.claims_ranked = ranked_claims.len();

        self.ensure_can_continue(DeepSearchPhaseId::Verify)?;
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
        let mut verdicts_by_claim_id: BTreeMap<String, Vec<ClaimVerdict>> = BTreeMap::new();
        if planned_verdict_votes == 0 {
            self.progress_sink
                .update_phase_counts("verify", 0, 0, 0)
                .await;
            self.progress_sink.skip_phase("verify").await;
        } else {
            let verify_tasks = ranked_claims
                .iter()
                .enumerate()
                .flat_map(|(claim_index, claim)| {
                    (0..self.limits.votes_per_claim).map(move |vote_index| {
                        let label = format!(
                            "verify-{claim_number:04}-vote-{vote_number:04}",
                            claim_number = claim_index + 1,
                            vote_number = vote_index + 1
                        );
                        let prompt =
                            verify_prompt(question, claim, &verification_evidence_text(claim));
                        let claim_id = claim.id.clone();
                        self.scheduled_agent_task::<ClaimVerdict, _>(
                            label,
                            "ClaimVerdict",
                            prompt,
                            move |verdict| VerifyPhaseOutput { claim_id, verdict },
                        )
                    })
                })
                .collect();
            let verify_budget = DeepSearchBudgetController::new(
                self.budget_guard.clone(),
                DeepSearchPhaseId::Verify,
            );
            let verify_report = WorkflowScheduler::new(self.limits.max_concurrency)
                .run_phase_controlled(
                    "verify",
                    "Verify",
                    verify_tasks,
                    self.cancellation.clone(),
                    self.progress_sink.as_ref(),
                    &verify_budget,
                )
                .await;
            phase_counts.verdict_votes = verify_report.outputs.len();
            phase_counts.verdict_failed = verify_report.failed_agent_calls;
            phase_counts.verdict_skipped = verify_report.skipped_agent_calls;
            if self.cancellation.is_cancelled() {
                return Err(DeepSearchCancelled::new(DeepSearchPhaseId::Verify).into());
            }
            self.ensure_can_continue(DeepSearchPhaseId::Verify)?;
            for output in verify_report.outputs {
                verdicts_by_claim_id
                    .entry(output.claim_id)
                    .or_default()
                    .push(output.verdict);
            }
        }

        stats.claims_verified = verdicts_by_claim_id
            .values()
            .filter(|verdicts| !verdicts.is_empty())
            .count();
        let aggregated_verdicts = aggregate_verdicts(
            &ranked_claims,
            &verdicts_by_claim_id,
            self.limits.refutations_required,
            self.limits.votes_per_claim,
        );
        stats.survived = aggregated_verdicts.survived.len();
        stats.killed = aggregated_verdicts.killed.len();

        self.ensure_can_continue(DeepSearchPhaseId::Synthesize)?;
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

        if !ranked_claims.is_empty()
            && aggregated_verdicts.survived.is_empty()
            && aggregated_verdicts.killed.is_empty()
            && aggregated_verdicts.unverified.len() == ranked_claims.len()
        {
            self.progress_sink
                .update_phase_counts("synthesize", 0, 0, 1)
                .await;
            self.progress_sink.skip_phase("synthesize").await;
            let report = all_unverified_report(question, &aggregated_verdicts.unverified, &stats);
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
            .run_agent_in_phase::<DeepSearchReport>(
                DeepSearchPhaseId::Synthesize,
                "synthesize",
                "DeepSearchReport",
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

        self.ensure_can_continue(DeepSearchPhaseId::Synthesize)?;
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

    fn ensure_can_continue(&self, phase_id: DeepSearchPhaseId) -> anyhow::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(DeepSearchCancelled::new(phase_id).into());
        }
        self.budget_guard.ensure_within_limits(phase_id)?;
        Ok(())
    }

    #[cfg(test)]
    async fn run_agent<T>(
        &self,
        label: &str,
        schema_name: &str,
        prompt: String,
    ) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        Ok(self
            .run_agent_output(label, schema_name, prompt)
            .await?
            .value)
    }

    async fn run_agent_in_phase<T>(
        &self,
        phase_id: DeepSearchPhaseId,
        label: &str,
        schema_name: &str,
        prompt: String,
    ) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let output = self.run_agent_output(label, schema_name, prompt).await?;
        self.budget_guard.add_tokens(output.tokens_used);
        self.ensure_can_continue(phase_id)?;
        Ok(output.value)
    }

    async fn run_agent_output<T>(
        &self,
        label: &str,
        schema_name: &str,
        prompt: String,
    ) -> anyhow::Result<ParsedAgentOutput<T>>
    where
        T: DeserializeOwned,
    {
        run_agent_with_runner(
            self.runner.clone(),
            self.json_repair.clone(),
            self.run_label.clone(),
            label,
            schema_name,
            prompt,
        )
        .await
    }

    fn scheduled_search_task(
        &self,
        label: String,
        angle: SearchAngle,
    ) -> ScheduledAgentTask<SearchPhaseOutput> {
        let source_provider = self.source_provider.clone();
        let max_results = self.limits.max_sources;
        ScheduledAgentTask::new(move |_| async move {
            let response = source_provider
                .search(WorkflowSearchRequest {
                    query: angle.query.clone(),
                    max_results,
                })
                .await?;
            let output = SearchOutput {
                results: response
                    .results
                    .into_iter()
                    .map(search_result_item_from_workflow)
                    .collect(),
            };
            let _ = label;
            Ok(ScheduledAgentOutput::new(SearchPhaseOutput {
                angle_label: angle.label,
                output,
            }))
        })
    }

    fn scheduled_extract_task(
        &self,
        label: String,
        question: String,
        source: SelectedSource,
        fetched_sources: Arc<AtomicUsize>,
        approval_blocked: Arc<AtomicUsize>,
    ) -> ScheduledAgentTask<ExtractPhaseOutput> {
        let source_provider = self.source_provider.clone();
        let runner = self.runner.clone();
        let json_repair = self.json_repair.clone();
        let run_label = self.run_label.clone();
        ScheduledAgentTask::new(move |_| async move {
            let fetch = source_provider
                .fetch(WorkflowFetchRequest {
                    url: source.url.clone(),
                    timeout_secs: None,
                })
                .await;
            match fetch.status {
                WorkflowFetchStatus::Fetched => {
                    fetched_sources.fetch_add(1, Ordering::SeqCst);
                }
                WorkflowFetchStatus::ApprovalBlocked => {
                    approval_blocked.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!(
                        "{}",
                        fetch
                            .error
                            .unwrap_or_else(|| "source fetch requires approval".to_string())
                    );
                }
                WorkflowFetchStatus::Failed => {
                    anyhow::bail!(
                        "{}",
                        fetch
                            .error
                            .unwrap_or_else(|| "source fetch failed".to_string())
                    );
                }
            }

            let source_text = fetch.content.unwrap_or_default();
            let prompt = extract_prompt(&question, &source, &source_text);
            let output = run_agent_with_runner::<SourceExtractOutput>(
                runner,
                json_repair,
                run_label,
                &label,
                "SourceExtractOutput",
                prompt,
            )
            .await?;
            let parsed = ExtractPhaseOutput {
                source,
                output: output.value,
            };
            Ok(scheduled_output_with_optional_tokens(
                parsed,
                output.tokens_used,
            ))
        })
    }

    fn scheduled_agent_task<T, U>(
        &self,
        label: String,
        schema_name: &'static str,
        prompt: String,
        map_output: impl FnOnce(T) -> U + Send + 'static,
    ) -> ScheduledAgentTask<U>
    where
        T: DeserializeOwned + Send + 'static,
        U: Send + 'static,
    {
        let runner = self.runner.clone();
        let json_repair = self.json_repair.clone();
        let run_label = self.run_label.clone();
        ScheduledAgentTask::new(move |_| async move {
            let parsed = run_agent_with_runner::<T>(
                runner,
                json_repair,
                run_label,
                &label,
                schema_name,
                prompt,
            )
            .await?;
            Ok(scheduled_output_with_optional_tokens(
                map_output(parsed.value),
                parsed.tokens_used,
            ))
        })
    }
}

async fn run_agent_with_runner<T>(
    runner: Arc<dyn AgentJsonRunner>,
    json_repair: Option<Arc<dyn AgentJsonRepair>>,
    run_label: String,
    label: &str,
    schema_name: &str,
    prompt: String,
) -> anyhow::Result<ParsedAgentOutput<T>>
where
    T: DeserializeOwned,
{
    let response = runner
        .run_json(AgentJsonRequest {
            label: format!("{run_label}:{label}"),
            prompt,
            schema_hint: Some(json!({"type": "object"})),
        })
        .await?;
    if response.value.is_object() {
        if let Ok(parsed) = serde_json::from_value(response.value.clone()) {
            return Ok(ParsedAgentOutput {
                value: parsed,
                tokens_used: response.tokens_used,
            });
        }
    }
    let value =
        parse_agent_json_response_for_schema(schema_name, &response.text, json_repair.as_deref())
            .await?;
    Ok(ParsedAgentOutput {
        value,
        tokens_used: response.tokens_used,
    })
}

fn scheduled_output_with_optional_tokens<T>(
    value: T,
    tokens_used: Option<i64>,
) -> ScheduledAgentOutput<T> {
    match tokens_used {
        Some(tokens_used) => ScheduledAgentOutput::with_tokens(value, tokens_used),
        None => ScheduledAgentOutput::new(value),
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

fn search_result_item_from_workflow(result: WorkflowSearchResult) -> SearchResultItem {
    SearchResultItem {
        url: result.url,
        title: result.title,
        snippet: result.snippet,
        relevance: search_relevance_for_rank(result.rank),
    }
}

fn search_relevance_for_rank(rank: usize) -> SearchRelevance {
    match rank {
        0..=3 => SearchRelevance::High,
        4..=8 => SearchRelevance::Medium,
        _ => SearchRelevance::Low,
    }
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
    expected_votes_per_claim: usize,
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
            failed_votes: expected_votes_per_claim.saturating_sub(verdicts.len()),
            total_votes: verdicts.len(),
        };

        if refuted_votes >= threshold {
            aggregated.killed.push(summary);
        } else if verdicts.len() >= threshold {
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

pub fn no_sources_report(question: &str, stats: &DeepSearchStats) -> DeepSearchReport {
    DeepSearchReport {
        summary: format!("No usable sources were found for: {question}"),
        findings: Vec::new(),
        caveats: vec![format!(
            "Deep search found 0 usable sources; {} search results were dropped by duplicate or budget filters.",
            stats.budget_dropped
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

pub fn all_unverified_report(
    question: &str,
    unverified: &[ClaimVoteSummary],
    stats: &DeepSearchStats,
) -> DeepSearchReport {
    DeepSearchReport {
        summary: format!("Candidate claims did not reach verification quorum for: {question}"),
        findings: unverified
            .iter()
            .map(|summary| {
                format!(
                    "Unverified: {} ({} valid votes, {} failed or missing votes)",
                    summary.claim.claim.claim, summary.total_votes, summary.failed_votes
                )
            })
            .collect(),
        caveats: vec![format!(
            "{} candidate claims lacked enough successful verifier votes to confirm or refute.",
            unverified
                .len()
                .max(stats.claims_extracted.saturating_sub(stats.claims_verified))
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
    use crate::runtime::workflow::json_repair::{AgentJsonParseFailure, AgentJsonRepair};
    use crate::runtime::workflow::progress::{
        RecordingWorkflowProgressSink, WorkflowProgressEvent,
    };
    use crate::runtime::workflow::types::DeepSearchLimits;
    use crate::runtime::workflow::{
        WorkflowCancellation, WorkflowFetchOutput, WorkflowSearchResponse, WorkflowSourceError,
        WorkflowSourceFetch, WorkflowSourceSearch,
    };

    fn result(url: &str, title: &str) -> SearchResultItem {
        SearchResultItem {
            url: url.to_string(),
            title: title.to_string(),
            snippet: "snippet".to_string(),
            relevance: SearchRelevance::High,
        }
    }

    fn source_provider(searches: Vec<Vec<SearchResultItem>>) -> Arc<MockWorkflowSourceProvider> {
        Arc::new(MockWorkflowSourceProvider::new(searches))
    }

    fn failing_source_provider(error: &str) -> Arc<MockWorkflowSourceProvider> {
        Arc::new(MockWorkflowSourceProvider::new_results(vec![Err(
            WorkflowSourceError::Provider(error.to_string()),
        )]))
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
        let sources = source_provider(vec![vec![
            result("https://example.com/a", "A"),
            result("https://example.com/b", "B"),
        ]]);

        let result = DeepSearchTemplateRunner::new(limits, runner.clone())
            .with_source_provider(sources)
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
            .run_agent("scope", "ScopeOutput", "ignored".to_string())
            .await
            .expect("parse structured value");

        assert_eq!(parsed, scoped);
    }

    #[tokio::test]
    async fn template_runner_repairs_invalid_json_once_before_failing_phase() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            "{\"question\":\"broken\",}".to_string()
        ]));
        let repair = Arc::new(StaticJsonRepair::new(vec![json_text(&scope(Vec::new()))]));

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner)
            .with_json_repair(repair.clone())
            .run("What changed?")
            .await
            .expect("scope JSON should be repaired");

        assert!(result.report.summary.contains("No usable sources"));
        assert_eq!(repair.schema_names(), vec!["ScopeOutput"]);
    }

    #[tokio::test]
    async fn template_runner_counts_search_item_failure_without_failing_run() {
        let runner = Arc::new(MockJsonRunner::new(vec![json_text(&scope(vec![
            SearchAngle {
                label: "extract phase bait".to_string(),
                query: "search query".to_string(),
                rationale: "why".to_string(),
            },
        ]))]));
        let sources = failing_source_provider("search provider failed");

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner)
            .with_source_provider(sources)
            .run("What changed?")
            .await
            .expect("failed host search item should be counted and degraded");

        assert!(result.report.summary.contains("No usable sources"));
        assert_eq!(result.phase_counts.searched_angles, 0);
        assert_eq!(result.phase_counts.search_failed, 1);
        assert_eq!(result.phase_counts.search_results, 0);
        assert_eq!(result.selected_sources.len(), 0);
    }

    #[tokio::test]
    async fn template_runner_stops_when_token_budget_is_exceeded() {
        let scoped = scope(vec![SearchAngle {
            label: "budget".to_string(),
            query: "budget query".to_string(),
            rationale: "why".to_string(),
        }]);
        let runner = Arc::new(MockJsonRunner::new_agent_responses(vec![
            agent_response_with_tokens(json_text(&scoped), 10),
        ]));
        let mut limits = limits(1, 1, 1, 1, 1);
        limits.token_budget = Some(5);

        let error = DeepSearchTemplateRunner::new(limits, runner.clone())
            .run("What changed?")
            .await
            .expect_err("token budget should stop workflow");

        let stopped = error
            .chain()
            .find_map(|source| source.downcast_ref::<DeepSearchStopped>())
            .expect("typed stop error");
        assert_eq!(stopped.phase_id(), DeepSearchPhaseId::Scope);
        assert_eq!(stopped.reason(), DeepSearchStopReason::TokenBudgetExceeded);
        assert_eq!(stopped.observed(), 10);
        assert_eq!(stopped.limit(), 5);
        assert_eq!(runner.labels(), vec!["deep-search:scope"]);
    }

    #[tokio::test]
    async fn template_runner_stops_scheduling_when_token_budget_is_exceeded_mid_phase() {
        let scoped = scope(vec![SearchAngle {
            label: "budget".to_string(),
            query: "budget query".to_string(),
            rationale: "why".to_string(),
        }]);
        let runner = Arc::new(MockJsonRunner::new_agent_responses(vec![
            agent_response_with_tokens(json_text(&scoped), 1),
            agent_response_with_tokens(
                json_text(&SourceExtractOutput {
                    source_url: "https://example.com/a".to_string(),
                    source_title: "A".to_string(),
                    source_quality: SourceQuality::High,
                    publish_date: None,
                    claims: vec![ExtractedClaim {
                        claim: "Costly claim".to_string(),
                        quote: "Quote".to_string(),
                        importance: ClaimImportance::High,
                    }],
                }),
                10,
            ),
        ]));
        let mut limits = limits(1, 2, 1, 1, 1);
        limits.token_budget = Some(5);
        let sources = source_provider(vec![vec![
            result("https://example.com/a", "A"),
            result("https://example.com/b", "B"),
        ]]);

        let error = DeepSearchTemplateRunner::new(limits, runner.clone())
            .with_source_provider(sources)
            .run("What changed?")
            .await
            .expect_err("token budget should stop queued extract work");

        let stopped = error
            .chain()
            .find_map(|source| source.downcast_ref::<DeepSearchStopped>())
            .expect("typed stop error");
        assert_eq!(stopped.phase_id(), DeepSearchPhaseId::Extract);
        assert_eq!(stopped.reason(), DeepSearchStopReason::TokenBudgetExceeded);
        assert_eq!(stopped.observed(), 11);
        assert_eq!(stopped.limit(), 5);
        assert_eq!(
            runner.labels(),
            vec!["deep-search:scope", "deep-search:extract-0001"]
        );
    }

    #[tokio::test]
    async fn template_runner_stops_immediately_when_runtime_limit_is_exceeded() {
        let runner = Arc::new(MockJsonRunner::new(Vec::new()));
        let mut limits = limits(1, 1, 1, 1, 1);
        limits.max_runtime_secs = Some(0);

        let error = DeepSearchTemplateRunner::new(limits, runner.clone())
            .run("What changed?")
            .await
            .expect_err("runtime limit should stop workflow");

        let stopped = error
            .downcast_ref::<DeepSearchStopped>()
            .expect("typed stop error");
        assert_eq!(stopped.phase_id(), DeepSearchPhaseId::Scope);
        assert_eq!(stopped.reason(), DeepSearchStopReason::RuntimeExceeded);
        assert_eq!(stopped.limit(), 0);
        assert!(runner.labels().is_empty());
    }

    #[tokio::test]
    async fn template_runner_counts_extract_item_failure_without_dropping_successes() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "current".to_string(),
                query: "current query".to_string(),
                rationale: "why".to_string(),
            }])),
            "not json".to_string(),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/good".to_string(),
                source_title: "Good".to_string(),
                source_quality: SourceQuality::High,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Surviving claim".to_string(),
                    quote: "Quote".to_string(),
                    importance: ClaimImportance::High,
                }],
            }),
            json_text(&verdict(false, VerdictConfidence::High)),
            json_text(&DeepSearchReport {
                summary: "Synthesized partial answer".to_string(),
                findings: vec!["Finding".to_string()],
                caveats: Vec::new(),
                open_questions: Vec::new(),
            }),
        ]));
        let sources = source_provider(vec![vec![
            result("https://example.com/bad", "Bad"),
            result("https://example.com/good", "Good"),
        ]]);

        let result = DeepSearchTemplateRunner::new(limits(1, 2, 1, 1, 1), runner)
            .with_source_provider(sources)
            .run("What changed?")
            .await
            .expect("invalid extract item should be counted and degraded");

        assert_eq!(result.selected_sources.len(), 2);
        assert_eq!(result.stats.sources_fetched, 2);
        assert_eq!(result.stats.claims_extracted, 1);
        assert_eq!(result.phase_counts.sources_extracted, 1);
        assert_eq!(result.phase_counts.extract_failed, 1);
        assert_eq!(result.phase_counts.claims_ranked, 1);
        assert_eq!(result.report.summary, "Synthesized partial answer");
    }

    #[tokio::test]
    async fn template_runner_counts_fetch_approval_block_without_failing_run() {
        let runner = Arc::new(MockJsonRunner::new(vec![json_text(&scope(vec![
            SearchAngle {
                label: "approval".to_string(),
                query: "approval query".to_string(),
                rationale: "why".to_string(),
            },
        ]))]));
        let sources = Arc::new(
            MockWorkflowSourceProvider::new(vec![vec![result(
                "https://example.com/blocked",
                "Blocked",
            )]])
            .with_fetches(vec![WorkflowFetchOutput {
                url: "https://example.com/blocked".to_string(),
                final_url: None,
                status: WorkflowFetchStatus::ApprovalBlocked,
                content: None,
                http_status: None,
                content_type: None,
                body_bytes: None,
                body_truncated: None,
                output_truncated: None,
                policy_decision: Some("review_required".to_string()),
                error: Some("network fetch requires approval".to_string()),
            }]),
        );

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner.clone())
            .with_source_provider(sources)
            .run("What changed?")
            .await
            .expect("approval-blocked fetch should degrade");

        assert!(result.report.summary.contains("No verifiable claims"));
        assert_eq!(result.selected_sources.len(), 1);
        assert_eq!(result.stats.sources_fetched, 0);
        assert_eq!(result.stats.approval_blocked, 1);
        assert_eq!(result.phase_counts.sources_extracted, 0);
        assert_eq!(result.phase_counts.extract_failed, 1);
        assert_eq!(runner.labels(), vec!["deep-search:scope"]);
    }

    #[tokio::test]
    async fn template_runner_counts_verify_item_failure_and_keeps_valid_votes() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "current".to_string(),
                query: "current query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::High,
                publish_date: None,
                claims: vec![ExtractedClaim {
                    claim: "Partly verified claim".to_string(),
                    quote: "Quote".to_string(),
                    importance: ClaimImportance::High,
                }],
            }),
            "not json".to_string(),
            json_text(&verdict(false, VerdictConfidence::High)),
        ]));
        let sources = source_provider(vec![vec![result("https://example.com/source", "Source")]]);

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 2, 2), runner)
            .with_source_provider(sources)
            .run("What changed?")
            .await
            .expect("invalid verify item should be counted and degraded");

        assert_eq!(result.phase_counts.verdict_votes, 1);
        assert_eq!(result.phase_counts.verdict_failed, 1);
        assert_eq!(result.stats.claims_verified, 1);
        assert_eq!(result.stats.survived, 0);
        assert_eq!(result.aggregated_verdicts.unverified[0].total_votes, 1);
        assert_eq!(result.aggregated_verdicts.unverified[0].failed_votes, 1);
        assert!(result.report.summary.contains("verification quorum"));
    }

    #[tokio::test]
    async fn template_runner_returns_no_claims_fallback_without_synthesis() {
        let runner = Arc::new(MockJsonRunner::new(vec![
            json_text(&scope(vec![SearchAngle {
                label: "current".to_string(),
                query: "current query".to_string(),
                rationale: "why".to_string(),
            }])),
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
        ]));
        let sources = source_provider(vec![vec![result("https://example.com/source", "Source")]]);

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 4, 2, 2), runner.clone())
            .with_source_provider(sources)
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
            json_text(&SourceExtractOutput {
                source_url: "https://example.com/source".to_string(),
                source_title: "Source".to_string(),
                source_quality: SourceQuality::Medium,
                publish_date: None,
                claims: Vec::new(),
            }),
        ]));
        let sources = source_provider(vec![vec![result("https://example.com/source", "Source")]]);
        let progress = RecordingWorkflowProgressSink::new();

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 4, 2, 2), runner)
            .with_source_provider(sources)
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
        let sources = source_provider(vec![vec![result("https://example.com/source", "Source")]]);

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner.clone())
            .with_source_provider(sources)
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
        let sources = source_provider(vec![vec![result("https://example.com/source", "Source")]]);

        let result = DeepSearchTemplateRunner::new(limits(1, 1, 1, 1, 1), runner.clone())
            .with_source_provider(sources)
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
        let sources = source_provider(vec![
            vec![
                result("https://www.example.com/a/?utm=1", "A"),
                result("https://example.com/b", "B"),
            ],
            vec![
                result("https://example.com/a", "A duplicate"),
                result("https://example.com/c", "C"),
                result("https://example.com/d", "D"),
            ],
        ]);

        let result = DeepSearchTemplateRunner::new(limits(2, 2, 4, 2, 2), runner.clone())
            .with_source_provider(sources)
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
        let runner = Arc::new(MockJsonRunner::new(vec![json_text(&scope(vec![
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
        ]))]));
        let sources = Arc::new(CancellingWorkflowSourceProvider::new(
            cancellation.clone(),
            vec![
                vec![result("https://example.com/a", "A")],
                vec![result("https://example.com/b", "B")],
            ],
        ));

        let error = DeepSearchTemplateRunner::new(limits(2, 2, 4, 2, 2), runner.clone())
            .with_source_provider(sources)
            .with_run_label("unit")
            .with_cancellation(cancellation)
            .run("What changed?")
            .await
            .expect_err("cancellation should stop template execution");

        let cancelled = error
            .downcast_ref::<DeepSearchCancelled>()
            .expect("template should return a cancellation error");
        assert_eq!(cancelled.phase_id(), DeepSearchPhaseId::Search);
        assert_eq!(runner.labels(), vec!["unit:scope"]);
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

        let aggregated = aggregate_verdicts(&claims, &verdicts, 2, 3);

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
        assert_eq!(aggregated.unverified[0].failed_votes, 3);
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
            max_runtime_secs: None,
        }
    }

    fn json_text<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("serialize mock response")
    }

    struct MockJsonRunner {
        responses: Mutex<VecDeque<AgentJsonResponse>>,
        calls: Mutex<Vec<AgentJsonRequest>>,
    }

    struct MockWorkflowSourceProvider {
        searches: Mutex<VecDeque<Result<Vec<SearchResultItem>, WorkflowSourceError>>>,
        fetches: Mutex<VecDeque<WorkflowFetchOutput>>,
    }

    impl MockWorkflowSourceProvider {
        fn new(searches: Vec<Vec<SearchResultItem>>) -> Self {
            Self::new_results(searches.into_iter().map(Ok).collect())
        }

        fn new_results(searches: Vec<Result<Vec<SearchResultItem>, WorkflowSourceError>>) -> Self {
            Self {
                searches: Mutex::new(VecDeque::from(searches)),
                fetches: Mutex::new(VecDeque::new()),
            }
        }

        fn with_fetches(mut self, fetches: Vec<WorkflowFetchOutput>) -> Self {
            self.fetches = Mutex::new(VecDeque::from(fetches));
            self
        }
    }

    #[async_trait]
    impl WorkflowSourceSearch for MockWorkflowSourceProvider {
        async fn search(
            &self,
            request: WorkflowSearchRequest,
        ) -> Result<WorkflowSearchResponse, WorkflowSourceError> {
            let results = self
                .searches
                .lock()
                .expect("lock searches")
                .pop_front()
                .unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(WorkflowSearchResponse {
                query: request.query,
                provider: "mock".to_string(),
                results: results
                    .into_iter()
                    .enumerate()
                    .map(|(index, result)| WorkflowSearchResult {
                        title: result.title,
                        url: result.url,
                        snippet: result.snippet,
                        rank: index + 1,
                        provider: "mock".to_string(),
                    })
                    .collect(),
            })
        }
    }

    #[async_trait]
    impl WorkflowSourceFetch for MockWorkflowSourceProvider {
        async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput {
            self.fetches
                .lock()
                .expect("lock fetches")
                .pop_front()
                .unwrap_or_else(|| WorkflowFetchOutput {
                    url: request.url,
                    final_url: None,
                    status: WorkflowFetchStatus::Fetched,
                    content: Some("Fetched source text".to_string()),
                    http_status: Some(200),
                    content_type: Some("text/plain".to_string()),
                    body_bytes: Some(19),
                    body_truncated: Some(false),
                    output_truncated: Some(false),
                    policy_decision: Some("allow".to_string()),
                    error: None,
                })
        }
    }

    struct CancellingWorkflowSourceProvider {
        inner: MockWorkflowSourceProvider,
        cancellation: WorkflowCancellation,
        calls: Mutex<usize>,
    }

    impl CancellingWorkflowSourceProvider {
        fn new(cancellation: WorkflowCancellation, searches: Vec<Vec<SearchResultItem>>) -> Self {
            Self {
                inner: MockWorkflowSourceProvider::new(searches),
                cancellation,
                calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl WorkflowSourceSearch for CancellingWorkflowSourceProvider {
        async fn search(
            &self,
            request: WorkflowSearchRequest,
        ) -> Result<WorkflowSearchResponse, WorkflowSourceError> {
            let response = self.inner.search(request).await?;
            let mut calls = self.calls.lock().expect("lock calls");
            *calls = calls.saturating_add(1);
            if *calls == 1 {
                self.cancellation.cancel();
            }
            Ok(response)
        }
    }

    #[async_trait]
    impl WorkflowSourceFetch for CancellingWorkflowSourceProvider {
        async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput {
            self.inner.fetch(request).await
        }
    }

    struct StaticJsonRepair {
        responses: Mutex<VecDeque<String>>,
        failures: Mutex<Vec<AgentJsonParseFailure>>,
    }

    impl StaticJsonRepair {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                failures: Mutex::new(Vec::new()),
            }
        }

        fn schema_names(&self) -> Vec<String> {
            self.failures
                .lock()
                .expect("lock failures")
                .iter()
                .map(|failure| failure.schema_name.clone())
                .collect()
        }
    }

    #[async_trait]
    impl AgentJsonRepair for StaticJsonRepair {
        async fn repair_json(&self, failure: AgentJsonParseFailure) -> anyhow::Result<String> {
            self.failures.lock().expect("lock failures").push(failure);
            self.responses
                .lock()
                .expect("lock repair responses")
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("repair response available"))
        }
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

    fn agent_response_with_tokens(text: String, tokens_used: i64) -> AgentJsonResponse {
        let value = serde_json::from_str(&text).unwrap_or_else(|_| json!(null));
        AgentJsonResponse {
            text,
            value,
            tokens_used: Some(tokens_used),
        }
    }
}
