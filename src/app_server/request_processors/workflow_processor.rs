use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    TurnStartParams, WorkflowCancelParams, WorkflowCancelResponse, WorkflowPresetId,
    WorkflowReadParams, WorkflowReadResponse, WorkflowRunStatus, WorkflowStartParams,
    WorkflowStartResponse, WorkflowTemplateId,
};
use crate::app_server::request_processors::thread_processor::{
    self, InitialHistory, StartThreadOptions,
};
use crate::app_server::request_processors::turn_processor;
use crate::app_server::services::AppServerServices;
use crate::app_server::AppServerError;
use crate::runtime::workflow::templates::deep_search::{
    DeepSearchCancelled, DeepSearchPhaseCounts, DeepSearchPhaseError, DeepSearchRunResult,
    DeepSearchTemplateRunner,
};
use crate::runtime::workflow::{
    parse_json_object, AgentJsonRequest, AgentJsonResponse, AgentJsonRunner, DeepSearchLimits,
    WorkflowCancellation, WorkflowProgressSink, WorkflowRunHandle, WorkflowRunId, WorkflowRunState,
};
use crate::session::{ThreadLineage, ThreadSource};
use crate::types::ThreadId;

const MAX_WORKFLOW_LABEL_QUESTION_CHARS: usize = 120;

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
    let entry = services
        .workflow_runs
        .get(&params.run_id, &config.workspace_root)
        .await?;
    let run = entry.handle.view().await;

    Ok(WorkflowReadResponse { run })
}

pub(in crate::app_server) async fn workflow_cancel(
    services: &AppServerServices,
    params: WorkflowCancelParams,
) -> Result<WorkflowCancelResponse> {
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let entry = services
        .workflow_runs
        .get(&params.run_id, &config.workspace_root)
        .await?;
    let status = entry.handle.view().await.status;
    if !is_terminal(status) {
        entry.cancellation.cancel();
        entry.handle.cancel().await;
    }
    let run = entry.handle.view().await;

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
        services,
        workflow_config,
        root_thread_id,
        run_id.clone(),
    ));
    let template_runner = DeepSearchTemplateRunner::new(limits, runner.clone())
        .with_progress_sink(progress_sink)
        .with_run_label(format!("workflow:{run_id}"))
        .with_cancellation(cancellation.clone());

    match template_runner.run(&question).await {
        Ok(result) => {
            if is_terminal(handle.view().await.status) {
                return;
            }
            record_deep_search_success(&handle, result, limits, runner.stats()).await;
        }
        Err(error) => {
            if error.downcast_ref::<DeepSearchCancelled>().is_some() && cancellation.is_cancelled()
            {
                handle.cancel().await;
                return;
            }
            if is_terminal(handle.view().await.status) {
                return;
            }
            record_deep_search_failure(&handle, &error, runner.stats()).await;
        }
    }
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
            json!(result.selected_sources),
        )
        .await;
    handle
        .record_artifact(
            "Ranked claims",
            Some(format!("{} claims", result.ranked_claims.len())),
            json!(result.ranked_claims),
        )
        .await;
    handle
        .record_artifact(
            "Verdicts",
            Some("aggregated".to_string()),
            json!(result.aggregated_verdicts),
        )
        .await;
    handle.set_report_summary(Some(result.report.summary)).await;
    handle.set_template_stats(template_stats).await;
    handle
        .add_agent_stats(agent_stats.calls, agent_stats.failed_calls, 0, None)
        .await;
    handle.complete().await;
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
            0,
            limits.max_angles.saturating_sub(counts.searched_angles),
        )
        .await;
    finish_counted_phase(handle, "search", counts.searched_angles).await;

    handle
        .update_phase_counts(
            "extract",
            counts.sources_extracted,
            0,
            counts
                .sources_selected
                .saturating_sub(counts.sources_extracted),
        )
        .await;
    finish_counted_phase(handle, "extract", counts.sources_extracted).await;

    let planned_verdict_votes = counts.claims_ranked * limits.votes_per_claim;
    handle
        .update_phase_counts(
            "verify",
            counts.verdict_votes,
            0,
            planned_verdict_votes.saturating_sub(counts.verdict_votes),
        )
        .await;
    finish_counted_phase(handle, "verify", counts.verdict_votes).await;

    let synthesized_count = usize::from(counts.synthesized);
    handle
        .update_phase_counts("synthesize", synthesized_count, 0, 1 - synthesized_count)
        .await;
    finish_counted_phase(handle, "synthesize", synthesized_count).await;
}

async fn finish_counted_phase(handle: &WorkflowRunHandle, phase_id: &str, completed_count: usize) {
    if completed_count > 0 {
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

    handle
        .record_artifact(
            "Error",
            Some("failed".to_string()),
            json!({ "error": error_message }),
        )
        .await;
    handle
        .set_report_summary(Some(format!("Deep search failed: {error_message}")))
        .await;
    handle
        .add_agent_stats(agent_stats.calls, agent_stats.failed_calls, 0, None)
        .await;
    handle.fail_phase(failed_phase).await;
    skip_unfinished_deep_search_phases(handle, failed_phase).await;
    handle.fail().await;
}

fn failed_phase_from_error(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<DeepSearchPhaseError>()
        .map(|phase_error| phase_error.phase_id().as_str())
        .unwrap_or("scope")
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
}

struct AppServerAgentJsonRunner {
    services: Arc<AppServerServices>,
    workflow_config: crate::config::AgentConfig,
    root_thread_id: ThreadId,
    run_id: String,
    calls: AtomicUsize,
    failed_calls: AtomicUsize,
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
        }
    }

    fn stats(&self) -> AgentJsonRunStats {
        AgentJsonRunStats {
            calls: self.calls.load(Ordering::SeqCst),
            failed_calls: self.failed_calls.load(Ordering::SeqCst),
        }
    }

    fn next_call_index(&self) -> usize {
        self.calls.fetch_add(1, Ordering::SeqCst) + 1
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
                    agent_type: None,
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
        shutdown_result.context("failed to shut down workflow child runtime")?;

        let text = final_turn.text.unwrap_or_default();
        let value = parse_json_object::<Value>(&text).unwrap_or(Value::Null);

        Ok(AgentJsonResponse {
            text,
            value,
            tokens_used: None,
        })
    }
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
    use crate::llm::MockLlm;
    use crate::registry::ToolRegistry;
    use crate::resolver::EnvModelResolver;
    use crate::state::rollout::{rollout_paths, RolloutStore};
    use crate::types::AssistantTurn;
    use tempfile::tempdir;

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
}
