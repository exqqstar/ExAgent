use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

use crate::app_server::protocol::{ThreadGoal, ThreadGoalStatus};
use crate::index_db::{
    GoalAccountingMode, GoalAccountingOutcome, GoalUpdate, IndexDb, ThreadGoalRecord,
    ThreadGoalStatusRecord,
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
    EmitUpdatedAndInjectPersistentContext {
        goal: ThreadGoal,
        source: &'static str,
        content: String,
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
        self.runtime.update_goal_status(thread_id, status).await
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
            } => {
                self.account_turn_progress(thread_id, turn_id, token_usage, true, false)
                    .await?;
                if tool_name != "update_goal" {
                    self.reset_suppression_for_current_goal(thread_id).await?;
                }
                Ok(GoalRuntimeEffect::None)
            }
            GoalRuntimeEvent::ToolCompletedGoal {
                thread_id,
                turn_id,
                token_usage,
            } => {
                self.account_turn_progress(thread_id, turn_id, token_usage, false, true)
                    .await?;
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
            GoalRuntimeEvent::ThreadResumed { thread_id } => self.thread_resumed(thread_id).await,
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

    pub(crate) async fn update_goal_status(
        &self,
        thread_id: &ThreadId,
        status: ThreadGoalStatus,
    ) -> anyhow::Result<ThreadGoal> {
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
        Ok(thread_goal_from_record(goal))
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
            return Ok(GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
                goal: goal.clone(),
                source: "goal_objective_updated",
                content: crate::runtime::goal::prompts::objective_updated_prompt(&goal),
            });
        }
        if active && !self.has_active_turn(thread_id).await {
            return Ok(GoalRuntimeEffect::ScheduleContinuation);
        }
        Ok(GoalRuntimeEffect::EmitUpdated(goal))
    }

    async fn external_clear(&self, thread_id: &ThreadId) -> anyhow::Result<GoalRuntimeEffect> {
        let mut state = self.state.lock().await;
        state.turn_accounting.remove(thread_id);
        state.wall_clock.remove(thread_id);
        Ok(GoalRuntimeEffect::EmitCleared(thread_id.clone()))
    }

    async fn thread_resumed(&self, thread_id: &ThreadId) -> anyhow::Result<GoalRuntimeEffect> {
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
            return Ok(GoalRuntimeEffect::ScheduleContinuation);
        }
        Ok(GoalRuntimeEffect::None)
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
        let mut state = self.state.lock().await;
        if !state
            .budget_limit_reported_goal_ids
            .insert(goal.goal_id.clone())
        {
            return Ok(GoalRuntimeEffect::EmitUpdated(thread_goal_from_record(
                goal,
            )));
        }
        let goal = thread_goal_from_record(goal);
        Ok(GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
            goal: goal.clone(),
            source: "goal_budget_limited",
            content: crate::runtime::goal::prompts::budget_limit_prompt(&goal),
        })
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use crate::index_db::{IndexDb, ProjectRecord, ProjectUpsert};
    use crate::types::ThreadId;

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
