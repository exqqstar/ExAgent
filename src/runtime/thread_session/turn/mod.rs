//! Turn execution for [`ThreadSession`], split across submodules by responsibility.
//!
//! Style note: functions that must mutate two or more `ThreadSession` fields at
//! once (e.g. `recorder` and `context_manager` together) are free functions over
//! destructured borrows rather than `&mut self` methods — see the
//! `let Self { agent, recorder, .. } = self;` destructures in the dispatchers
//! below. `&mut self` borrows the whole session, so it cannot express the
//! field-disjoint borrows the sampling/tool loop needs; the borrow checker, not
//! style preference, draws that line. Anything needing only `&self`/`&mut self`
//! stays a method. Do not "unify" the hot-loop free fns back into methods.

mod compaction_flow;
mod context_start;
mod external_input;
mod goal_effects;
mod recording;
mod sampling;
mod turn_config;

use std::future::Future;

use anyhow::{anyhow, Result};
use tokio::sync::oneshot;

use self::goal_effects::{apply_goal_effect, record_goal_turn_started_marker};
use self::recording::{assistant_turn_has_activity, current_token_usage};
use self::sampling::run_session_turn;
use self::turn_config::TurnThinkingModeOverride;
use super::{LiveEventSink, RuntimeInterrupt, ThreadSession};
use crate::events::RuntimeEventKind;
use crate::model::multimodal;
use crate::runtime::goal::runtime::{GoalRuntimeEvent, GoalTurnTrigger};
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::runtime::turn_mode::TurnMode;
use crate::state::rollout::RolloutItem;
use crate::types::{TurnId, UserInput};

#[cfg(test)]
mod tests;

impl ThreadSession {
    #[cfg(test)]
    pub(crate) async fn handle_user_input(
        &mut self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.handle_user_input_parts(
            turn_id,
            vec![UserInput::Text { text: prompt }],
            turn_context,
            interrupt,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn handle_user_input_parts(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.handle_user_input_parts_with_start_ack(turn_id, input, turn_context, interrupt, None)
            .await
    }

    pub(crate) async fn handle_user_input_parts_with_start_ack(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
        start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    ) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self
            .handle_user_input_inner(turn_id, input, turn_context, interrupt, start_tx)
            .await;
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    pub(crate) async fn handle_goal_continuation(
        &mut self,
        turn_id: TurnId,
        goal_id: String,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let turn_id_for_error = turn_id.clone();
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self
            .handle_goal_continuation_inner(turn_id, goal_id, interrupt)
            .await;
        if let Err(err) = &result {
            if !is_turn_interrupted_error(err) {
                let _ = self.record_runtime_error_for_turn_from_live(&turn_id_for_error, err);
            }
        }
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    pub(crate) async fn handle_manual_compaction(&mut self) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self.handle_manual_compaction_inner().await;
        if let Err(error) = &result {
            let _ = self.record_runtime_error_without_turn(error.to_string());
        }
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    async fn handle_goal_continuation_inner(
        &mut self,
        turn_id: TurnId,
        goal_id: String,
        mut interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.record_turn_started(&snapshot, &turn_id, None)?;
        let Some(goal_runtime) = self.goal_runtime.as_ref().cloned() else {
            return Err(anyhow!("goal continuation requested without goal runtime"));
        };
        let Some(goal) = goal_runtime.get_goal(&snapshot.thread_id).await? else {
            return Err(anyhow!("goal continuation requested without active goal"));
        };
        if goal.goal_id != goal_id
            || goal.status != crate::app_server::protocol::ThreadGoalStatus::Active
        {
            return Err(anyhow!("goal continuation candidate is no longer active"));
        }
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::ThreadGoalContinuationStarted {
                goal_id: goal.goal_id.clone(),
            },
        )?;
        let continuation_prompt = goal_runtime
            .continuation_prompt_for_goal(&snapshot.thread_id, &goal)
            .await?;
        let context_message = self
            .context_manager
            .record_persistent_internal_context("goal_continuation", continuation_prompt);
        self.context_manager.sync_snapshot(&mut snapshot);
        self.rollout_store
            .append_items_blocking(&[RolloutItem::response_item_for_turn(
                turn_id.clone(),
                context_message,
            )])?;
        let effect = goal_runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &snapshot.thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::GoalContinuation,
                token_usage: current_token_usage(&self.context_manager),
            })
            .await?;
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            Some(&turn_id),
            effect,
        )
        .await?;
        self.recorder.record(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::ThreadGoalTurnStarted {
                goal_id: goal.goal_id.clone(),
            },
        )?;

