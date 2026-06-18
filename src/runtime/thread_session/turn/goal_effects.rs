use anyhow::{anyhow, Result};

use super::super::{LiveEventSink, ThreadEventRecorder, ThreadSession};
use crate::agent::Agent;
use crate::events::RuntimeEventKind;
use crate::llm::LlmRequestOptions;
use crate::runtime::context::ContextManager;
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEffect};
use crate::session::ThreadSnapshot;
use crate::state::rollout::RolloutItem;
use crate::types::{ConversationMessage, TurnId};

impl ThreadSession {
    pub(crate) async fn handle_goal_runtime_effect(
        &mut self,
        effect: GoalRuntimeEffect,
    ) -> Result<bool> {
        let should_check_goal_continuation = !matches!(effect, GoalRuntimeEffect::None);
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            None,
            effect,
        )
        .await?;
        Ok(should_check_goal_continuation)
    }
}

pub(super) async fn record_goal_turn_started_marker(
    goal_runtime: &GoalRuntime,
    recorder: &mut ThreadEventRecorder,
    snapshot: &ThreadSnapshot,
    turn_id: &TurnId,
) -> Result<()> {
    if let Some(goal_id) = goal_runtime
        .active_goal_id_for_turn(&snapshot.thread_id, turn_id)
        .await
    {
        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::ThreadGoalTurnStarted { goal_id },
        )?;
    }
    Ok(())
}

pub(super) async fn apply_goal_effect(
    agent: Option<&Agent>,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: Option<&TurnId>,
    effect: GoalRuntimeEffect,
) -> Result<()> {
    match effect {
        GoalRuntimeEffect::None | GoalRuntimeEffect::ScheduleContinuation => Ok(()),
        GoalRuntimeEffect::EmitUpdated(goal) => {
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndGoalReport { goal, report } => {
            let report = finalize_goal_report(agent, rollout_store, report).await;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalReport { report },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitCleared(thread_id) => {
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalCleared { thread_id },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitModeUpdated {
            thread_id,
            goal_id,
            mode,
        } => {
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalModeUpdated {
                    thread_id,
                    goal_id,
                    mode,
                },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
            goal,
            source,
            content,
        } => {
            let message = context_manager.record_persistent_internal_context(source, content);
            context_manager.sync_snapshot(snapshot);
            if let Some(turn_id) = turn_id {
                rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
                    turn_id.clone(),
                    message,
                )])?;
            }
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
            goal,
            source,
            content,
            report,
        } => {
            let message = context_manager.record_persistent_internal_context(source, content);
            context_manager.sync_snapshot(snapshot);
            if let Some(turn_id) = turn_id {
                rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
                    turn_id.clone(),
                    message,
                )])?;
            }
            let report = finalize_goal_report(agent, rollout_store, report).await;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalReport { report },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndContinuationSuppressed { goal, reason } => {
            let goal_id = goal.goal_id.clone();
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalContinuationSuppressed { goal_id, reason },
            )?;
            Ok(())
        }
    }
}

async fn finalize_goal_report(
    agent: Option<&Agent>,
    rollout_store: &crate::state::rollout::RolloutStore,
    mut report: crate::app_server::protocol::ThreadGoalReport,
) -> crate::app_server::protocol::ThreadGoalReport {
    let rollout_items =
        crate::state::rollout::RolloutStore::read_items_blocking(rollout_store.path())
            .unwrap_or_default();
    let events = crate::state::rollout::events_from_rollout_items(&rollout_items);
    // This rollout is scoped to the current thread. RuntimeOverlay keeps only
    // active unresolved approvals, so resolved and interrupted approvals are not counted.
    report.pending_approvals_count =
        crate::runtime::thread_session::RuntimeOverlay::from_events(&events)
            .pending_approvals
            .len();
    if let Some(agent) = agent {
        if let Ok(summary) = sample_goal_report_summary(agent, &report).await {
            report.summary = summary;
            return report;
        }
    }
    if report.summary.trim().is_empty() {
        report.summary = fallback_goal_report_summary(&report);
    }
    report
}

async fn sample_goal_report_summary(
    agent: &Agent,
    report: &crate::app_server::protocol::ThreadGoalReport,
) -> Result<String> {
    let prompt = vec![ConversationMessage::user(
        crate::runtime::goal::prompts::goal_report_summary_prompt(report),
    )];
    let completion = agent
        .sample_assistant_turn(
            &prompt,
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: agent.config().thinking_mode,
                reasoning_capabilities: None,
            },
        )
        .await?;
    let summary = completion.turn.text.unwrap_or_default().trim().to_string();
    if summary.is_empty() {
        return Err(anyhow!("empty goal report summary"));
    }
    Ok(summary)
}

pub(super) fn changed_files_for_goal_report(
    tool_name: &str,
    result: &crate::types::ToolResult,
) -> Vec<String> {
    if result.status != crate::types::ToolStatus::Success {
        return Vec::new();
    }
    let Some(meta) = result.meta.as_ref() else {
        return Vec::new();
    };
    let files = match tool_name {
        "apply_patch" => meta
            .get("changed_files")
            .and_then(serde_json::Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "write_file" => ["normalized_path", "requested_path", "path"]
            .iter()
            .find_map(|key| meta.get(*key).and_then(serde_json::Value::as_str))
            .map(|file| vec![file.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut deduped = Vec::new();
    for file in files {
        if !deduped.iter().any(|existing| existing == &file) {
            deduped.push(file);
        }
    }
    deduped
}

fn fallback_goal_report_summary(report: &crate::app_server::protocol::ThreadGoalReport) -> String {
    format!(
        "Goal finished with status {} after {} turn(s).",
        goal_report_status_label(report.final_status),
        report.turns_run
    )
}

fn goal_report_status_label(status: crate::app_server::protocol::ThreadGoalStatus) -> &'static str {
    match status {
        crate::app_server::protocol::ThreadGoalStatus::Active => "active",
        crate::app_server::protocol::ThreadGoalStatus::Paused => "paused",
        crate::app_server::protocol::ThreadGoalStatus::Blocked => "blocked",
        crate::app_server::protocol::ThreadGoalStatus::UsageLimited => "usage_limited",
        crate::app_server::protocol::ThreadGoalStatus::BudgetLimited => "budget_limited",
        crate::app_server::protocol::ThreadGoalStatus::Complete => "complete",
    }
}
