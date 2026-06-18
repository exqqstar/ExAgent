use std::future::Future;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, oneshot, Notify};

use super::facade::{WorkspaceRuntimeOpGate, WorkspaceRuntimeOpPermit};
use super::op::{ThreadOp, ThreadOpResult, ThreadRuntimeError};
use super::reservation::{ActiveRuntimeTurnGuard, TurnReservations};
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEffect, GoalRuntimeEvent};
use crate::runtime::subagent::AgentTurnTerminalStatus;
use crate::runtime::thread_session::{RuntimeInterrupt, ThreadSession};
use crate::types::{ThreadId, TurnId, UserInput};

pub(super) const PENDING_MAIL_TURN_PROMPT: &str =
    "Process the pending inter-agent messages in your mailbox.";
pub(super) struct ThreadSubmission {
    pub(super) op: ThreadOp,
    pub(super) start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    pub(super) completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    pub(super) interrupt: Option<RuntimeInterrupt>,
    pub(super) _active_turn_guard: Option<ActiveRuntimeTurnGuard>,
    pub(super) _workspace_runtime_op_guard: Option<WorkspaceRuntimeOpPermit>,
}
pub(super) fn spawn_runtime_loop<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => {
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build thread runtime loop runtime");
                runtime.block_on(future);
            });
        }
    }
}

pub(super) struct ThreadRuntimeLoop {
    pub(super) op_tx: mpsc::Sender<ThreadSubmission>,
    pub(super) op_rx: mpsc::Receiver<ThreadSubmission>,
    pub(super) session: ThreadSession,
    pub(super) thread_id: ThreadId,
    pub(super) turn_reservation: TurnReservations,
    pub(super) goal_runtime: Option<Arc<GoalRuntime>>,
    pub(super) workspace_runtime_op_gate: Option<Arc<dyn WorkspaceRuntimeOpGate>>,
}

impl ThreadRuntimeLoop {
    pub(super) async fn run(mut self) {
        let _stopped = self.session.stopped_guard();
        if let Some(goal_runtime) = self.goal_runtime.as_ref() {
            let restored_events = self.session.persisted_runtime_events().unwrap_or_default();
            let _ = goal_runtime
                .apply(GoalRuntimeEvent::ThreadResumed {
                    thread_id: &self.thread_id,
                    restored_events: &restored_events,
                })
                .await;
        }
        let mut shut_down = false;
        while let Some(mut submission) = self.op_rx.recv().await {
            let mut check_goal_continuation = false;
            match submission.op {
                ThreadOp::Shutdown => {
                    self.session.shutdown().await;
                    shut_down = true;
                    complete(submission.completion_tx, Ok(ThreadOpResult::Ack));
                    break;
                }
                ThreadOp::UserInput {
                    turn_id,
                    input,
                    turn_context,
                } => {
                    let notify_turn_id = turn_id.clone();
                    let result = self
                        .session
                        .handle_user_input_parts_with_start_ack(
                            turn_id,
                            input,
                            turn_context,
                            submission.interrupt,
                            submission.start_tx,
                        )
                        .await;
                    if let Some((turn_id_for_notify, status, message)) =
                        terminal_notification_from_user_input_result(&notify_turn_id, &result)
                    {
                        self.session
                            .notify_parent_of_terminal_turn(&turn_id_for_notify, status, message)
                            .await;
                    }
                    drop(submission._active_turn_guard.take());
                    drop(submission._workspace_runtime_op_guard.take());
                    complete(submission.completion_tx, result);
                    check_goal_continuation = true;
                }
                ThreadOp::GoalContinuation { turn_id, goal_id } => {
                    let notify_turn_id = turn_id.clone();
                    let result = self
                        .session
                        .handle_goal_continuation(turn_id, goal_id, submission.interrupt)
                        .await;
                    if let Some((turn_id_for_notify, status, message)) =
                        terminal_notification_from_user_input_result(&notify_turn_id, &result)
                    {
                        self.session
                            .notify_parent_of_terminal_turn(&turn_id_for_notify, status, message)
                            .await;
                    }
                    drop(submission._active_turn_guard.take());
                    drop(submission._workspace_runtime_op_guard.take());
                    complete(submission.completion_tx, result);
                    check_goal_continuation = true;
                }
                ThreadOp::Interrupt { turn_id } => {
                    let result = self.session.handle_interrupt(turn_id).await;
                    complete(submission.completion_tx, result);
                }
                ThreadOp::ApprovalDecision {
                    turn_id,
                    approval_id,
                    status,
                    note,
                } => {
                    let result = self
                        .session
                        .handle_approval_decision(turn_id, approval_id, status, note)
                        .await;
                    complete(submission.completion_tx, result);
                }
                ThreadOp::SubmitUserInput {
                    turn_id,
                    request_id,
                    dismissed,
                } => {
                    let result = self
                        .session
                        .handle_user_input_response(turn_id, request_id, dismissed)
                        .await;
                    complete(submission.completion_tx, result);
                }
                ThreadOp::ManualCompaction => {
                    let result = self.session.handle_manual_compaction().await;
                    drop(submission._active_turn_guard.take());
                    drop(submission._workspace_runtime_op_guard.take());
                    complete(submission.completion_tx, result);
                }
                ThreadOp::GoalRuntimeEffect { effect } => {
                    match self.session.handle_goal_runtime_effect(effect).await {
                        Ok(should_check_goal_continuation) => {
                            check_goal_continuation = should_check_goal_continuation;
                            complete(submission.completion_tx, Ok(ThreadOpResult::Ack));
                        }
                        Err(error) => {
                            let _ = self
                                .session
                                .record_runtime_error_without_turn(error.to_string());
                            complete(submission.completion_tx, Err(error));
                        }
                    }
                }
            }
            let _ = self.maybe_start_turn_for_pending_mail().await;
            if check_goal_continuation {
                let _ = self.maybe_enqueue_goal_continuation().await;
            }
        }
        if !shut_down {
            self.session.shutdown().await;
        }
    }