        let run_result = {
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            let runtime_turn_id = turn_id.clone();
            race_optional_interrupt(
                run_session_turn(
                    agent,
                    recorder,
                    rollout_store,
                    context_manager,
                    goal_runtime.as_deref(),
                    &mut snapshot,
                    runtime_turn_id,
                    None,
                    None,
                    TurnThinkingModeOverride::Inherit,
                    TurnMode::Default,
                    inbox,
                ),
                interrupt.as_mut(),
            )
            .await
        };
        let final_turn = match run_result {
            Ok(TurnOutcome::Completed(turn)) => turn,
            Ok(TurnOutcome::Interrupted) => {
                let interrupt = interrupt
                    .as_ref()
                    .expect("interrupted turn must have interrupt state");
                self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                return Err(ThreadRuntimeError::TurnInterrupted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                }
                .into());
            }
            Err(err) => return Err(err),
        };

        let effect = goal_runtime
            .apply(GoalRuntimeEvent::TurnFinished {
                thread_id: &snapshot.thread_id,
                turn_id: &turn_id,
                turn_completed: true,
                token_usage: current_token_usage(&self.context_manager),
                assistant_had_activity: assistant_turn_has_activity(&final_turn),
            })
            .await?;
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            Some(&turn_id),
            effect,
        )
        .await?;
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::TurnCompleted,
        )?;
        Ok(ThreadOpResult::UserInput {
            turn_id,
            final_turn,
        })
    }

    async fn handle_user_input_inner(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        mut interrupt: Option<RuntimeInterrupt>,
        mut start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    ) -> Result<ThreadOpResult> {
        let turn_cwd = turn_context
            .as_ref()
            .and_then(|context| context.cwd.clone());
        let turn_resolved_model = turn_context
            .as_ref()
            .and_then(|context| context.resolved_model.clone());
        let turn_thinking_mode = TurnThinkingModeOverride::from_turn_context(turn_context.as_ref());
        let turn_mode = turn_context
            .as_ref()
            .map(|context| context.turn_mode)
            .unwrap_or(TurnMode::Default);
        let effective_model = turn_resolved_model
            .as_ref()
            .unwrap_or(&self.base_config.model);
        if let Err(err) = multimodal::validate_turn_input_modalities(
            &input,
            &effective_model.capabilities.input_modalities,
        ) {
            send_start_ack_error(start_tx.take(), &err);
            return Err(err);
        }
        if let Err(err) = multimodal::validate_local_image_inputs(&input) {
            send_start_ack_error(start_tx.take(), &err);
            return Err(err);
        }

        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.record_turn_started(&snapshot, &turn_id, start_tx)?;
        if let Err(err) = self
            .ensure_agent_for_turn_model(turn_resolved_model.as_ref())
            .await
        {
            self.record_runtime_error(&snapshot, &turn_id, &err)?;
            return Err(err);
        }

        match race_optional_interrupt(
            self.compact_before_turn_if_needed(&turn_id, &mut snapshot),
            interrupt.as_mut(),
        )
        .await
        {
            Ok(TurnOutcome::Completed(())) => {}
            Ok(TurnOutcome::Interrupted) => {
                let interrupt = interrupt
                    .as_ref()
                    .expect("interrupted turn must have interrupt state");
                self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                return Err(ThreadRuntimeError::TurnInterrupted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                }
                .into());
            }
            Err(err) => {
                self.record_runtime_error(&snapshot, &turn_id, &err)?;
                return Err(err);
            }
        }

        let runtime_turn_cwd = turn_cwd.clone();
        if let Err(err) = self
            .record_user_turn_start(
                &turn_id,
                &input,
                turn_cwd,
                turn_resolved_model.as_ref(),
                turn_thinking_mode,
                turn_mode,
                &mut snapshot,
            )
            .await
        {
            self.record_runtime_error(&snapshot, &turn_id, &err)?;
            return Err(err);
        }

        let run_result = {
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            let runtime_turn_id = turn_id.clone();
            race_optional_interrupt(
                run_session_turn(
                    agent,
                    recorder,
                    rollout_store,
                    context_manager,
                    goal_runtime.as_deref(),
                    &mut snapshot,
                    runtime_turn_id,
                    runtime_turn_cwd,
                    turn_resolved_model.as_ref(),
                    turn_thinking_mode,
                    turn_mode,
                    inbox,
                ),
                interrupt.as_mut(),
            )
            .await
        };
        let final_turn = match run_result {
            Ok(TurnOutcome::Completed(turn)) => turn,
            Ok(TurnOutcome::Interrupted) => {
                let interrupt = interrupt
                    .as_ref()
                    .expect("interrupted turn must have interrupt state");
                self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                return Err(ThreadRuntimeError::TurnInterrupted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                }
                .into());
            }
            Err(err) => {
                let message = err.to_string();
                self.append_and_broadcast_snapshot(
                    &snapshot,
                    Some(&turn_id),
                    RuntimeEventKind::RuntimeError { message },
                )?;
                return Err(err);
            }
        };

        if !matches!(turn_mode, TurnMode::Plan) {
            if let Some(goal_runtime) = self.goal_runtime.as_ref() {
                let effect = goal_runtime
                    .apply(GoalRuntimeEvent::TurnFinished {
                        thread_id: &snapshot.thread_id,
                        turn_id: &turn_id,
                        turn_completed: true,
                        token_usage: current_token_usage(&self.context_manager),
                        assistant_had_activity: assistant_turn_has_activity(&final_turn),
                    })
                    .await?;
                apply_goal_effect(
                    Some(&self.agent),
                    &mut self.recorder,
                    &self.rollout_store,
                    &mut self.context_manager,
                    &mut snapshot,
                    Some(&turn_id),
                    effect,
                )
                .await?;
                record_goal_turn_started_marker(
                    goal_runtime,
                    &mut self.recorder,
                    &snapshot,
                    &turn_id,
                )
                .await?;
            }
        }

        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::TurnCompleted,
        )?;

        Ok(ThreadOpResult::UserInput {
            turn_id,
            final_turn,
        })
    }
}

enum TurnOutcome<T> {
    Completed(T),
    Interrupted,
}

async fn race_optional_interrupt<T>(
    fut: impl Future<Output = Result<T>>,
    interrupt: Option<&mut RuntimeInterrupt>,
) -> Result<TurnOutcome<T>> {
    let Some(interrupt) = interrupt else {
        return Ok(TurnOutcome::Completed(fut.await?));
    };

    tokio::select! {
        result = fut => Ok(TurnOutcome::Completed(result?)),
        _ = &mut interrupt.interrupt_rx => Ok(TurnOutcome::Interrupted),
    }
}

fn is_turn_interrupted_error(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<ThreadRuntimeError>(),
        Some(ThreadRuntimeError::TurnInterrupted { .. })
    )
}

fn send_start_ack_error(start_tx: Option<oneshot::Sender<Result<TurnId>>>, err: &anyhow::Error) {
    if let Some(start_tx) = start_tx {
        let _ = start_tx.send(Err(anyhow!(err.to_string())));
    }
}
