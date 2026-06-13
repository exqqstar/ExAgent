use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

use crate::app_server::protocol::{
    ThreadGoal, ThreadGoalReport, ThreadGoalReportOpenQuestion, ThreadGoalReviewRejectCategory,
    ThreadGoalReviewStatus, ThreadGoalReviewSummary, ThreadGoalStatus,
};
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::index_db::{
    GoalAccountingMode, GoalAccountingOutcome, GoalUpdate, IndexDb, ThreadGoalRecord,
    ThreadGoalStatusRecord,
};
use crate::runtime::forge::escalation::{decision_for_goal, ForgeEscalationDecision};
use crate::runtime::forge::open_questions::OpenQuestionStore;
use crate::runtime::forge::review::{
    ReviewRejectCategory, ReviewStatus, ReviewStore, ReviewTicket,
};
use crate::types::{ThreadId, TokenUsage, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalTurnTrigger {
    User,
    GoalContinuation,
}

pub(crate) enum GoalRuntimeEvent<'a> {
    TurnStarted {
        thread_id: &'a ThreadId,
        turn_id: &'a TurnId,
        trigger: GoalTurnTrigger,
        token_usage: TokenUsage,
    },
    ToolCompleted {
        thread_id: &'a ThreadId,
        turn_id: &'a TurnId,
        tool_name: &'a str,
        token_usage: TokenUsage,
        changed_files: Vec<String>,
    },
    ToolCompletedGoal {
        thread_id: &'a ThreadId,
        turn_id: &'a TurnId,
        token_usage: TokenUsage,
    },
    TurnFinished {
        thread_id: &'a ThreadId,
        turn_id: &'a TurnId,
        turn_completed: bool,
        token_usage: TokenUsage,
        assistant_had_activity: bool,
    },
    ExternalMutationStarting {
        thread_id: &'a ThreadId,
        turn_id: Option<&'a TurnId>,
    },
    ExternalSet {
        thread_id: &'a ThreadId,
        goal: ThreadGoal,
        previous_goal: Option<ThreadGoal>,
    },
    ExternalClear {
        thread_id: &'a ThreadId,
    },
    ThreadResumed {
        thread_id: &'a ThreadId,
        restored_events: &'a [RuntimeEvent],
    },
    MaybeContinueIfIdle {
        thread_id: &'a ThreadId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalRuntimeEffect {
    None,
    EmitUpdated(ThreadGoal),
    EmitCleared(ThreadId),
    EmitUpdatedAndGoalReport {
        goal: ThreadGoal,
        report: ThreadGoalReport,
    },
    EmitUpdatedAndInjectPersistentContext {
        goal: ThreadGoal,
        source: &'static str,
        content: String,
    },
    EmitUpdatedInjectContextAndGoalReport {
        goal: ThreadGoal,
        source: &'static str,
        content: String,
        report: ThreadGoalReport,
    },
    EmitUpdatedAndContinuationSuppressed {
        goal: ThreadGoal,
        reason: String,
    },
    ScheduleContinuation,
}

#[derive(Clone)]
pub(crate) struct GoalRuntime {
    db: IndexDb,
    state: Arc<Mutex<GoalRuntimeState>>,
    accounting_lock: Arc<Semaphore>,
    continuation_lock: Arc<Semaphore>,
}

#[derive(Clone)]
pub struct GoalToolApi {
    runtime: Arc<GoalRuntime>,
}

impl GoalToolApi {
    pub(crate) fn new(runtime: Arc<GoalRuntime>) -> Self {
        Self { runtime }
    }

    pub(crate) async fn get_goal(
        &self,
        thread_id: &ThreadId,
    ) -> anyhow::Result<Option<ThreadGoal>> {
        self.runtime.get_goal(thread_id).await
    }

    pub(crate) async fn create_goal(
        &self,
        thread_id: &ThreadId,
        objective: String,
        token_budget: Option<i64>,
    ) -> anyhow::Result<ThreadGoal> {
        self.runtime
            .create_goal(thread_id, objective, token_budget)
            .await
    }

    pub(crate) async fn update_goal(
        &self,
        thread_id: &ThreadId,
        status: ThreadGoalStatus,
    ) -> anyhow::Result<ThreadGoal> {
        self.runtime
            .update_goal_status_from_tool(thread_id, status)
            .await
    }

    pub(crate) async fn account_update_goal_tool(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
    ) -> anyhow::Result<()> {
        let _ = self
            .runtime
            .apply(GoalRuntimeEvent::ToolCompletedGoal {
                thread_id,
                turn_id,
                token_usage: TokenUsage::default(),
            })
            .await?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct GoalRuntimeState {
    turn_accounting: HashMap<ThreadId, GoalTurnAccountingSnapshot>,
    wall_clock: HashMap<ThreadId, GoalWallClockAccountingSnapshot>,
    budget_limit_reported_goal_ids: HashSet<String>,
    goal_turn_counts: HashMap<String, i64>,
    goal_changed_files: HashMap<String, Vec<String>>,
    reported_goal_statuses: HashSet<String>,
    pending_status_effects: HashMap<ThreadId, GoalRuntimeEffect>,
    forge_escalation_notified_keys: HashSet<String>,
}

#[derive(Debug, Clone)]
struct GoalTurnAccountingSnapshot {
    turn_id: TurnId,
    trigger: GoalTurnTrigger,
    last_accounted_token_usage: TokenUsage,
    active_goal_id: Option<String>,
    counted_autonomous_activity: bool,
}

#[derive(Debug, Clone)]
struct GoalWallClockAccountingSnapshot {
    active_goal_id: Option<String>,
    last_accounted_at: Instant,
}

impl GoalRuntime {
    pub(crate) fn new(db: IndexDb) -> Self {
        Self {
            db,
            state: Arc::new(Mutex::new(GoalRuntimeState::default())),
            accounting_lock: Arc::new(Semaphore::new(1)),
            continuation_lock: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) async fn apply(
        &self,
        event: GoalRuntimeEvent<'_>,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        match event {
            GoalRuntimeEvent::TurnStarted {
                thread_id,
                turn_id,
                trigger,
                token_usage,
            } => {
                self.turn_started(thread_id, turn_id, trigger, token_usage)
                    .await
            }
            GoalRuntimeEvent::ToolCompleted {
                thread_id,
                turn_id,
                tool_name,
                token_usage,
                changed_files,
            } => {
                self.record_changed_files_for_current_goal(thread_id, turn_id, changed_files)
                    .await;
                let effect = self
                    .account_turn_progress(thread_id, turn_id, token_usage, true, false)
                    .await?;
                if tool_name != "update_goal" {
                    self.reset_suppression_for_current_goal(thread_id).await?;
                }
                Ok(effect)
            }
            GoalRuntimeEvent::ToolCompletedGoal {
                thread_id,
                turn_id,
                token_usage,
            } => {
                self.account_turn_progress(thread_id, turn_id, token_usage, false, true)
                    .await?;
                if let Some(effect) = self
                    .state
                    .lock()
                    .await
                    .pending_status_effects
                    .remove(thread_id)
                {
                    return Ok(effect);
                }
                Ok(GoalRuntimeEffect::None)
            }
            GoalRuntimeEvent::TurnFinished {
                thread_id,
                turn_id,
                turn_completed,
                token_usage,
                assistant_had_activity,
            } => {
                self.turn_finished(
                    thread_id,
                    turn_id,
                    turn_completed,
                    token_usage,
                    assistant_had_activity,
                )
                .await
            }
            GoalRuntimeEvent::ExternalMutationStarting { thread_id, turn_id } => {
                self.external_mutation_starting(thread_id, turn_id).await
            }
            GoalRuntimeEvent::ExternalSet {
                thread_id,
                goal,
                previous_goal,
            } => self.external_set(thread_id, goal, previous_goal).await,
            GoalRuntimeEvent::ExternalClear { thread_id } => self.external_clear(thread_id).await,
            GoalRuntimeEvent::ThreadResumed {
                thread_id,
                restored_events,
            } => self.thread_resumed(thread_id, restored_events).await,
            GoalRuntimeEvent::MaybeContinueIfIdle { thread_id } => {
                self.maybe_continue_if_idle(thread_id).await
            }
        }
    }

    pub(crate) async fn active_goal_snapshot(
        &self,
        thread_id: &ThreadId,
    ) -> anyhow::Result<Option<ThreadGoal>> {
        let Some(goal) = self.db.get_thread_goal(thread_id).await? else {
            return Ok(None);
        };
        if goal.status != ThreadGoalStatusRecord::Active {
            return Ok(None);
        }
        Ok(Some(thread_goal_from_record(goal)))
    }

    pub(crate) async fn create_goal(
        &self,
        thread_id: &ThreadId,
        objective: String,
        token_budget: Option<i64>,
    ) -> anyhow::Result<ThreadGoal> {
        let goal = self
            .db
            .insert_thread_goal(thread_id, &objective, token_budget)
            .await?
            .ok_or_else(|| anyhow::anyhow!("thread already has a goal"))?;
        let goal = thread_goal_from_record(goal);
        self.mark_active_goal(thread_id, Some(goal.goal_id.clone()))
            .await;
        Ok(goal)
    }

    pub(crate) async fn update_goal_status_effect(
        &self,
        thread_id: &ThreadId,
        status: ThreadGoalStatus,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let previous = self.db.get_thread_goal(thread_id).await?;
        let status = status_record_from_protocol(status);
        let goal = self
            .db
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(status),
                    token_budget: None,
                    expected_goal_id: None,
                },
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("thread has no current goal"))?;
        let goal = thread_goal_from_record(goal);
        self.mark_active_goal(
            thread_id,
            (goal.status == ThreadGoalStatus::Active).then_some(goal.goal_id.clone()),
        )
        .await;
        Ok(self
            .goal_updated_effect(goal, previous.map(thread_goal_from_record))
            .await)
    }

    async fn update_goal_status_from_tool(
        &self,
        thread_id: &ThreadId,
        status: ThreadGoalStatus,
    ) -> anyhow::Result<ThreadGoal> {
        let effect = self.update_goal_status_effect(thread_id, status).await?;
        let goal = goal_from_effect(&effect).clone();
        let mut state = self.state.lock().await;
        state
            .pending_status_effects
            .insert(thread_id.clone(), effect);
        Ok(goal)
    }

    pub(crate) async fn get_goal(
        &self,
        thread_id: &ThreadId,
    ) -> anyhow::Result<Option<ThreadGoal>> {
        self.db
            .get_thread_goal(thread_id)
            .await
            .map(|goal| goal.map(thread_goal_from_record))
    }

    pub(crate) async fn active_goal_id_for_turn(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
    ) -> Option<String> {
        let state = self.state.lock().await;
        state
            .turn_accounting
            .get(thread_id)
            .filter(|snapshot| &snapshot.turn_id == turn_id)
            .and_then(|snapshot| snapshot.active_goal_id.clone())
    }

    async fn turn_started(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        trigger: GoalTurnTrigger,
        token_usage: TokenUsage,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let _permit = self.accounting_lock.acquire().await?;
        let goal = self.db.get_thread_goal(thread_id).await?;
        let active_goal_id = goal
            .as_ref()
            .filter(|goal| goal.status == ThreadGoalStatusRecord::Active)
            .map(|goal| goal.goal_id.clone());
        if trigger == GoalTurnTrigger::User {
            let _ = self
                .db
                .reset_thread_goal_continuation_suppression(thread_id, active_goal_id.as_deref())
                .await?;
        }
        self.mark_active_goal_locked(thread_id, active_goal_id.clone())
            .await;
        let mut state = self.state.lock().await;
        if let Some(goal_id) = active_goal_id.as_ref() {
            *state.goal_turn_counts.entry(goal_id.clone()).or_insert(0) += 1;
        }
        state.turn_accounting.insert(
            thread_id.clone(),
            GoalTurnAccountingSnapshot {
                turn_id: turn_id.clone(),
                trigger,
                last_accounted_token_usage: token_usage,
                active_goal_id,
                counted_autonomous_activity: false,
            },
        );
        Ok(GoalRuntimeEffect::None)
    }

    async fn account_turn_progress(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        token_usage: TokenUsage,
        mark_autonomous_activity: bool,
        suppress_budget_steering: bool,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let _permit = self.accounting_lock.acquire().await?;
        self.account_turn_progress_locked(
            thread_id,
            turn_id,
            token_usage,
            mark_autonomous_activity,
            suppress_budget_steering,
        )
        .await
    }

    async fn account_turn_progress_locked(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        token_usage: TokenUsage,
        mark_autonomous_activity: bool,
        suppress_budget_steering: bool,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let mut state = self.state.lock().await;
        let Some(snapshot) = state.turn_accounting.get_mut(thread_id) else {
            return Ok(GoalRuntimeEffect::None);
        };
        if &snapshot.turn_id != turn_id {
            return Ok(GoalRuntimeEffect::None);
        }
        let Some(goal_id) = snapshot.active_goal_id.clone() else {
            snapshot.last_accounted_token_usage = token_usage;
            return Ok(GoalRuntimeEffect::None);
        };
        let token_delta = token_delta(&snapshot.last_accounted_token_usage, &token_usage);
        let time_delta_seconds = time_delta_since_last_accounting_locked(&mut state, thread_id);
        drop(state);

        let outcome = self
            .db
            .account_thread_goal_usage(
                thread_id,
                time_delta_seconds,
                token_delta,
                GoalAccountingMode::ActiveOnly,
                Some(&goal_id),
            )
            .await?;

        let mut state = self.state.lock().await;
        if let Some(snapshot) = state.turn_accounting.get_mut(thread_id) {
            snapshot.last_accounted_token_usage = token_usage;
            if mark_autonomous_activity || token_delta > 0 {
                snapshot.counted_autonomous_activity = true;
            }
        }
        drop(state);

        match outcome {
            GoalAccountingOutcome::Updated(goal)
                if goal.status == ThreadGoalStatusRecord::BudgetLimited
                    && !suppress_budget_steering =>
            {
                self.budget_limit_effect(goal).await
            }
            GoalAccountingOutcome::Updated(goal) => Ok(GoalRuntimeEffect::EmitUpdated(
                thread_goal_from_record(goal),
            )),
            GoalAccountingOutcome::Unchanged(_) => Ok(GoalRuntimeEffect::None),
        }
    }

    async fn turn_finished(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        _turn_completed: bool,
        token_usage: TokenUsage,
        assistant_had_activity: bool,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let _permit = self.accounting_lock.acquire().await?;
        let _ = self
            .account_turn_progress_locked(
                thread_id,
                turn_id,
                token_usage,
                assistant_had_activity,
                false,
            )
            .await?;
        let snapshot = {
            let mut state = self.state.lock().await;
            state.turn_accounting.remove(thread_id)
        };
        let Some(snapshot) = snapshot else {
            return Ok(GoalRuntimeEffect::None);
        };
        let Some(goal_id) = snapshot.active_goal_id else {
            return Ok(GoalRuntimeEffect::None);
        };
        let Some(goal) = self.db.get_thread_goal(thread_id).await? else {
            return Ok(GoalRuntimeEffect::None);
        };
        if goal.goal_id != goal_id || goal.status != ThreadGoalStatusRecord::Active {
            return Ok(GoalRuntimeEffect::None);
        }
        if snapshot.trigger == GoalTurnTrigger::GoalContinuation
            && !snapshot.counted_autonomous_activity
        {
            let Some(updated) = self
                .db
                .suppress_thread_goal_continuation(thread_id, &goal.goal_id, turn_id)
                .await?
            else {
                return Ok(GoalRuntimeEffect::None);
            };
            return Ok(GoalRuntimeEffect::EmitUpdatedAndContinuationSuppressed {
                goal: thread_goal_from_record(updated),
                reason: "goal continuation produced no counted autonomous activity".to_string(),
            });
        }
        Ok(GoalRuntimeEffect::ScheduleContinuation)
    }

    async fn external_mutation_starting(
        &self,
        thread_id: &ThreadId,
        turn_id: Option<&TurnId>,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let _permit = self.accounting_lock.acquire().await?;
        if let Some(turn_id) = turn_id {
            let token_usage = {
                let state = self.state.lock().await;
                state
                    .turn_accounting
                    .get(thread_id)
                    .filter(|snapshot| &snapshot.turn_id == turn_id)
                    .map(|snapshot| snapshot.last_accounted_token_usage.clone())
                    .unwrap_or_default()
            };
            let _ = self
                .account_turn_progress_locked(thread_id, turn_id, token_usage, false, true)
                .await?;
            return Ok(GoalRuntimeEffect::None);
        }

        let goal_id = {
            let state = self.state.lock().await;
            state
                .wall_clock
                .get(thread_id)
                .and_then(|snapshot| snapshot.active_goal_id.clone())
        };
        let Some(goal_id) = goal_id else {
            return Ok(GoalRuntimeEffect::None);
        };
        let time_delta_seconds = {
            let mut state = self.state.lock().await;
            time_delta_since_last_accounting_locked(&mut state, thread_id)
        };
        let outcome = self
            .db
            .account_thread_goal_usage(
                thread_id,
                time_delta_seconds,
                0,
                GoalAccountingMode::ActiveOnly,
                Some(&goal_id),
            )
            .await?;
        Ok(match outcome {
            GoalAccountingOutcome::Updated(goal) => {
                GoalRuntimeEffect::EmitUpdated(thread_goal_from_record(goal))
            }
            GoalAccountingOutcome::Unchanged(_) => GoalRuntimeEffect::None,
        })
    }

    async fn external_set(
        &self,
        thread_id: &ThreadId,
        goal: ThreadGoal,
        previous_goal: Option<ThreadGoal>,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let active = goal.status == ThreadGoalStatus::Active;
        let _ = self
            .db
            .reset_thread_goal_continuation_suppression(thread_id, Some(&goal.goal_id))
            .await?;
        self.mark_active_goal(thread_id, active.then_some(goal.goal_id.clone()))
            .await;
        if previous_goal
            .as_ref()
            .is_some_and(|previous| previous.objective != goal.objective)
        {
            let content = crate::runtime::goal::prompts::objective_updated_prompt(&goal);
            let final_effect = self.goal_updated_effect(goal.clone(), previous_goal).await;
            if let GoalRuntimeEffect::EmitUpdatedAndGoalReport { report, .. } = final_effect {
                return Ok(GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
                    goal,
                    source: "goal_objective_updated",
                    content,
                    report,
                });
            }
            return Ok(GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
                goal,
                source: "goal_objective_updated",
                content,
            });
        }
        if active && !self.has_active_turn(thread_id).await {
            return Ok(GoalRuntimeEffect::ScheduleContinuation);
        }
        Ok(self.goal_updated_effect(goal, previous_goal).await)
    }

    async fn external_clear(&self, thread_id: &ThreadId) -> anyhow::Result<GoalRuntimeEffect> {
        let mut state = self.state.lock().await;
        state.turn_accounting.remove(thread_id);
        state.wall_clock.remove(thread_id);
        Ok(GoalRuntimeEffect::EmitCleared(thread_id.clone()))
    }

    async fn thread_resumed(
        &self,
        thread_id: &ThreadId,
        restored_events: &[RuntimeEvent],
    ) -> anyhow::Result<GoalRuntimeEffect> {
        self.rebuild_report_facts_from_events(thread_id, restored_events)
            .await;
        let goal = self.db.get_thread_goal(thread_id).await?;
        let active_goal_id = goal
            .as_ref()
            .filter(|goal| goal.status == ThreadGoalStatusRecord::Active)
            .map(|goal| goal.goal_id.clone());
        self.mark_active_goal(thread_id, active_goal_id.clone())
            .await;
        if goal.as_ref().is_some_and(|goal| {
            goal.status == ThreadGoalStatusRecord::Active && !goal.continuation_suppressed
        }) {
            return Ok(GoalRuntimeEffect::ScheduleContinuation);
        }
        Ok(GoalRuntimeEffect::None)
    }

    async fn maybe_continue_if_idle(
        &self,
        thread_id: &ThreadId,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let _permit = self.continuation_lock.acquire().await?;
        if self.has_active_turn(thread_id).await {
            return Ok(GoalRuntimeEffect::None);
        }
        let Some(goal) = self.db.get_thread_goal(thread_id).await? else {
            return Ok(GoalRuntimeEffect::None);
        };
        if goal.status == ThreadGoalStatusRecord::Active && !goal.continuation_suppressed {
            if let Some(effect) = self.forge_escalation_effect(thread_id, &goal).await? {
                return Ok(effect);
            }
            return Ok(GoalRuntimeEffect::ScheduleContinuation);
        }
        Ok(GoalRuntimeEffect::None)
    }

    async fn forge_escalation_effect(
        &self,
        thread_id: &ThreadId,
        goal: &ThreadGoalRecord,
    ) -> anyhow::Result<Option<GoalRuntimeEffect>> {
        let review_store = ReviewStore::new(self.db.clone());
        let question_store = OpenQuestionStore::new(self.db.clone());
        match decision_for_goal(&review_store, &question_store, &goal.goal_id).await? {
            ForgeEscalationDecision::None => Ok(None),
            ForgeEscalationDecision::ActiveGuidance { key, content } => {
                if !self.mark_forge_escalation_notified(key).await {
                    return Ok(None);
                }
                Ok(Some(
                    GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
                        goal: thread_goal_from_record(goal.clone()),
                        source: "forge_review_guidance",
                        content,
                    },
                ))
            }
            ForgeEscalationDecision::PauseForQuestions { key, content } => {
                if !self.mark_forge_escalation_notified(key).await {
                    return Ok(None);
                }
                let updated = self
                    .update_goal_status_for_forge_escalation(
                        thread_id,
                        &goal.goal_id,
                        ThreadGoalStatusRecord::Paused,
                    )
                    .await?;
                let content =
                    format!("{content}\n\nGoal paused awaiting answers to open questions.");
                if let Some(report) = self.report_for_forge_pause(&updated).await {
                    return Ok(Some(
                        GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
                            goal: updated,
                            source: "forge_open_questions_paused",
                            content,
                            report,
                        },
                    ));
                }
                Ok(Some(
                    GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
                        goal: updated,
                        source: "forge_open_questions_paused",
                        content,
                    },
                ))
            }
            ForgeEscalationDecision::BlockedExternal { key, content } => {
                if !self.mark_forge_escalation_notified(key).await {
                    return Ok(None);
                }
                let updated = self
                    .update_goal_status_for_forge_escalation(
                        thread_id,
                        &goal.goal_id,
                        ThreadGoalStatusRecord::Blocked,
                    )
                    .await?;
                if let Some(report) = self.report_for_final_status(&updated).await {
                    return Ok(Some(
                        GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
                            goal: updated,
                            source: "forge_external_blocker",
                            content,
                            report,
                        },
                    ));
                }
                Ok(Some(
                    GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
                        goal: updated,
                        source: "forge_external_blocker",
                        content,
                    },
                ))
            }
        }
    }

    async fn mark_forge_escalation_notified(&self, key: String) -> bool {
        let mut state = self.state.lock().await;
        state.forge_escalation_notified_keys.insert(key)
    }

    async fn update_goal_status_for_forge_escalation(
        &self,
        thread_id: &ThreadId,
        goal_id: &str,
        status: ThreadGoalStatusRecord,
    ) -> anyhow::Result<ThreadGoal> {
        let updated = self
            .db
            .update_thread_goal(
                thread_id,
                GoalUpdate {
                    objective: None,
                    status: Some(status),
                    token_budget: None,
                    expected_goal_id: Some(goal_id.to_string()),
                },
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("thread has no current goal"))?;
        let goal = thread_goal_from_record(updated);
        self.mark_active_goal(
            thread_id,
            (goal.status == ThreadGoalStatus::Active).then_some(goal.goal_id.clone()),
        )
        .await;
        Ok(goal)
    }

    async fn reset_suppression_for_current_goal(&self, thread_id: &ThreadId) -> anyhow::Result<()> {
        let expected_goal_id = {
            let state = self.state.lock().await;
            state
                .turn_accounting
                .get(thread_id)
                .and_then(|snapshot| snapshot.active_goal_id.clone())
        };
        let _ = self
            .db
            .reset_thread_goal_continuation_suppression(thread_id, expected_goal_id.as_deref())
            .await?;
        Ok(())
    }

    async fn budget_limit_effect(
        &self,
        goal: ThreadGoalRecord,
    ) -> anyhow::Result<GoalRuntimeEffect> {
        let first_budget_report = {
            let mut state = self.state.lock().await;
            state
                .budget_limit_reported_goal_ids
                .insert(goal.goal_id.clone())
        };
        if !first_budget_report {
            return Ok(GoalRuntimeEffect::EmitUpdated(thread_goal_from_record(
                goal,
            )));
        }
        let goal = thread_goal_from_record(goal);
        let content = crate::runtime::goal::prompts::budget_limit_prompt(&goal);
        if let Some(report) = self.report_for_final_status(&goal).await {
            return Ok(GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
                goal,
                source: "goal_budget_limited",
                content,
                report,
            });
        }
        Ok(GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
            goal,
            source: "goal_budget_limited",
            content,
        })
    }

    async fn goal_updated_effect(
        &self,
        goal: ThreadGoal,
        previous_goal: Option<ThreadGoal>,
    ) -> GoalRuntimeEffect {
        let transitioned = previous_goal
            .as_ref()
            .is_none_or(|previous| previous.status != goal.status);
        if transitioned {
            if let Some(report) = self.report_for_final_status(&goal).await {
                return GoalRuntimeEffect::EmitUpdatedAndGoalReport { goal, report };
            }
        }
        GoalRuntimeEffect::EmitUpdated(goal)
    }

    async fn report_for_final_status(&self, goal: &ThreadGoal) -> Option<ThreadGoalReport> {
        self.report_for_status(goal, false).await
    }

    async fn report_for_forge_pause(&self, goal: &ThreadGoal) -> Option<ThreadGoalReport> {
        self.report_for_status(goal, true).await
    }

    async fn report_for_status(
        &self,
        goal: &ThreadGoal,
        include_paused: bool,
    ) -> Option<ThreadGoalReport> {
        if !is_final_report_status(goal.status)
            && !(include_paused && goal.status == ThreadGoalStatus::Paused)
        {
            return None;
        }
        let open_questions = self.report_open_questions(&goal.goal_id).await;
        let review_summary = self.report_review_summary(&goal.goal_id).await;
        let mut state = self.state.lock().await;
        let key = report_status_key(&goal.goal_id, goal.status);
        if !state.reported_goal_statuses.insert(key) {
            return None;
        }
        let turns_run = state
            .goal_turn_counts
            .get(&goal.goal_id)
            .copied()
            .unwrap_or_default();
        let changed_files = state
            .goal_changed_files
            .get(&goal.goal_id)
            .cloned()
            .unwrap_or_default();
        Some(ThreadGoalReport {
            goal_id: goal.goal_id.clone(),
            objective: goal.objective.clone(),
            final_status: goal.status,
            turns_run,
            tokens_used: goal.tokens_used,
            token_budget: goal.token_budget,
            time_used_seconds: goal.time_used_seconds,
            changed_files,
            pending_approvals_count: 0,
            open_questions,
            review_summary,
            summary: fallback_report_summary(goal),
        })
    }

    async fn report_open_questions(&self, goal_id: &str) -> Vec<ThreadGoalReportOpenQuestion> {
        OpenQuestionStore::new(self.db.clone())
            .unresolved_for_goal(goal_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|question| ThreadGoalReportOpenQuestion {
                question_id: question.question_id,
                question: question.question,
                blocks_what: question.blocks_what,
            })
            .collect()
    }

    async fn report_review_summary(&self, goal_id: &str) -> Option<ThreadGoalReviewSummary> {
        ReviewStore::new(self.db.clone())
            .latest_ticket(goal_id)
            .await
            .ok()
            .flatten()
            .map(thread_goal_review_summary_from_ticket)
    }

    async fn record_changed_files_for_current_goal(
        &self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        changed_files: Vec<String>,
    ) {
        if changed_files.is_empty() {
            return;
        }
        let mut state = self.state.lock().await;
        let Some(goal_id) = state
            .turn_accounting
            .get(thread_id)
            .filter(|snapshot| &snapshot.turn_id == turn_id)
            .and_then(|snapshot| snapshot.active_goal_id.clone())
        else {
            return;
        };
        let files = state.goal_changed_files.entry(goal_id).or_default();
        for file in changed_files {
            if !files.iter().any(|existing| existing == &file) {
                files.push(file);
            }
        }
    }

    async fn rebuild_report_facts_from_events(
        &self,
        thread_id: &ThreadId,
        events: &[RuntimeEvent],
    ) {
        let mut turn_counts = HashMap::<String, i64>::new();
        let mut changed_files = HashMap::<String, Vec<String>>::new();
        for event in events.iter().filter(|event| &event.thread_id == thread_id) {
            match &event.kind {
                RuntimeEventKind::ThreadGoalTurnStarted { goal_id } => {
                    *turn_counts.entry(goal_id.clone()).or_insert(0) += 1;
                }
                RuntimeEventKind::ThreadGoalToolCompleted {
                    goal_id,
                    changed_files: files,
                } => {
                    let goal_files = changed_files.entry(goal_id.clone()).or_default();
                    for file in files {
                        if !goal_files.iter().any(|existing| existing == file) {
                            goal_files.push(file.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        if turn_counts.is_empty() && changed_files.is_empty() {
            return;
        }
        let mut state = self.state.lock().await;
        for (goal_id, count) in turn_counts {
            state.goal_turn_counts.insert(goal_id, count);
        }
        for (goal_id, files) in changed_files {
            state.goal_changed_files.insert(goal_id, files);
        }
    }

    async fn mark_active_goal(&self, thread_id: &ThreadId, goal_id: Option<String>) {
        let mut state = self.state.lock().await;
        mark_active_goal_locked(&mut state, thread_id, goal_id);
    }

    async fn mark_active_goal_locked(&self, thread_id: &ThreadId, goal_id: Option<String>) {
        let mut state = self.state.lock().await;
        mark_active_goal_locked(&mut state, thread_id, goal_id);
    }

    async fn has_active_turn(&self, thread_id: &ThreadId) -> bool {
        let state = self.state.lock().await;
        state.turn_accounting.contains_key(thread_id)
    }
}

fn mark_active_goal_locked(
    state: &mut GoalRuntimeState,
    thread_id: &ThreadId,
    goal_id: Option<String>,
) {
    let reset = state
        .wall_clock
        .get(thread_id)
        .is_none_or(|snapshot| snapshot.active_goal_id != goal_id);
    if reset {
        state.wall_clock.insert(
            thread_id.clone(),
            GoalWallClockAccountingSnapshot {
                active_goal_id: goal_id,
                last_accounted_at: Instant::now(),
            },
        );
    }
}

fn time_delta_since_last_accounting_locked(
    state: &mut GoalRuntimeState,
    thread_id: &ThreadId,
) -> i64 {
    let Some(snapshot) = state.wall_clock.get_mut(thread_id) else {
        return 0;
    };
    let seconds = snapshot.last_accounted_at.elapsed().as_secs();
    snapshot.last_accounted_at += Duration::from_secs(seconds);
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

fn token_delta(previous: &TokenUsage, current: &TokenUsage) -> i64 {
    current.total_tokens.saturating_sub(previous.total_tokens)
}

fn thread_goal_from_record(record: ThreadGoalRecord) -> ThreadGoal {
    ThreadGoal {
        thread_id: record.thread_id,
        goal_id: record.goal_id,
        objective: record.objective,
        status: status_protocol_from_record(record.status),
        token_budget: record.token_budget,
        tokens_used: record.tokens_used,
        time_used_seconds: record.time_used_seconds,
        continuation_suppressed: record.continuation_suppressed,
        continuation_suppressed_after_turn_id: record.continuation_suppressed_after_turn_id,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

fn status_protocol_from_record(status: ThreadGoalStatusRecord) -> ThreadGoalStatus {
    match status {
        ThreadGoalStatusRecord::Active => ThreadGoalStatus::Active,
        ThreadGoalStatusRecord::Paused => ThreadGoalStatus::Paused,
        ThreadGoalStatusRecord::Blocked => ThreadGoalStatus::Blocked,
        ThreadGoalStatusRecord::UsageLimited => ThreadGoalStatus::UsageLimited,
        ThreadGoalStatusRecord::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        ThreadGoalStatusRecord::Complete => ThreadGoalStatus::Complete,
    }
}

fn status_record_from_protocol(status: ThreadGoalStatus) -> ThreadGoalStatusRecord {
    match status {
        ThreadGoalStatus::Active => ThreadGoalStatusRecord::Active,
        ThreadGoalStatus::Paused => ThreadGoalStatusRecord::Paused,
        ThreadGoalStatus::Blocked => ThreadGoalStatusRecord::Blocked,
        ThreadGoalStatus::UsageLimited => ThreadGoalStatusRecord::UsageLimited,
        ThreadGoalStatus::BudgetLimited => ThreadGoalStatusRecord::BudgetLimited,
        ThreadGoalStatus::Complete => ThreadGoalStatusRecord::Complete,
    }
}

fn goal_from_effect(effect: &GoalRuntimeEffect) -> &ThreadGoal {
    match effect {
        GoalRuntimeEffect::EmitUpdated(goal)
        | GoalRuntimeEffect::EmitUpdatedAndGoalReport { goal, .. }
        | GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext { goal, .. }
        | GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport { goal, .. }
        | GoalRuntimeEffect::EmitUpdatedAndContinuationSuppressed { goal, .. } => goal,
        GoalRuntimeEffect::None
        | GoalRuntimeEffect::EmitCleared(_)
        | GoalRuntimeEffect::ScheduleContinuation => {
            unreachable!("status update must produce an updated goal")
        }
    }
}

fn is_final_report_status(status: ThreadGoalStatus) -> bool {
    matches!(
        status,
        ThreadGoalStatus::Complete
            | ThreadGoalStatus::Blocked
            | ThreadGoalStatus::UsageLimited
            | ThreadGoalStatus::BudgetLimited
    )
}

fn thread_goal_review_summary_from_ticket(ticket: ReviewTicket) -> ThreadGoalReviewSummary {
    ThreadGoalReviewSummary {
        ticket_id: ticket.ticket_id,
        status: match ticket.status {
            ReviewStatus::Pending => ThreadGoalReviewStatus::Pending,
            ReviewStatus::Approved => ThreadGoalReviewStatus::Approved,
            ReviewStatus::Rejected => ThreadGoalReviewStatus::Rejected,
        },
        reviewed_hash: ticket.reviewed_hash,
        reject_category: ticket
            .reject_category
            .map(thread_goal_review_reject_category),
        findings: ticket.findings,
    }
}

fn thread_goal_review_reject_category(
    category: ReviewRejectCategory,
) -> ThreadGoalReviewRejectCategory {
    match category {
        ReviewRejectCategory::RetriableGap => ThreadGoalReviewRejectCategory::RetriableGap,
        ReviewRejectCategory::NeedsUser => ThreadGoalReviewRejectCategory::NeedsUser,
        ReviewRejectCategory::ExternalBlocker => ThreadGoalReviewRejectCategory::ExternalBlocker,
    }
}

fn report_status_key(goal_id: &str, status: ThreadGoalStatus) -> String {
    format!("{goal_id}:{}", status_report_name(status))
}

fn status_report_name(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::Blocked => "blocked",
        ThreadGoalStatus::UsageLimited => "usage_limited",
        ThreadGoalStatus::BudgetLimited => "budget_limited",
        ThreadGoalStatus::Complete => "complete",
    }
}

fn fallback_report_summary(goal: &ThreadGoal) -> String {
    format!(
        "Goal {} with status {}.",
        goal.goal_id,
        status_report_name(goal.status)
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::index_db::{IndexDb, ProjectRecord, ProjectUpsert};
    use crate::runtime::forge::open_questions::OpenQuestionStore;
    use crate::runtime::forge::review::{ReviewRejectCategory, ReviewStore, ReviewVerdict};
    use crate::state::rollout::{events_from_rollout_items, RolloutItem, RolloutStore};
    use crate::types::{EventId, ThreadId};

    use super::*;

    async fn runtime_with_thread(thread_id: &ThreadId) -> (tempfile::TempDir, GoalRuntime) {
        let dir = tempfile::tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let project = add_project(&db, &dir.path().join("alpha"), "Alpha").await;
        insert_thread(&db, &project.id, thread_id).await;
        (dir, GoalRuntime::new(db))
    }

    async fn add_project(db: &IndexDb, root: &Path, name: &str) -> ProjectRecord {
        tokio::fs::create_dir_all(root).await.unwrap();
        db.upsert_project(ProjectUpsert {
            name: name.into(),
            path: root.into(),
        })
        .await
        .unwrap()
    }

    async fn insert_thread(db: &IndexDb, project_id: &str, thread_id: &ThreadId) {
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project_id)
        .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
        .bind(format!("{} title", thread_id.as_str()))
        .bind(format!("{} preview", thread_id.as_str()))
        .execute(db.pool())
        .await
        .unwrap();
    }

    fn usage(total: i64) -> TokenUsage {
        TokenUsage {
            total_tokens: total,
            ..TokenUsage::default()
        }
    }

    #[tokio::test]
    async fn turn_started_marks_active_goal_for_accounting() {
        let thread_id = ThreadId::new("goal_runtime_turn_started");
        let turn_id = TurnId::new("turn_1");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "ship runtime".to_string(), Some(100))
            .await
            .unwrap();

        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(10),
            })
            .await
            .unwrap();

        let state = runtime.state.lock().await;
        let snapshot = state.turn_accounting.get(&thread_id).unwrap();
        assert_eq!(
            snapshot.active_goal_id.as_deref(),
            Some(goal.goal_id.as_str())
        );
        assert_eq!(snapshot.last_accounted_token_usage.total_tokens, 10);
    }

    #[tokio::test]
    async fn tool_completed_accounts_token_and_time_delta() {
        let thread_id = ThreadId::new("goal_runtime_tool_completed");
        let turn_id = TurnId::new("turn_1");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "account usage".to_string(), Some(100))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(10),
            })
            .await
            .unwrap();
        {
            let mut state = runtime.state.lock().await;
            state
                .wall_clock
                .get_mut(&thread_id)
                .unwrap()
                .last_accounted_at -= Duration::from_secs(3);
        }

        runtime
            .apply(GoalRuntimeEvent::ToolCompleted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                tool_name: "read_file",
                token_usage: usage(15),
                changed_files: Vec::new(),
            })
            .await
            .unwrap();

        let goal = runtime.get_goal(&thread_id).await.unwrap().unwrap();
        assert_eq!(goal.tokens_used, 5);
        assert_eq!(goal.time_used_seconds, 3);
    }

    #[tokio::test]
    async fn budget_limit_clears_active_accounting_when_steering_is_suppressed() {
        let thread_id = ThreadId::new("goal_runtime_budget_suppressed");
        let turn_id = TurnId::new("turn_1");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "hit budget".to_string(), Some(5))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::ToolCompletedGoal {
                thread_id: &thread_id,
                turn_id: &turn_id,
                token_usage: usage(5),
            })
            .await
            .unwrap();

        assert!(matches!(effect, GoalRuntimeEffect::None));
        assert_eq!(
            runtime.get_goal(&thread_id).await.unwrap().unwrap().status,
            ThreadGoalStatus::BudgetLimited
        );
    }

    #[tokio::test]
    async fn budget_limit_unsuppressed_returns_goal_report_without_deadlock() {
        let thread_id = ThreadId::new("goal_runtime_budget_report");
        let turn_id = TurnId::new("turn_1");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "hit budget with report".to_string(), Some(5))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();

        let effect = tokio::time::timeout(
            Duration::from_millis(250),
            runtime.apply(GoalRuntimeEvent::ToolCompleted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                tool_name: "apply_patch",
                token_usage: usage(6),
                changed_files: vec!["src/runtime/goal/runtime.rs".to_string()],
            }),
        )
        .await
        .expect("budget limit report path should not deadlock")
        .unwrap();

        let GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport { goal, report, .. } = effect
        else {
            panic!("expected budget limit report effect, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::BudgetLimited);
        assert_eq!(report.final_status, ThreadGoalStatus::BudgetLimited);
        assert_eq!(
            report.changed_files,
            vec!["src/runtime/goal/runtime.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn thread_resumed_restores_active_goal_wall_clock() {
        let thread_id = ThreadId::new("goal_runtime_resumed");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "resume goal".to_string(), None)
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::ThreadResumed {
                thread_id: &thread_id,
                restored_events: &[],
            })
            .await
            .unwrap();

        assert!(matches!(effect, GoalRuntimeEffect::ScheduleContinuation));
        assert!(runtime
            .state
            .lock()
            .await
            .wall_clock
            .contains_key(&thread_id));
    }

    #[tokio::test]
    async fn empty_goal_continuation_suppresses_next_auto_continue() {
        let thread_id = ThreadId::new("goal_runtime_empty_continue");
        let turn_id = TurnId::new("turn_empty");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "continue goal".to_string(), None)
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::GoalContinuation,
                token_usage: usage(0),
            })
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::TurnFinished {
                thread_id: &thread_id,
                turn_id: &turn_id,
                turn_completed: true,
                token_usage: usage(0),
                assistant_had_activity: false,
            })
            .await
            .unwrap();

        assert!(matches!(
            effect,
            GoalRuntimeEffect::EmitUpdatedAndContinuationSuppressed { .. }
        ));
        let goal = runtime.get_goal(&thread_id).await.unwrap().unwrap();
        assert!(goal.continuation_suppressed);
        assert_eq!(goal.continuation_suppressed_after_turn_id, Some(turn_id));
        let next = runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &thread_id,
            })
            .await
            .unwrap();
        assert!(matches!(next, GoalRuntimeEffect::None));
    }

    #[tokio::test]
    async fn retriable_reject_with_progress_injects_findings_and_keeps_goal_active() {
        let thread_id = ThreadId::new("goal_runtime_reject_progress");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "fix review gap".to_string(), None)
            .await
            .unwrap();
        let review_store = ReviewStore::new(runtime.db.clone());
        let ticket = review_store
            .mint_ticket(goal.goal_id.clone(), Some("hash_a".to_string()))
            .await
            .unwrap();
        review_store
            .resolve_ticket_with_category(
                &ticket.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_a".to_string()),
                Some("missing regression test".to_string()),
                Some(ReviewRejectCategory::RetriableGap),
            )
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &thread_id,
            })
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext { goal, content, .. } = effect
        else {
            panic!("expected review guidance context, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::Active);
        assert!(content.contains("missing regression test"));
        assert!(!content.contains("change approach"));
    }

    #[tokio::test]
    async fn repeated_retriable_reject_same_hash_injects_no_progress_guidance() {
        let thread_id = ThreadId::new("goal_runtime_reject_stuck");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "fix persistent review gap".to_string(), None)
            .await
            .unwrap();
        let review_store = ReviewStore::new(runtime.db.clone());
        for (ticket_id_suffix, finding) in [("a", "first gap"), ("b", "same gap persists")] {
            let ticket = review_store
                .mint_ticket(goal.goal_id.clone(), Some("hash_same".to_string()))
                .await
                .unwrap();
            assert!(ticket.ticket_id.contains("rev_"), "{ticket_id_suffix}");
            review_store
                .resolve_ticket_with_category(
                    &ticket.ticket_id,
                    ReviewVerdict::Reject,
                    Some("hash_same".to_string()),
                    Some(finding.to_string()),
                    Some(ReviewRejectCategory::RetriableGap),
                )
                .await
                .unwrap();
        }

        let effect = runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &thread_id,
            })
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext { goal, content, .. } = effect
        else {
            panic!("expected no-progress guidance context, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::Active);
        assert!(content.contains("same gap persists"));
        assert!(content.contains("change approach"));
    }

    #[tokio::test]
    async fn needs_user_reject_with_open_questions_pauses_goal() {
        let thread_id = ThreadId::new("goal_runtime_needs_user_pauses");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "wait for product decision".to_string(), None)
            .await
            .unwrap();
        let review_store = ReviewStore::new(runtime.db.clone());
        let question_store = OpenQuestionStore::new(runtime.db.clone());
        question_store
            .record_question(
                thread_id.clone(),
                goal.goal_id.clone(),
                "Which customer segment is first?",
                "Rollout targeting",
            )
            .await
            .unwrap();
        let ticket = review_store
            .mint_ticket(goal.goal_id.clone(), Some("hash_a".to_string()))
            .await
            .unwrap();
        review_store
            .resolve_ticket_with_category(
                &ticket.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_a".to_string()),
                Some("needs user decision".to_string()),
                Some(ReviewRejectCategory::NeedsUser),
            )
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &thread_id,
            })
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
            goal,
            content,
            report,
            ..
        } = effect
        else {
            panic!("expected paused report context, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::Paused);
        assert_eq!(report.final_status, ThreadGoalStatus::Paused);
        assert_eq!(report.open_questions.len(), 1);
        assert_eq!(
            report.open_questions[0].question,
            "Which customer segment is first?"
        );
        let review_summary = report.review_summary.expect("review summary");
        assert_eq!(review_summary.ticket_id, ticket.ticket_id);
        assert_eq!(
            review_summary.reject_category,
            Some(crate::app_server::protocol::ThreadGoalReviewRejectCategory::NeedsUser)
        );
        assert!(content.contains("Which customer segment is first?"));
    }

    #[tokio::test]
    async fn external_blocker_reject_blocks_goal_with_reason_context() {
        let thread_id = ThreadId::new("goal_runtime_external_blocks");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "wait for upstream outage".to_string(), None)
            .await
            .unwrap();
        let review_store = ReviewStore::new(runtime.db.clone());
        let ticket = review_store
            .mint_ticket(goal.goal_id.clone(), Some("hash_a".to_string()))
            .await
            .unwrap();
        review_store
            .resolve_ticket_with_category(
                &ticket.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_a".to_string()),
                Some("external API is down".to_string()),
                Some(ReviewRejectCategory::ExternalBlocker),
            )
            .await
            .unwrap();

        let effect = runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &thread_id,
            })
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
            goal,
            content,
            report,
            ..
        } = effect
        else {
            panic!("expected blocked report context, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::Blocked);
        assert_eq!(report.final_status, ThreadGoalStatus::Blocked);
        assert!(content.contains("external API is down"));
    }

    #[tokio::test]
    async fn user_activity_resets_empty_continuation_suppression() {
        let thread_id = ThreadId::new("goal_runtime_user_resets");
        let turn_id = TurnId::new("turn_user");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "continue goal".to_string(), None)
            .await
            .unwrap();
        runtime
            .db
            .suppress_thread_goal_continuation(&thread_id, &goal.goal_id, &TurnId::new("empty"))
            .await
            .unwrap();

        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();

        assert!(
            !runtime
                .get_goal(&thread_id)
                .await
                .unwrap()
                .unwrap()
                .continuation_suppressed
        );
    }

    #[tokio::test]
    async fn external_mutation_accounts_before_status_change() {
        let thread_id = ThreadId::new("goal_runtime_external_accounts");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "pause goal".to_string(), None)
            .await
            .unwrap();
        {
            let mut state = runtime.state.lock().await;
            state
                .wall_clock
                .get_mut(&thread_id)
                .unwrap()
                .last_accounted_at -= Duration::from_secs(2);
        }

        runtime
            .apply(GoalRuntimeEvent::ExternalMutationStarting {
                thread_id: &thread_id,
                turn_id: None,
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .get_goal(&thread_id)
                .await
                .unwrap()
                .unwrap()
                .time_used_seconds,
            2
        );
    }

    #[tokio::test]
    async fn update_goal_to_complete_emits_goal_report_effect() {
        let thread_id = ThreadId::new("goal_runtime_complete_report");
        let turn_id = TurnId::new("turn_complete");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "ship the morning report".to_string(), Some(500))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(10),
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ToolCompletedGoal {
                thread_id: &thread_id,
                turn_id: &turn_id,
                token_usage: usage(42),
            })
            .await
            .unwrap();

        let effect = runtime
            .update_goal_status_effect(&thread_id, ThreadGoalStatus::Complete)
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedAndGoalReport { goal, report } = effect else {
            panic!("expected goal report effect, got {effect:?}");
        };
        assert_eq!(goal.status, ThreadGoalStatus::Complete);
        assert_eq!(report.objective, "ship the morning report");
        assert_eq!(report.final_status, ThreadGoalStatus::Complete);
        assert_eq!(report.turns_run, 1);
        assert_eq!(report.tokens_used, 32);
        assert_eq!(report.token_budget, Some(500));
        assert_eq!(report.pending_approvals_count, 0);
    }

    #[tokio::test]
    async fn goal_report_changed_files_are_scoped_to_goal_id() {
        let thread_id = ThreadId::new("goal_runtime_changed_files_scope");
        let first_turn_id = TurnId::new("turn_first");
        let second_turn_id = TurnId::new("turn_second");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "first goal".to_string(), Some(500))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &first_turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ToolCompleted {
                thread_id: &thread_id,
                turn_id: &first_turn_id,
                tool_name: "apply_patch",
                token_usage: usage(1),
                changed_files: vec!["src/first.rs".to_string()],
            })
            .await
            .unwrap();
        let first_effect = runtime
            .update_goal_status_effect(&thread_id, ThreadGoalStatus::Complete)
            .await
            .unwrap();
        let GoalRuntimeEffect::EmitUpdatedAndGoalReport {
            report: first_report,
            ..
        } = first_effect
        else {
            panic!("expected first goal report, got {first_effect:?}");
        };
        assert_eq!(first_report.changed_files, vec!["src/first.rs".to_string()]);

        tokio::time::sleep(Duration::from_millis(2)).await;
        let second_goal = runtime
            .db
            .replace_thread_goal(
                &thread_id,
                "second goal",
                ThreadGoalStatusRecord::Active,
                Some(500),
            )
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ThreadResumed {
                thread_id: &thread_id,
                restored_events: &[],
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &second_turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ToolCompleted {
                thread_id: &thread_id,
                turn_id: &second_turn_id,
                tool_name: "write_file",
                token_usage: usage(1),
                changed_files: vec!["src/second.rs".to_string()],
            })
            .await
            .unwrap();
        let second_effect = runtime
            .update_goal_status_effect(&thread_id, ThreadGoalStatus::Complete)
            .await
            .unwrap();
        let GoalRuntimeEffect::EmitUpdatedAndGoalReport {
            report: second_report,
            ..
        } = second_effect
        else {
            panic!("expected second goal report, got {second_effect:?}");
        };
        assert_eq!(second_report.goal_id, second_goal.goal_id);
        assert_eq!(
            second_report.changed_files,
            vec!["src/second.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn external_set_final_status_with_objective_change_emits_report_and_context() {
        let thread_id = ThreadId::new("goal_runtime_external_final_objective");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        let previous_goal = runtime
            .create_goal(&thread_id, "old objective".to_string(), None)
            .await
            .unwrap();
        let mut goal = previous_goal.clone();
        goal.objective = "new objective".to_string();
        goal.status = ThreadGoalStatus::Complete;

        let effect = runtime
            .apply(GoalRuntimeEvent::ExternalSet {
                thread_id: &thread_id,
                goal,
                previous_goal: Some(previous_goal),
            })
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport { source, report, .. } =
            effect
        else {
            panic!("expected objective context plus goal report, got {effect:?}");
        };
        assert_eq!(source, "goal_objective_updated");
        assert_eq!(report.objective, "new objective");
        assert_eq!(report.final_status, ThreadGoalStatus::Complete);
    }

    #[tokio::test]
    async fn thread_resumed_rebuilds_goal_report_facts_from_persisted_markers() {
        let thread_id = ThreadId::new("goal_runtime_restart_report_facts");
        let turn_id = TurnId::new("turn_before_restart");
        let (dir, runtime) = runtime_with_thread(&thread_id).await;
        let goal = runtime
            .create_goal(&thread_id, "survive runtime restart".to_string(), Some(500))
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::User,
                token_usage: usage(0),
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ToolCompleted {
                thread_id: &thread_id,
                turn_id: &turn_id,
                tool_name: "write_file",
                token_usage: usage(10),
                changed_files: vec!["src/restarted.rs".to_string()],
            })
            .await
            .unwrap();
        let marker_events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_goal_turn_started"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ThreadGoalTurnStarted {
                    goal_id: goal.goal_id.clone(),
                },
            },
            RuntimeEvent {
                event_id: EventId::new("evt_goal_tool_completed"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id),
                kind: RuntimeEventKind::ThreadGoalToolCompleted {
                    goal_id: goal.goal_id.clone(),
                    changed_files: vec!["src/restarted.rs".to_string()],
                },
            },
        ];
        let rollout_store = RolloutStore::new(dir.path().join("goal-markers.jsonl"));
        rollout_store
            .append_items_blocking(
                &marker_events
                    .iter()
                    .cloned()
                    .map(RolloutItem::EventMsg)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let rollout_items = RolloutStore::read_items_blocking(rollout_store.path()).unwrap();
        let restored_events = events_from_rollout_items(&rollout_items);
        let restarted_runtime = GoalRuntime::new(runtime.db.clone());

        restarted_runtime
            .apply(GoalRuntimeEvent::ThreadResumed {
                thread_id: &thread_id,
                restored_events: &restored_events,
            })
            .await
            .unwrap();
        let effect = restarted_runtime
            .update_goal_status_effect(&thread_id, ThreadGoalStatus::Complete)
            .await
            .unwrap();

        let GoalRuntimeEffect::EmitUpdatedAndGoalReport { report, .. } = effect else {
            panic!("expected restarted runtime goal report, got {effect:?}");
        };
        assert_eq!(report.turns_run, 1);
        assert_eq!(report.changed_files, vec!["src/restarted.rs".to_string()]);
    }

    #[tokio::test]
    async fn thread_resumed_resets_wall_clock_baseline_to_now_without_counting_app_closed_time() {
        let thread_id = ThreadId::new("goal_runtime_resume_baseline");
        let (_dir, runtime) = runtime_with_thread(&thread_id).await;
        runtime
            .create_goal(&thread_id, "resume baseline".to_string(), None)
            .await
            .unwrap();
        {
            let mut state = runtime.state.lock().await;
            state.wall_clock.clear();
        }

        runtime
            .apply(GoalRuntimeEvent::ThreadResumed {
                thread_id: &thread_id,
                restored_events: &[],
            })
            .await
            .unwrap();
        runtime
            .apply(GoalRuntimeEvent::ExternalMutationStarting {
                thread_id: &thread_id,
                turn_id: None,
            })
            .await
            .unwrap();

        assert_eq!(
            runtime
                .get_goal(&thread_id)
                .await
                .unwrap()
                .unwrap()
                .time_used_seconds,
            0
        );
    }
}
