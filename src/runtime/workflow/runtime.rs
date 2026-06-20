use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::app_server::protocol::{
    WorkflowArtifactSummary, WorkflowPhaseStatus, WorkflowPhaseView, WorkflowPresetId,
    WorkflowRunStatus, WorkflowRunView, WorkflowStats, WorkflowStopReason, WorkflowTemplateId,
};
use crate::runtime::workflow::artifacts::ArtifactStore;
use crate::runtime::workflow::types::{WorkflowLimits, WorkflowRunId};
use crate::types::ThreadId;

#[async_trait]
pub trait WorkflowTemplate: Send + Sync {
    fn id(&self) -> WorkflowTemplateId;

    async fn run(&self, context: WorkflowContext) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct WorkflowContext {
    pub handle: WorkflowRunHandle,
    pub limits: WorkflowLimits,
}

#[derive(Clone)]
pub struct WorkflowRunHandle {
    state: Arc<Mutex<WorkflowRunState>>,
}

impl WorkflowRunHandle {
    pub fn new(state: WorkflowRunState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub async fn view(&self) -> WorkflowRunView {
        self.state.lock().await.view()
    }

    pub async fn start(&self) {
        self.state.lock().await.start();
    }

    pub async fn declare_phase(
        &self,
        id: impl Into<String>,
        label: impl Into<String>,
        planned_count: usize,
    ) {
        self.state
            .lock()
            .await
            .declare_phase(id, label, planned_count);
    }

    pub async fn start_phase(
        &self,
        id: impl Into<String>,
        label: impl Into<String>,
        planned_count: usize,
    ) {
        self.state
            .lock()
            .await
            .start_phase(id, label, planned_count);
    }

    pub async fn update_phase_counts(
        &self,
        phase_id: &str,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    ) {
        self.state.lock().await.update_phase_counts(
            phase_id,
            completed_count,
            failed_count,
            skipped_count,
        );
    }

    pub async fn complete_phase(&self, phase_id: &str) {
        self.state.lock().await.complete_phase(phase_id);
    }

    pub async fn fail_phase(&self, phase_id: &str) {
        self.state.lock().await.fail_phase(phase_id);
    }

    pub async fn cancel_phase(&self, phase_id: &str) {
        self.state.lock().await.cancel_phase(phase_id);
    }

    pub async fn skip_phase(&self, phase_id: &str) {
        self.state.lock().await.skip_phase(phase_id);
    }

    pub async fn record_artifact(
        &self,
        label: impl Into<String>,
        status: Option<String>,
        payload: Value,
    ) -> Option<WorkflowArtifactSummary> {
        self.state
            .lock()
            .await
            .record_artifact(label, status, payload)
    }

    pub async fn add_agent_stats(
        &self,
        agent_calls: usize,
        failed_agent_calls: usize,
        skipped_agent_calls: usize,
        tokens_used: Option<i64>,
    ) {
        self.state.lock().await.add_agent_stats(
            agent_calls,
            failed_agent_calls,
            skipped_agent_calls,
            tokens_used,
        );
    }

    pub async fn complete(&self) {
        self.state.lock().await.complete();
    }

    pub async fn fail(&self) {
        self.state.lock().await.fail();
    }

    pub async fn cancel(&self) {
        self.state.lock().await.cancel();
    }

    pub async fn set_report_summary(&self, report_summary: Option<String>) {
        self.state.lock().await.set_report_summary(report_summary);
    }

    pub async fn set_template_stats(&self, template_stats: Value) {
        self.state.lock().await.set_template_stats(template_stats);
    }

    pub async fn set_stop_reason(&self, stop_reason: Option<WorkflowStopReason>) {
        self.state.lock().await.set_stop_reason(stop_reason);
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowRunState {
    run_id: WorkflowRunId,
    thread_id: ThreadId,
    template_id: WorkflowTemplateId,
    preset_id: WorkflowPresetId,
    label: String,
    status: WorkflowRunStatus,
    phases: Vec<WorkflowPhaseView>,
    phase_index_by_id: HashMap<String, usize>,
    artifacts: ArtifactStore,
    stats: WorkflowStats,
    report_summary: Option<String>,
    stop_reason: Option<WorkflowStopReason>,
    created_at_ms: i64,
    updated_at_ms: i64,
    started_at_ms: Option<i64>,
    completed_at_ms: Option<i64>,
}

impl WorkflowRunState {
    pub fn new(
        run_id: WorkflowRunId,
        thread_id: ThreadId,
        template_id: WorkflowTemplateId,
        preset_id: WorkflowPresetId,
        label: String,
    ) -> Self {
        let now = current_unix_ms();
        Self {
            run_id,
            thread_id,
            template_id,
            preset_id,
            label,
            status: WorkflowRunStatus::Queued,
            phases: Vec::new(),
            phase_index_by_id: HashMap::new(),
            artifacts: ArtifactStore::default(),
            stats: WorkflowStats {
                agent_calls: 0,
                failed_agent_calls: 0,
                skipped_agent_calls: 0,
                total_artifacts: 0,
                tokens_used: None,
                elapsed_ms: 0,
                template_stats: Value::Null,
            },
            report_summary: None,
            stop_reason: None,
            created_at_ms: now,
            updated_at_ms: now,
            started_at_ms: None,
            completed_at_ms: None,
        }
    }

    pub fn start(&mut self) {
        if self.is_terminal() {
            return;
        }
        let now = self.touch();
        self.status = WorkflowRunStatus::Running;
        self.started_at_ms.get_or_insert(now);
    }

    pub fn declare_phase(
        &mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        planned_count: usize,
    ) {
        if self.is_terminal() {
            return;
        }

        let id = id.into();
        let label = label.into();

        if let Some(index) = self.phase_index_by_id.get(&id).copied() {
            if is_phase_terminal(self.phases[index].status) {
                return;
            }
            let now = self.touch();
            let phase = &mut self.phases[index];
            phase.label = label;
            phase.status = WorkflowPhaseStatus::Pending;
            phase.planned_count = planned_count;
            phase.completed_count = 0;
            phase.failed_count = 0;
            phase.skipped_count = 0;
            phase.started_at_ms = None;
            phase.updated_at_ms = now;
            phase.completed_at_ms = None;
            return;
        }

        let now = self.touch();
        self.phase_index_by_id.insert(id.clone(), self.phases.len());
        self.phases.push(WorkflowPhaseView {
            id,
            label,
            status: WorkflowPhaseStatus::Pending,
            planned_count,
            completed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            started_at_ms: None,
            updated_at_ms: now,
            completed_at_ms: None,
        });
    }

    pub fn start_phase(
        &mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        planned_count: usize,
    ) {
        if self.is_terminal() {
            return;
        }

        let id = id.into();
        let label = label.into();

        if let Some(index) = self.phase_index_by_id.get(&id).copied() {
            if is_phase_terminal(self.phases[index].status) {
                return;
            }
            let now = self.touch();
            let phase = &mut self.phases[index];
            phase.label = label;
            phase.status = WorkflowPhaseStatus::Running;
            phase.planned_count = planned_count;
            phase.started_at_ms.get_or_insert(now);
            phase.updated_at_ms = now;
            return;
        }

        let now = self.touch();
        self.phase_index_by_id.insert(id.clone(), self.phases.len());
        self.phases.push(WorkflowPhaseView {
            id,
            label,
            status: WorkflowPhaseStatus::Running,
            planned_count,
            completed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            started_at_ms: Some(now),
            updated_at_ms: now,
            completed_at_ms: None,
        });
    }

    pub fn update_phase_counts(
        &mut self,
        phase_id: &str,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    ) {
        if self.is_terminal() {
            return;
        }
        let Some(index) = self.phase_index_by_id.get(phase_id).copied() else {
            return;
        };
        if is_phase_terminal(self.phases[index].status) {
            return;
        }
        let now = self.touch();
        let phase = &mut self.phases[index];
        phase.completed_count = completed_count;
        phase.failed_count = failed_count;
        phase.skipped_count = skipped_count;
        phase.updated_at_ms = now;
    }

    pub fn complete_phase(&mut self, phase_id: &str) {
        self.finish_phase(phase_id, WorkflowPhaseStatus::Completed);
    }

    pub fn fail_phase(&mut self, phase_id: &str) {
        self.finish_phase(phase_id, WorkflowPhaseStatus::Failed);
    }

    pub fn cancel_phase(&mut self, phase_id: &str) {
        self.finish_phase(phase_id, WorkflowPhaseStatus::Cancelled);
    }

    pub fn skip_phase(&mut self, phase_id: &str) {
        self.finish_phase(phase_id, WorkflowPhaseStatus::Skipped);
    }

    pub fn record_artifact(
        &mut self,
        label: impl Into<String>,
        status: Option<String>,
        payload: Value,
    ) -> Option<WorkflowArtifactSummary> {
        if self.is_terminal() {
            return None;
        }
        let summary = self.artifacts.record(label, status, payload);
        self.stats.total_artifacts = self.artifacts.len();
        self.touch();
        Some(summary)
    }

    pub fn add_agent_stats(
        &mut self,
        agent_calls: usize,
        failed_agent_calls: usize,
        skipped_agent_calls: usize,
        tokens_used: Option<i64>,
    ) {
        if self.is_terminal() {
            return;
        }
        self.stats.agent_calls += agent_calls;
        self.stats.failed_agent_calls += failed_agent_calls;
        self.stats.skipped_agent_calls += skipped_agent_calls;
        if let Some(tokens) = tokens_used {
            self.stats.tokens_used = Some(self.stats.tokens_used.unwrap_or(0) + tokens);
        }
        self.touch();
    }

    pub fn complete(&mut self) {
        self.finish_run(WorkflowRunStatus::Completed);
    }

    pub fn fail(&mut self) {
        self.finish_run(WorkflowRunStatus::Failed);
    }

    pub fn cancel(&mut self) {
        if self.is_terminal() {
            return;
        }
        self.cancel_non_terminal_phases();
        self.finish_run(WorkflowRunStatus::Cancelled);
    }

    pub fn set_report_summary(&mut self, report_summary: Option<String>) {
        if self.is_terminal() {
            return;
        }
        self.report_summary = report_summary;
        self.touch();
    }

    pub fn set_template_stats(&mut self, template_stats: Value) {
        if self.is_terminal() {
            return;
        }
        self.stats.template_stats = template_stats;
        self.touch();
    }

    pub fn set_stop_reason(&mut self, stop_reason: Option<WorkflowStopReason>) {
        if self.is_terminal() {
            return;
        }
        self.stop_reason = stop_reason;
        self.touch();
    }

    pub fn view(&self) -> WorkflowRunView {
        WorkflowRunView {
            run_id: self.run_id.to_string(),
            thread_id: self.thread_id.clone(),
            template_id: self.template_id.clone(),
            preset_id: self.preset_id,
            label: self.label.clone(),
            status: self.status,
            phases: self.phases.clone(),
            artifacts: self.artifacts.list_summaries(),
            stats: self.stats.clone(),
            report_summary: self.report_summary.clone(),
            stop_reason: self.stop_reason,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            started_at_ms: self.started_at_ms,
            completed_at_ms: self.completed_at_ms,
        }
    }

    fn finish_phase(&mut self, phase_id: &str, status: WorkflowPhaseStatus) {
        if self.is_terminal() {
            return;
        }
        let Some(index) = self.phase_index_by_id.get(phase_id).copied() else {
            return;
        };
        if is_phase_terminal(self.phases[index].status) {
            return;
        }
        let now = self.touch();
        let phase = &mut self.phases[index];
        phase.status = status;
        phase.updated_at_ms = now;
        phase.completed_at_ms = Some(now);
    }

    fn finish_run(&mut self, status: WorkflowRunStatus) {
        if self.is_terminal() {
            return;
        }
        let now = self.touch();
        self.status = status;
        self.completed_at_ms = Some(now);
        let started_at = self.started_at_ms.unwrap_or(self.created_at_ms);
        self.stats.elapsed_ms = now.saturating_sub(started_at);
    }

    fn cancel_non_terminal_phases(&mut self) {
        let now = self.touch();
        for phase in &mut self.phases {
            if is_phase_terminal(phase.status) {
                continue;
            }
            phase.status = WorkflowPhaseStatus::Cancelled;
            phase.updated_at_ms = now;
            phase.completed_at_ms = Some(now);
        }
    }

    fn is_terminal(&self) -> bool {
        is_terminal_status(self.status)
    }

    fn touch(&mut self) -> i64 {
        let now = current_unix_ms();
        self.updated_at_ms = now.max(self.updated_at_ms);
        self.updated_at_ms
    }
}

fn current_unix_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

fn is_terminal_status(status: WorkflowRunStatus) -> bool {
    matches!(
        status,
        WorkflowRunStatus::Completed | WorkflowRunStatus::Failed | WorkflowRunStatus::Cancelled
    )
}

fn is_phase_terminal(status: WorkflowPhaseStatus) -> bool {
    matches!(
        status,
        WorkflowPhaseStatus::Completed
            | WorkflowPhaseStatus::Failed
            | WorkflowPhaseStatus::Skipped
            | WorkflowPhaseStatus::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::{
        WorkflowPhaseStatus, WorkflowPresetId, WorkflowRunStatus, WorkflowTemplateId,
    };
    use crate::runtime::workflow::types::WorkflowRunId;
    use crate::types::ThreadId;
    use serde_json::json;

    #[test]
    fn run_state_projects_phase_artifact_and_stats_view() {
        let mut run = WorkflowRunState::new(
            WorkflowRunId::new("workflow_1"),
            ThreadId::new("thread_1"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: Rust async".to_string(),
        );

        run.start();
        run.start_phase("search", "Search", 3);
        run.update_phase_counts("search", 2, 1, 0);
        let artifact = run
            .record_artifact("Sources", Some("ready".to_string()), json!({"count": 2}))
            .expect("artifact recorded");
        run.add_agent_stats(3, 1, 0, Some(120));
        run.complete_phase("search");
        run.complete();

        let view = run.view();

        assert_eq!(view.run_id, "workflow_1");
        assert_eq!(view.status, WorkflowRunStatus::Completed);
        assert_eq!(view.phases[0].status, WorkflowPhaseStatus::Completed);
        assert_eq!(view.phases[0].completed_count, 2);
        assert_eq!(view.phases[0].failed_count, 1);
        assert_eq!(view.artifacts, vec![artifact]);
        assert_eq!(view.stats.agent_calls, 3);
        assert_eq!(view.stats.failed_agent_calls, 1);
        assert_eq!(view.stats.total_artifacts, 1);
        assert_eq!(view.stats.tokens_used, Some(120));
        assert!(view.started_at_ms.is_some());
        assert!(view.completed_at_ms.is_some());
    }

    #[tokio::test]
    async fn run_handle_exposes_template_mutation_surface() {
        let handle = WorkflowRunHandle::new(WorkflowRunState::new(
            WorkflowRunId::new("workflow_2"),
            ThreadId::new("thread_2"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Standard,
            "Deep research: workflows".to_string(),
        ));

        handle.start().await;
        handle.start_phase("verify", "Verify", 4).await;
        handle.update_phase_counts("verify", 3, 1, 0).await;
        handle.complete_phase("verify").await;
        handle
            .record_artifact("Votes", None, json!({"accepted": 3}))
            .await;
        handle.add_agent_stats(4, 1, 0, Some(400)).await;
        handle
            .set_report_summary(Some("Three claims survived.".to_string()))
            .await;
        handle.set_template_stats(json!({"survived": 3})).await;
        handle.complete().await;

        let view = handle.view().await;
        assert_eq!(view.status, WorkflowRunStatus::Completed);
        assert_eq!(view.phases[0].status, WorkflowPhaseStatus::Completed);
        assert_eq!(view.phases[0].completed_count, 3);
        assert_eq!(view.stats.agent_calls, 4);
        assert_eq!(view.stats.failed_agent_calls, 1);
        assert_eq!(view.stats.tokens_used, Some(400));
        assert_eq!(view.stats.template_stats, json!({"survived": 3}));
        assert_eq!(
            view.report_summary.as_deref(),
            Some("Three claims survived.")
        );
    }

    #[tokio::test]
    async fn cancelled_run_ignores_later_background_mutations() {
        let handle = WorkflowRunHandle::new(WorkflowRunState::new(
            WorkflowRunId::new("workflow_cancelled"),
            ThreadId::new("thread_cancelled"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: cancellation".to_string(),
        ));

        handle.start().await;
        handle.start_phase("scope", "Scope", 1).await;
        handle.declare_phase("search", "Search", 3).await;
        handle.update_phase_counts("scope", 1, 0, 0).await;
        handle.cancel().await;
        let cancelled = handle.view().await;
        assert_eq!(cancelled.status, WorkflowRunStatus::Cancelled);
        assert_eq!(
            cancelled
                .phases
                .iter()
                .map(|phase| (phase.id.as_str(), phase.status))
                .collect::<Vec<_>>(),
            vec![
                ("scope", WorkflowPhaseStatus::Cancelled),
                ("search", WorkflowPhaseStatus::Cancelled),
            ]
        );

        handle.start().await;
        handle.start_phase("search", "Search", 3).await;
        handle.update_phase_counts("scope", 2, 0, 0).await;
        handle.complete_phase("scope").await;
        handle.fail_phase("search").await;
        handle.skip_phase("extract").await;
        handle.cancel_phase("verify").await;
        handle
            .record_artifact("Report", Some("late".to_string()), json!({"late": true}))
            .await;
        handle.add_agent_stats(9, 8, 7, Some(600)).await;
        handle
            .set_report_summary(Some("late report".to_string()))
            .await;
        handle.set_template_stats(json!({"late": true})).await;
        handle.complete().await;
        handle.fail().await;
        handle.cancel().await;

        assert_eq!(handle.view().await, cancelled);
    }

    #[test]
    fn declared_phase_starts_pending_until_explicitly_started() {
        let mut run = WorkflowRunState::new(
            WorkflowRunId::new("workflow_pending"),
            ThreadId::new("thread_pending"),
            WorkflowTemplateId::DeepResearch,
            WorkflowPresetId::Quick,
            "Deep research: pending".to_string(),
        );

        run.start();
        run.declare_phase("search", "Search", 3);

        let pending = run.view();
        assert_eq!(pending.phases[0].status, WorkflowPhaseStatus::Pending);
        assert_eq!(pending.phases[0].started_at_ms, None);
        assert_eq!(pending.phases[0].completed_at_ms, None);

        run.start_phase("search", "Search", 3);

        let running = run.view();
        assert_eq!(running.phases[0].status, WorkflowPhaseStatus::Running);
        assert!(running.phases[0].started_at_ms.is_some());
    }
}
