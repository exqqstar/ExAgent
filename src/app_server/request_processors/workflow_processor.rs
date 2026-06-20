use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    TurnStartParams, WorkflowCancelParams, WorkflowCancelResponse, WorkflowPresetId,
    WorkflowReadParams, WorkflowReadResponse, WorkflowRunStatus, WorkflowRunView,
    WorkflowStartParams, WorkflowStartResponse, WorkflowStopReason, WorkflowTemplateId,
};
use crate::app_server::request_processors::thread_processor::{
    self, InitialHistory, StartThreadOptions,
};
use crate::app_server::request_processors::turn_processor;
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::read_thread_state_from_storage;
use crate::app_server::AppServerError;
use crate::runtime::agent_profile::AgentType;
use crate::runtime::workflow::templates::deep_search::{
    DeepSearchCancelled, DeepSearchPhaseCounts, DeepSearchPhaseError, DeepSearchRunResult,
    DeepSearchStopReason, DeepSearchStopped, DeepSearchTemplateRunner,
};
use crate::runtime::workflow::{
    json_repair_prompt, parse_json_object, AgentJsonParseFailure, AgentJsonRepair,
    AgentJsonRequest, AgentJsonResponse, AgentJsonRunner, DeepSearchLimits,
    RuntimeWorkflowSourceProvider, UnavailableWorkflowSourceProvider, WorkflowCancellation,
    WorkflowProgressSink, WorkflowRunHandle, WorkflowRunId, WorkflowRunState,
    WorkflowSourceProvider,
};
use crate::session::{ThreadLineage, ThreadSource};
use crate::state::rollout::{
    latest_workflow_run_from_rollout_items, rollout_paths, RolloutItem, RolloutStore,
};
use crate::tools::web::web_search::BraveSearchProvider;
use crate::types::ThreadId;

const MAX_WORKFLOW_LABEL_QUESTION_CHARS: usize = 120;
const MAX_WORKFLOW_ARTIFACT_ITEMS: usize = 50;
const MAX_WORKFLOW_TEMPLATE_STATS_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(in crate::app_server) struct WorkflowRunRegistry {
    runs: Mutex<HashMap<String, WorkflowRunRegistryEntry>>,
}

#[derive(Clone)]
struct WorkflowRunRegistryEntry {
    workspace_root: PathBuf,
    handle: WorkflowRunHandle,
    cancellation: WorkflowCancellation,
}

impl WorkflowRunRegistry {
    pub(in crate::app_server) fn new() -> Self {
        Self::default()
    }

    async fn insert(
        &self,
        run_id: String,
        workspace_root: PathBuf,
        handle: WorkflowRunHandle,
        cancellation: WorkflowCancellation,
    ) {
        self.runs.lock().await.insert(
            run_id,
            WorkflowRunRegistryEntry {
                workspace_root,
                handle,
                cancellation,
            },
        );
    }

    async fn get(
        &self,
        run_id: &str,
        workspace_root: &std::path::Path,
    ) -> Result<WorkflowRunRegistryEntry> {
        let entry = self.runs.lock().await.get(run_id).cloned();
        let Some(entry) = entry else {
            bail!(AppServerError::InvalidRequest(format!(
                "workflow run not found: {run_id}"
            )));
        };

        if entry.workspace_root != workspace_root {
            bail!(AppServerError::InvalidRequest(format!(
                "workflow run not found in workspace: {run_id}"
            )));
        }

        Ok(entry)
    }

    async fn find(&self, run_id: &str) -> Option<WorkflowRunRegistryEntry> {
        self.runs.lock().await.get(run_id).cloned()
    }
}

pub(in crate::app_server) fn new_workflow_run_registry() -> Arc<WorkflowRunRegistry> {
    Arc::new(WorkflowRunRegistry::new())
}

pub(in crate::app_server) async fn workflow_start(
    services: &AppServerServices,
    params: WorkflowStartParams,
) -> Result<WorkflowStartResponse> {
    validate_template(&params.template_id)?;
    let config = OverridePolicy::merge_thread_start(
        &services.base_config,
        RuntimeOverrides {
            workspace_root: params.workspace_root,
            cwd: params.cwd,
            permission_profile: None,
        },
    )?;
    let workspace_root = config.workspace_root.clone();
    let template_id = params.template_id;
    let preset_id = params.preset_id;
    let label = workflow_label(&template_id, &params.question);
    let question = params.question;
    let workflow_config = config.clone();

    let new_thread = thread_processor::start_thread_with_options(
        services,
        StartThreadOptions {
            config,
            initial_history: InitialHistory::New,
            thread_source: ThreadSource::User,
            lineage: None,
            subagent_control: None,
        },
    )?;

    let thread_id = new_thread.thread_id.clone();
    let run_id = format!("workflow_run_{}", thread_id.as_str());
    let handle = WorkflowRunHandle::new(WorkflowRunState::new(
        WorkflowRunId::new(run_id.clone()),
        thread_id.clone(),
        template_id.clone(),
        preset_id,
        label,
    ));
    let cancellation = WorkflowCancellation::new();

    services
        .workflow_runs
        .insert(run_id.clone(), workspace_root, handle, cancellation.clone())
        .await;

    spawn_workflow_execution(
        Arc::new(services.clone()),
        workflow_config,
        run_id.clone(),
        thread_id,
        template_id,
        preset_id,
        question,
        cancellation,
    );

    Ok(WorkflowStartResponse {
        run_id,
        thread_id: new_thread.thread_id,
        status: WorkflowRunStatus::Queued,
    })
}