    async fn maybe_start_turn_for_pending_mail(&mut self) -> Result<()> {
        if !self.op_rx.is_empty() {
            return Ok(());
        }
        if self.active_turn_id().is_some() {
            return Ok(());
        }
        if !self.session.inbox_handle().has_trigger_turn_pending().await {
            return Ok(());
        }
        let workspace_runtime_op_guard = match self.workspace_runtime_op_gate.as_ref() {
            Some(gate) => match gate.begin_runtime_op(&self.session.workspace_root()) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        thread_id = %self.thread_id.as_str(),
                        "skipping pending-mail turn while workspace runtime operation is gated"
                    );
                    return Ok(());
                }
            },
            None => None,
        };
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        let (turn_id, guard) = self.turn_reservation.reserve_next(
            &self.thread_id,
            interrupt_tx,
            interrupted.clone(),
        )?;
        self.op_tx
            .send(ThreadSubmission {
                op: ThreadOp::UserInput {
                    turn_id,
                    input: vec![UserInput::Text {
                        text: PENDING_MAIL_TURN_PROMPT.to_string(),
                    }],
                    turn_context: None,
                },
                start_tx: None,
                completion_tx: None,
                interrupt: Some(RuntimeInterrupt {
                    interrupt_rx,
                    interrupted,
                }),
                _active_turn_guard: Some(guard),
                _workspace_runtime_op_guard: workspace_runtime_op_guard,
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        Ok(())
    }

    async fn maybe_enqueue_goal_continuation(&mut self) -> Result<()> {
        if !self.op_rx.is_empty() {
            return Ok(());
        }
        let Some(goal_runtime) = self.goal_runtime.as_ref() else {
            return Ok(());
        };
        let workspace_runtime_op_guard = match self.workspace_runtime_op_gate.as_ref() {
            Some(gate) => match gate.begin_runtime_op(&self.session.workspace_root()) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        thread_id = %self.thread_id.as_str(),
                        "skipping goal continuation while workspace runtime operation is gated"
                    );
                    return Ok(());
                }
            },
            None => None,
        };
        if self.active_turn_id().is_some() {
            return Ok(());
        }
        let effect = goal_runtime
            .apply(GoalRuntimeEvent::MaybeContinueIfIdle {
                thread_id: &self.thread_id,
            })
            .await?;
        if !matches!(effect, GoalRuntimeEffect::ScheduleContinuation) {
            return Ok(());
        }
        let Some(goal) = goal_runtime.get_goal(&self.thread_id).await? else {
            return Ok(());
        };
        if goal.status != crate::app_server::protocol::ThreadGoalStatus::Active
            || goal.continuation_suppressed
        {
            return Ok(());
        }
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        let (turn_id, guard) = self.turn_reservation.reserve_next(
            &self.thread_id,
            interrupt_tx,
            interrupted.clone(),
        )?;
        self.op_tx
            .send(ThreadSubmission {
                op: ThreadOp::GoalContinuation {
                    turn_id,
                    goal_id: goal.goal_id,
                },
                start_tx: None,
                completion_tx: None,
                interrupt: Some(RuntimeInterrupt {
                    interrupt_rx,
                    interrupted,
                }),
                _active_turn_guard: Some(guard),
                _workspace_runtime_op_guard: workspace_runtime_op_guard,
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        Ok(())
    }

    fn active_turn_id(&self) -> Option<TurnId> {
        self.turn_reservation.active_turn_id()
    }
}

fn complete(
    completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    result: Result<ThreadOpResult>,
) {
    if let Some(completion_tx) = completion_tx {
        let _ = completion_tx.send(result);
    }
}

fn terminal_notification_from_user_input_result(
    turn_id: &TurnId,
    result: &Result<ThreadOpResult>,
) -> Option<(TurnId, AgentTurnTerminalStatus, String)> {
    match result {
        Ok(ThreadOpResult::UserInput { final_turn, .. }) => Some((
            turn_id.clone(),
            AgentTurnTerminalStatus::Completed,
            final_turn
                .text
                .clone()
                .unwrap_or_else(|| "Subagent turn completed without final text.".to_string()),
        )),
        Ok(ThreadOpResult::Interrupted { .. }) => Some((
            turn_id.clone(),
            AgentTurnTerminalStatus::Interrupted,
            "Subagent turn was interrupted.".to_string(),
        )),
        Err(err)
            if matches!(
                err.downcast_ref::<ThreadRuntimeError>(),
                Some(ThreadRuntimeError::TurnInterrupted { .. })
            ) =>
        {
            Some((
                turn_id.clone(),
                AgentTurnTerminalStatus::Interrupted,
                err.to_string(),
            ))
        }
        Err(err) => Some((
            turn_id.clone(),
            AgentTurnTerminalStatus::Failed,
            err.to_string(),
        )),
        _ => None,
    }
}