pub(in crate::app_server) async fn workflow_read(
    services: &AppServerServices,
    params: WorkflowReadParams,
) -> Result<WorkflowReadResponse> {
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let run = match services.workflow_runs.find(&params.run_id).await {
        Some(entry) => {
            if entry.workspace_root != config.workspace_root {
                bail!(AppServerError::InvalidRequest(format!(
                    "workflow run not found in workspace: {}",
                    params.run_id
                )));
            }
            entry.handle.view().await
        }
        None => read_terminal_workflow_run(&config.workspace_root, &params.run_id)
            .await?
            .ok_or_else(|| {
                AppServerError::InvalidRequest(format!("workflow run not found: {}", params.run_id))
            })?,
    };

    Ok(WorkflowReadResponse { run })
}

pub(in crate::app_server) async fn workflow_cancel(
    services: &AppServerServices,
    params: WorkflowCancelParams,
) -> Result<WorkflowCancelResponse> {
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let Some(entry) = services.workflow_runs.find(&params.run_id).await else {
        let run = read_terminal_workflow_run(&config.workspace_root, &params.run_id)
            .await?
            .ok_or_else(|| {
                AppServerError::InvalidRequest(format!("workflow run not found: {}", params.run_id))
            })?;
        return Ok(WorkflowCancelResponse { run });
    };
    if entry.workspace_root != config.workspace_root {
        bail!(AppServerError::InvalidRequest(format!(
            "workflow run not found in workspace: {}",
            params.run_id
        )));
    }
    let status = entry.handle.view().await.status;
    if !is_terminal(status) {
        entry.cancellation.cancel();
        entry.handle.cancel().await;
    }
    let run = entry.handle.view().await;
    persist_terminal_workflow_run(&config.workspace_root, &run).await?;

    Ok(WorkflowCancelResponse { run })
}

fn validate_template(template_id: &WorkflowTemplateId) -> Result<()> {
    match template_id {
        WorkflowTemplateId::DeepResearch => Ok(()),
    }
}

fn workflow_label(template_id: &WorkflowTemplateId, question: &str) -> String {
    let prefix = match template_id {
        WorkflowTemplateId::DeepResearch => "Deep research",
    };
    let question = question.trim();
    if question.is_empty() {
        return prefix.to_string();
    }

    format!("{prefix}: {}", truncate_label(question))
}

fn truncate_label(value: &str) -> String {
    let mut chars = value.chars();
    let truncated: String = chars
        .by_ref()
        .take(MAX_WORKFLOW_LABEL_QUESTION_CHARS)
        .collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn is_terminal(status: WorkflowRunStatus) -> bool {
    matches!(
        status,
        WorkflowRunStatus::Completed | WorkflowRunStatus::Failed | WorkflowRunStatus::Cancelled
    )
}

fn workflow_thread_id_from_run_id(run_id: &str) -> Result<ThreadId> {
    let Some(thread_id) = run_id.strip_prefix("workflow_run_") else {
        bail!(AppServerError::InvalidRequest(format!(
            "workflow run not found: {run_id}"
        )));
    };
    if !is_valid_workflow_thread_id_suffix(thread_id) {
        bail!(AppServerError::InvalidRequest(format!(
            "workflow run not found: {run_id}"
        )));
    }
    Ok(ThreadId::new(thread_id.to_string()))
}

fn is_valid_workflow_thread_id_suffix(thread_id: &str) -> bool {
    !thread_id.is_empty()
        && thread_id != "."
        && thread_id != ".."
        && thread_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

async fn persist_terminal_workflow_run(workspace_root: &Path, run: &WorkflowRunView) -> Result<()> {
    if !is_terminal(run.status) {
        return Ok(());
    }
    let paths = rollout_paths(workspace_root, &run.thread_id);
    RolloutStore::new(paths.rollout_path)
        .append_items(&[RolloutItem::WorkflowRun(run.clone())])
        .await
        .context("failed to persist terminal workflow run")?;
    Ok(())
}

async fn read_terminal_workflow_run(
    workspace_root: &Path,
    run_id: &str,
) -> Result<Option<WorkflowRunView>> {
    let thread_id = workflow_thread_id_from_run_id(run_id)?;
    let paths = rollout_paths(workspace_root, &thread_id);
    let items = RolloutStore::read_items(&paths.rollout_path)
        .await
        .context("failed to read workflow rollout")?;
    Ok(
        latest_workflow_run_from_rollout_items(&items, run_id)
            .filter(|run| is_terminal(run.status)),
    )
}

fn spawn_workflow_execution(
    services: Arc<AppServerServices>,
    workflow_config: crate::config::AgentConfig,
    run_id: String,
    thread_id: ThreadId,
    template_id: WorkflowTemplateId,
    preset_id: WorkflowPresetId,
    question: String,
    cancellation: WorkflowCancellation,
) {
    tokio::spawn(async move {
        let entry = match services
            .workflow_runs
            .get(&run_id, &workflow_config.workspace_root)
            .await
        {
            Ok(entry) => entry,
            Err(_) => return,
        };

        match template_id {
            WorkflowTemplateId::DeepResearch => {
                run_deep_research_workflow(
                    services,
                    workflow_config,
                    entry.handle,
                    thread_id,
                    run_id,
                    preset_id,
                    question,
                    cancellation,
                )
                .await;
            }
        }
    });
}

async fn run_deep_research_workflow(
    services: Arc<AppServerServices>,
    workflow_config: crate::config::AgentConfig,
    handle: WorkflowRunHandle,
    root_thread_id: ThreadId,
    run_id: String,
    preset_id: WorkflowPresetId,
    question: String,
    cancellation: WorkflowCancellation,
) {
    if is_terminal(handle.view().await.status) {
        return;
    }

    let limits = DeepSearchLimits::for_preset(preset_id);
    handle.start().await;
    let progress_sink = Arc::new(WorkflowRunHandleProgressSink::new(handle.clone()));

    let runner = Arc::new(AppServerAgentJsonRunner::new(
        services.clone(),
        workflow_config.clone(),
        root_thread_id,
        run_id.clone(),
    ));
    let source_provider = workflow_source_provider_for_config(&services, &workflow_config);
    let template_runner = DeepSearchTemplateRunner::new(limits, runner.clone())
        .with_source_provider(source_provider)
        .with_json_repair(runner.clone())
        .with_progress_sink(progress_sink)
        .with_run_label(format!("workflow:{run_id}"))
        .with_cancellation(cancellation.clone());

    match template_runner.run(&question).await {
        Ok(result) => {
            if is_terminal(handle.view().await.status) {
                return;
            }
            record_deep_search_success(&handle, result, limits, runner.stats()).await;
            let run = handle.view().await;
            let _ = persist_terminal_workflow_run(&workflow_config.workspace_root, &run).await;
        }
        Err(error) => {
            if error.downcast_ref::<DeepSearchCancelled>().is_some() && cancellation.is_cancelled()
            {
                handle.cancel().await;
                let run = handle.view().await;
                let _ = persist_terminal_workflow_run(&workflow_config.workspace_root, &run).await;
                return;
            }
            if is_terminal(handle.view().await.status) {
                return;
            }
            record_deep_search_failure(&handle, &error, runner.stats()).await;
            let run = handle.view().await;
            let _ = persist_terminal_workflow_run(&workflow_config.workspace_root, &run).await;
        }
    }
}

fn workflow_source_provider_for_config(
    services: &AppServerServices,
    workflow_config: &crate::config::AgentConfig,
) -> Arc<dyn WorkflowSourceProvider> {
    if let Some(provider) = services.workflow_source_provider.clone() {
        return provider;
    }

    let Some(search_config) = workflow_config.web_search.as_ref() else {
        return Arc::new(UnavailableWorkflowSourceProvider::new(
            "web search is not configured",
        ));
    };

    if !search_config.provider.eq_ignore_ascii_case("brave") {
        return Arc::new(UnavailableWorkflowSourceProvider::new(format!(
            "unsupported web search provider: {}",
            search_config.provider
        )));
    }

    Arc::new(RuntimeWorkflowSourceProvider::new(
        Arc::new(BraveSearchProvider::new(search_config.api_key.clone())),
        workflow_config.clone(),
    ))
}

async fn record_deep_search_success(
    handle: &WorkflowRunHandle,
    result: DeepSearchRunResult,
    limits: DeepSearchLimits,
    agent_stats: AgentJsonRunStats,
) {
    let mut template_stats = result.stats.to_template_stats();
    if let Value::Object(stats) = &mut template_stats {
        stats.insert(
            "phase_counts".to_string(),
            serde_json::to_value(&result.phase_counts).unwrap_or(Value::Null),
        );
    }

    project_deep_search_phase_counts(handle, &result.phase_counts, limits).await;
    handle
        .record_artifact(
            "Report",
            Some("completed".to_string()),
            json!(result.report),
        )
        .await;
    handle
        .record_artifact(
            "Sources",
            Some(format!("{} selected", result.selected_sources.len())),
            compact_artifact_payload(json!(result.selected_sources)),
        )
        .await;
    handle
        .record_artifact(
            "Ranked claims",
            Some(format!("{} claims", result.ranked_claims.len())),
            compact_artifact_payload(json!(result.ranked_claims)),
        )
        .await;
    handle
        .record_artifact(
            "Verdicts",
            Some("aggregated".to_string()),
            compact_artifact_payload(json!(result.aggregated_verdicts)),
        )
        .await;
    handle.set_report_summary(Some(result.report.summary)).await;
    handle
        .set_template_stats(compact_template_stats(template_stats))
        .await;
    handle
        .add_agent_stats(
            agent_stats.calls,
            agent_stats.failed_calls,
            0,
            agent_stats.tokens_used,
        )
        .await;
    handle.complete().await;
}

fn compact_artifact_payload(value: Value) -> Value {
    compact_json_value(value, MAX_WORKFLOW_ARTIFACT_ITEMS)
}

fn compact_template_stats(value: Value) -> Value {
    let Ok(serialized) = serde_json::to_vec(&value) else {
        return json!({"truncated": true, "reason": "template_stats_not_serializable"});
    };
    if serialized.len() <= MAX_WORKFLOW_TEMPLATE_STATS_BYTES {
        return value;
    }

    json!({
        "truncated": true,
        "original_bytes": serialized.len(),
        "max_bytes": MAX_WORKFLOW_TEMPLATE_STATS_BYTES,
    })
}

fn compact_json_value(value: Value, max_items: usize) -> Value {
    match value {
        Value::Array(items) => compact_json_array(items, max_items),
        Value::Object(mut object) => {
            for value in object.values_mut() {
                if value.as_array().is_some() {
                    let replaced = std::mem::take(value);
                    *value = compact_json_value(replaced, max_items);
                }
            }
            Value::Object(object)
        }
        other => other,
    }
}

fn compact_json_array(items: Vec<Value>, max_items: usize) -> Value {
    if items.len() <= max_items {
        return Value::Array(items);
    }
    let original_len = items.len();
    let kept = items.into_iter().take(max_items).collect::<Vec<_>>();
    json!({
        "items": kept,
        "truncated_count": original_len.saturating_sub(max_items),
        "max_items": max_items,
    })
}

async fn project_deep_search_phase_counts(
    handle: &WorkflowRunHandle,
    counts: &DeepSearchPhaseCounts,
    limits: DeepSearchLimits,
) {
    handle.update_phase_counts("scope", 1, 0, 0).await;
    handle.complete_phase("scope").await;

    handle
        .update_phase_counts(
            "search",
            counts.searched_angles,
            counts.search_failed,
            counts.search_skipped
                + limits
                    .max_angles
                    .saturating_sub(counts.searched_angles + counts.search_failed),
        )
        .await;
    finish_counted_phase(
        handle,
        "search",
        counts.searched_angles,
        counts.search_failed,
    )
    .await;

    handle
        .update_phase_counts(
            "extract",
            counts.sources_extracted,
            counts.extract_failed,
            counts
                .sources_selected
                .saturating_sub(counts.sources_extracted + counts.extract_failed)
                + counts.extract_skipped,
        )
        .await;
    finish_counted_phase(
        handle,
        "extract",
        counts.sources_extracted,
        counts.extract_failed,
    )
    .await;

    let planned_verdict_votes = counts.claims_ranked * limits.votes_per_claim;
    handle
        .update_phase_counts(
            "verify",
            counts.verdict_votes,
            counts.verdict_failed,
            counts.verdict_skipped
                + planned_verdict_votes
                    .saturating_sub(counts.verdict_votes + counts.verdict_failed),
        )
        .await;
    finish_counted_phase(
        handle,
        "verify",
        counts.verdict_votes,
        counts.verdict_failed,
    )
    .await;

    let synthesized_count = usize::from(counts.synthesized);
    handle
        .update_phase_counts("synthesize", synthesized_count, 0, 1 - synthesized_count)
        .await;
    finish_counted_phase(handle, "synthesize", synthesized_count, 0).await;
}

async fn finish_counted_phase(
    handle: &WorkflowRunHandle,
    phase_id: &str,
    completed_count: usize,
    failed_count: usize,
) {
    if failed_count > 0 {
        handle.fail_phase(phase_id).await;
    } else if completed_count > 0 {
        handle.complete_phase(phase_id).await;
    } else {
        handle.skip_phase(phase_id).await;
    }
}

async fn record_deep_search_failure(
    handle: &WorkflowRunHandle,
    error: &anyhow::Error,
    agent_stats: AgentJsonRunStats,
) {
    let error_message = error.to_string();
    let failed_phase = failed_phase_from_error(error);
    let mut payload = json!({ "error": error_message });
    if let Some(stopped) = deep_search_stopped_from_error(error) {
        payload = json!({
            "error": error_message,
            "stop_reason": stopped.reason().as_str(),
            "phase": stopped.phase_id().as_str(),
            "observed": stopped.observed(),
            "limit": stopped.limit(),
        });
    }

    handle
        .record_artifact("Error", Some("failed".to_string()), payload)
        .await;
    handle
        .set_report_summary(Some(format!("Deep search failed: {error_message}")))
        .await;
    if let Some(stopped) = deep_search_stopped_from_error(error) {
        handle
            .set_stop_reason(Some(workflow_stop_reason_from_deep_search(
                stopped.reason(),
            )))
            .await;
    }
    handle
        .add_agent_stats(
            agent_stats.calls,
            agent_stats.failed_calls,
            0,
            agent_stats.tokens_used,
        )
        .await;
    handle.fail_phase(failed_phase).await;
    skip_unfinished_deep_search_phases(handle, failed_phase).await;
    handle.fail().await;
}

fn failed_phase_from_error(error: &anyhow::Error) -> &'static str {
    if let Some(stopped) = deep_search_stopped_from_error(error) {
        return stopped.phase_id().as_str();
    }
    error
        .downcast_ref::<DeepSearchPhaseError>()
        .map(|phase_error| phase_error.phase_id().as_str())
        .unwrap_or("scope")
}

fn deep_search_stopped_from_error(error: &anyhow::Error) -> Option<&DeepSearchStopped> {
    error
        .chain()
        .find_map(|source| source.downcast_ref::<DeepSearchStopped>())
}

fn workflow_stop_reason_from_deep_search(reason: DeepSearchStopReason) -> WorkflowStopReason {
    match reason {
        DeepSearchStopReason::TokenBudgetExceeded => WorkflowStopReason::TokenBudgetExceeded,
        DeepSearchStopReason::RuntimeExceeded => WorkflowStopReason::RuntimeExceeded,
    }
}

async fn skip_unfinished_deep_search_phases(handle: &WorkflowRunHandle, failed_phase: &str) {
    for phase_id in ["scope", "search", "extract", "verify", "synthesize"] {
        if phase_id != failed_phase {
            handle.skip_phase(phase_id).await;
        }
    }
}

struct WorkflowRunHandleProgressSink {
    handle: WorkflowRunHandle,
}

impl WorkflowRunHandleProgressSink {
    fn new(handle: WorkflowRunHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl WorkflowProgressSink for WorkflowRunHandleProgressSink {
    async fn declare_phase(&self, id: &str, label: &str, planned_count: usize) {
        self.handle.declare_phase(id, label, planned_count).await;
    }

    async fn start_phase(&self, id: &str, label: &str, planned_count: usize) {
        self.handle.start_phase(id, label, planned_count).await;
    }

    async fn update_phase_counts(
        &self,
        id: &str,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    ) {
        self.handle
            .update_phase_counts(id, completed_count, failed_count, skipped_count)
            .await;
    }

    async fn complete_phase(&self, id: &str) {
        self.handle.complete_phase(id).await;
    }

    async fn fail_phase(&self, id: &str) {
        self.handle.fail_phase(id).await;
    }

    async fn cancel_phase(&self, id: &str) {
        self.handle.cancel_phase(id).await;
    }

    async fn skip_phase(&self, id: &str) {
        self.handle.skip_phase(id).await;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AgentJsonRunStats {
    calls: usize,
    failed_calls: usize,
    tokens_used: Option<i64>,
}

struct AppServerAgentJsonRunner {
    services: Arc<AppServerServices>,
    workflow_config: crate::config::AgentConfig,
    root_thread_id: ThreadId,
    run_id: String,
    calls: AtomicUsize,
    failed_calls: AtomicUsize,
    tokens_used: AtomicI64,
    has_token_usage: AtomicBool,
}

impl AppServerAgentJsonRunner {
    fn new(
        services: Arc<AppServerServices>,
        workflow_config: crate::config::AgentConfig,
        root_thread_id: ThreadId,
        run_id: String,
    ) -> Self {
        Self {
            services,
            workflow_config,
            root_thread_id,
            run_id,
            calls: AtomicUsize::new(0),
            failed_calls: AtomicUsize::new(0),
            tokens_used: AtomicI64::new(0),
            has_token_usage: AtomicBool::new(false),
        }
    }

    fn stats(&self) -> AgentJsonRunStats {
        AgentJsonRunStats {
            calls: self.calls.load(Ordering::SeqCst),
            failed_calls: self.failed_calls.load(Ordering::SeqCst),
            tokens_used: self
                .has_token_usage
                .load(Ordering::SeqCst)
                .then(|| self.tokens_used.load(Ordering::SeqCst)),
        }
    }

    fn next_call_index(&self) -> usize {
        self.calls.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn record_tokens(&self, tokens_used: Option<i64>) {
        let Some(tokens_used) = tokens_used else {
            return;
        };
        self.has_token_usage.store(true, Ordering::SeqCst);
        self.tokens_used.fetch_add(tokens_used, Ordering::SeqCst);
    }
}

#[async_trait]
impl AgentJsonRunner for AppServerAgentJsonRunner {
    async fn run_json(&self, request: AgentJsonRequest) -> anyhow::Result<AgentJsonResponse> {
        let call_index = self.next_call_index();
        match self.run_json_inner(request, call_index).await {
            Ok(response) => Ok(response),
            Err(error) => {
                self.failed_calls.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        }
    }
}

#[async_trait]
impl AgentJsonRepair for AppServerAgentJsonRunner {
    async fn repair_json(&self, failure: AgentJsonParseFailure) -> anyhow::Result<String> {
        let request = AgentJsonRequest {
            label: format!(
                "repair-{}",
                sanitize_agent_path_segment(&failure.schema_name)
            ),
            prompt: json_repair_prompt(&failure),
            schema_hint: Some(json!({"type": "object"})),
        };
        let call_index = self.next_call_index();
        match self.run_json_inner(request, call_index).await {
            Ok(response) if response.text.trim().is_empty() && response.value.is_object() => {
                Ok(response.value.to_string())
            }
            Ok(response) => Ok(response.text),
            Err(error) => {
                self.failed_calls.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        }
    }
}

impl AppServerAgentJsonRunner {
    async fn run_json_inner(
        &self,
        request: AgentJsonRequest,
        call_index: usize,
    ) -> anyhow::Result<AgentJsonResponse> {
        let child_thread = thread_processor::start_thread_with_options(
            self.services.as_ref(),
            StartThreadOptions {
                config: self.workflow_config.clone(),
                initial_history: InitialHistory::New,
                thread_source: ThreadSource::Subagent,
                lineage: Some(ThreadLineage {
                    parent_thread_id: self.root_thread_id.clone(),
                    root_thread_id: self.root_thread_id.clone(),
                    depth: 1,
                    agent_path: format!(
                        "/root/workflow-{}/{}-{}",
                        sanitize_agent_path_segment(&self.run_id),
                        call_index,
                        sanitize_agent_path_segment(&request.label)
                    ),
                    agent_type: Some(AgentType::WorkflowResearch),
                    agent_role: Some("workflow_json_runner".to_string()),
                    agent_nickname: Some(request.label.clone()),
                    forked_from_id: None,
                }),
                subagent_control: None,
            },
        )?;

        let child_thread_id = child_thread.thread_id;
        let run_result = turn_processor::run_turn_through_runtime(
            self.services.as_ref(),
            TurnStartParams {
                thread_id: child_thread_id.clone(),
                prompt: request.prompt,
                input: vec![],
                workspace_root: Some(self.workflow_config.workspace_root.display().to_string()),
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await;
        let shutdown_result = self
            .services
            .runtime_loader
            .shutdown_and_remove(&child_thread_id)
            .await;
        let (_thread_id, _workspace_root, final_turn) = run_result?;
        let tokens_used = workflow_child_tokens_used(
            &self.services,
            &self.workflow_config.workspace_root,
            &child_thread_id,
        );
        self.record_tokens(tokens_used);
        shutdown_result.context("failed to shut down workflow child runtime")?;

        let text = final_turn.text.unwrap_or_default();
        let value = parse_json_object::<Value>(&text).unwrap_or(Value::Null);

        Ok(AgentJsonResponse {
            text,
            value,
            tokens_used,
        })
    }
}

fn workflow_child_tokens_used(
    services: &AppServerServices,
    workspace_root: &std::path::Path,
    child_thread_id: &ThreadId,
) -> Option<i64> {
    services
        .runtime_loader
        .runtime_for(child_thread_id)
        .and_then(|runtime| {
            runtime
                .live_view()
                .snapshot
                .token_info
                .map(|info| info.total_token_usage.total_tokens)
        })
        .or_else(|| {
            read_thread_state_from_storage(workspace_root, child_thread_id)
                .ok()
                .flatten()
                .and_then(|stored| stored.snapshot.token_info)
                .map(|info| info.total_token_usage.total_tokens)
        })
}

fn sanitize_agent_path_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::WorkflowPhaseStatus;
    use crate::config::AgentConfig;
    use crate::llm::{LlmClient, LlmRequestOptions, MockLlm};
    use crate::registry::ToolRegistry;
    use crate::resolver::EnvModelResolver;
    use crate::runtime::workflow::templates::deep_search::DeepSearchPhaseId;
    use crate::state::rollout::{rollout_paths, RolloutStore};
    use crate::tools::ToolSpec;
    use crate::types::{AssistantTurn, ConversationMessage, LlmCompletion, TokenUsage};
    use tempfile::tempdir;

    struct TokenUsageLlm {
        text: String,
        tokens: i64,
    }

    #[async_trait]
    impl LlmClient for TokenUsageLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            Ok(LlmCompletion {
                turn: AssistantTurn {
                    text: Some(self.text.clone()),
                    tool_calls: Vec::new(),
                    reasoning: Vec::new(),
                },
                token_usage: Some(TokenUsage {
                    total_tokens: self.tokens,
                    ..TokenUsage::default()
                }),
            })
        }
    }

    #[tokio::test]
    async fn workflow_agent_json_runner_shuts_down_child_runtime_after_call() {
        let dir = tempdir().unwrap();
        let workspace_root = std::fs::canonicalize(dir.path()).unwrap();
        let config = AgentConfig {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root.clone(),
            ..AgentConfig::default()
        };
        let services = Arc::new(AppServerServices::with_llm_and_model_resolver(
            config.clone(),
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some(serde_json::json!({"ok": true}).to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new,
            Arc::new(EnvModelResolver),
        ));
        let runner = AppServerAgentJsonRunner::new(
            services.clone(),
            config,
            ThreadId::new("workflow-root"),
            "workflow_run_test".to_string(),
        );

        let response = runner
            .run_json(AgentJsonRequest {
                label: "scope".to_string(),
                prompt: "Return JSON".to_string(),
                schema_hint: None,
            })
            .await
            .expect("run json");

        assert_eq!(response.value, serde_json::json!({"ok": true}));
        let child_ids = std::fs::read_dir(workspace_root.join(".exagent").join("threads"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(ThreadId::new))
            .collect::<Vec<_>>();
        assert_eq!(child_ids.len(), 1);
        let child_id = &child_ids[0];
        let child_rollout = rollout_paths(&workspace_root, child_id);
        RolloutStore::read_items_blocking(&child_rollout.rollout_path)
            .expect("child rollout remains readable");
        assert!(services.runtime_loader.runtime_for(child_id).is_none());
    }

    #[tokio::test]
    async fn workflow_agent_json_runner_can_repair_json_with_child_turn() {
        let dir = tempdir().unwrap();
        let workspace_root = std::fs::canonicalize(dir.path()).unwrap();
        let config = AgentConfig {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root.clone(),
            ..AgentConfig::default()
        };
        let services = Arc::new(AppServerServices::with_llm_and_model_resolver(
            config.clone(),
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some(serde_json::json!({"ok": true}).to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            }])),
            ToolRegistry::new,
            Arc::new(EnvModelResolver),
        ));
        let runner = AppServerAgentJsonRunner::new(
            services.clone(),
            config,
            ThreadId::new("workflow-root"),
            "workflow_run_test".to_string(),
        );

        let repaired = runner
            .repair_json(AgentJsonParseFailure::new(
                "ScopeOutput",
                "{bad",
                "bad json",
            ))
            .await
            .expect("repair json");

        assert_eq!(repaired, serde_json::json!({"ok": true}).to_string());
        let child_ids = std::fs::read_dir(workspace_root.join(".exagent").join("threads"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(ThreadId::new))
            .collect::<Vec<_>>();
        assert_eq!(child_ids.len(), 1);
        assert!(services.runtime_loader.runtime_for(&child_ids[0]).is_none());
        assert_eq!(runner.stats().calls, 1);
    }

    #[tokio::test]
    async fn workflow_agent_json_runner_records_child_token_usage() {
        let dir = tempdir().unwrap();
        let workspace_root = std::fs::canonicalize(dir.path()).unwrap();
        let config = AgentConfig {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root,
            ..AgentConfig::default()
        };
        let services = Arc::new(AppServerServices::with_llm_and_model_resolver(
            config.clone(),
            Box::new(TokenUsageLlm {
                text: serde_json::json!({"ok": true}).to_string(),
                tokens: 42,
            }),
            ToolRegistry::new,
            Arc::new(EnvModelResolver),
        ));
        let runner = AppServerAgentJsonRunner::new(
            services,
            config,
            ThreadId::new("workflow-root"),
            "workflow_run_test".to_string(),
        );

        let response = runner
            .run_json(AgentJsonRequest {
                label: "scope".to_string(),
                prompt: "Return JSON".to_string(),
                schema_hint: None,
            })
            .await
            .expect("run json");

        assert_eq!(response.tokens_used, Some(42));
        assert_eq!(runner.stats().tokens_used, Some(42));
    }

    #[tokio::test]
    async fn cancelled_template_token_backstops_visible_run_state() {
        let dir = tempdir().unwrap();
        let workspace_root = std::fs::canonicalize(dir.path()).unwrap();
        let config = AgentConfig {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root,
            ..AgentConfig::default()
        };
        let services = Arc::new(AppServerServices::with_llm_and_model_resolver(
            config.clone(),
            Box::new(MockLlm::new(vec![])),
            ToolRegistry::new,
            Arc::new(EnvModelResolver),
        ));
        let handle = WorkflowRunHandle::new(WorkflowRunState::new(
            WorkflowRunId::new("workflow_cancel_backstop"),
            ThreadId::new("thread_cancel_backstop"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: cancelled token".to_string(),
        ));
        let cancellation = WorkflowCancellation::new();
        cancellation.cancel();

        run_deep_research_workflow(
            services,
            config,
            handle.clone(),
            ThreadId::new("thread_cancel_backstop"),
            "workflow_cancel_backstop".to_string(),
            WorkflowPresetId::Quick,
            "Research cancellation backstop".to_string(),
            cancellation,
        )
        .await;

        let view = handle.view().await;
        assert_eq!(view.status, WorkflowRunStatus::Cancelled);
        assert!(view
            .phases
            .iter()
            .all(|phase| phase.status != WorkflowPhaseStatus::Running));
        assert_eq!(view.artifacts.len(), 0);
    }

    #[tokio::test]
    async fn record_failure_does_not_infer_phase_from_plain_error_message() {
        let handle = WorkflowRunHandle::new(WorkflowRunState::new(
            WorkflowRunId::new("workflow_plain_error"),
            ThreadId::new("thread_plain_error"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: plain error".to_string(),
        ));
        handle.start().await;
        handle.declare_phase("scope", "Scope", 1).await;
        handle.declare_phase("search", "Search", 1).await;
        handle.declare_phase("extract", "Extract", 1).await;
        handle.declare_phase("verify", "Verify", 1).await;
        handle.declare_phase("synthesize", "Synthesize", 1).await;

        let error = anyhow::anyhow!("deep search extract phase failed");

        record_deep_search_failure(&handle, &error, AgentJsonRunStats::default()).await;

        let view = handle.view().await;
        assert_eq!(view.status, WorkflowRunStatus::Failed);
        let phase_statuses = view
            .phases
            .iter()
            .map(|phase| (phase.id.as_str(), phase.status))
            .collect::<Vec<_>>();
        assert_eq!(
            phase_statuses,
            vec![
                ("scope", WorkflowPhaseStatus::Failed),
                ("search", WorkflowPhaseStatus::Skipped),
                ("extract", WorkflowPhaseStatus::Skipped),
                ("verify", WorkflowPhaseStatus::Skipped),
                ("synthesize", WorkflowPhaseStatus::Skipped),
            ]
        );
    }

    #[tokio::test]
    async fn record_failure_exposes_typed_stop_reason_on_run_view() {
        let handle = WorkflowRunHandle::new(WorkflowRunState::new(
            WorkflowRunId::new("workflow_stopped"),
            ThreadId::new("thread_stopped"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: stopped".to_string(),
        ));
        handle.start().await;
        handle.declare_phase("scope", "Scope", 1).await;
        handle.declare_phase("search", "Search", 1).await;
        handle.declare_phase("extract", "Extract", 1).await;
        handle.declare_phase("verify", "Verify", 1).await;
        handle.declare_phase("synthesize", "Synthesize", 1).await;

        let error: anyhow::Error = DeepSearchStopped::new(
            DeepSearchPhaseId::Verify,
            DeepSearchStopReason::TokenBudgetExceeded,
            101,
            100,
        )
        .into();

        record_deep_search_failure(&handle, &error, AgentJsonRunStats::default()).await;

        let view = handle.view().await;
        assert_eq!(view.status, WorkflowRunStatus::Failed);
        assert_eq!(
            view.stop_reason,
            Some(WorkflowStopReason::TokenBudgetExceeded)
        );
        assert_eq!(
            view.phases
                .iter()
                .find(|phase| phase.id == "verify")
                .map(|phase| phase.status),
            Some(WorkflowPhaseStatus::Failed)
        );
    }

    #[test]
    fn compact_artifact_payload_caps_large_arrays() {
        let value = compact_artifact_payload(Value::Array(
            (0..(MAX_WORKFLOW_ARTIFACT_ITEMS + 2))
                .map(|index| json!({"index": index}))
                .collect(),
        ));

        assert_eq!(
            value["items"].as_array().expect("items array").len(),
            MAX_WORKFLOW_ARTIFACT_ITEMS
        );
        assert_eq!(value["truncated_count"], json!(2));
    }

    #[test]
    fn compact_template_stats_replaces_oversized_payload() {
        let value = compact_template_stats(json!({
            "large": "x".repeat(MAX_WORKFLOW_TEMPLATE_STATS_BYTES + 1)
        }));

        assert_eq!(value["truncated"], json!(true));
        assert_eq!(value["max_bytes"], json!(MAX_WORKFLOW_TEMPLATE_STATS_BYTES));
    }

    #[test]
    fn workflow_run_id_suffix_must_be_plain_thread_id_segment() {
        let valid =
            workflow_thread_id_from_run_id("workflow_run_thread-123_abc").expect("valid run id");
        assert_eq!(valid.as_str(), "thread-123_abc");

        for run_id in [
            "thread-123",
            "workflow_run_",
            "workflow_run_ ",
            "workflow_run_.",
            "workflow_run_..",
            "workflow_run_thread/child",
            "workflow_run_thread\\child",
            "workflow_run_../thread",
            "workflow_run_thread child",
            "workflow_run_thread.child",
        ] {
            assert!(
                workflow_thread_id_from_run_id(run_id).is_err(),
                "accepted invalid workflow run id: {run_id}"
            );
        }
    }
}
