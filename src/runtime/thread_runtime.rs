use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{anyhow, Result};
use std::future::Future;
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::{AgentConfig, ThinkingMode};
use crate::events::RuntimeEvent;
use crate::policy::PolicyManager;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEffect, GoalRuntimeEvent};
use crate::runtime::subagent::{AgentControl, AgentTurnTerminalStatus, InterAgentCommunication};
use crate::runtime::thread_session::{
    RuntimeInterrupt, ThreadInbox, ThreadSession, ThreadSessionLiveState, ThreadSessionLiveView,
    ThreadSessionOptions,
};
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ApprovalId, ApprovalStatus};
use crate::types::{AssistantTurn, ThreadId, TurnId, UserInput};

const THREAD_OP_CHANNEL_CAPACITY: usize = 64;
const THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;
const PENDING_MAIL_TURN_PROMPT: &str = "Process the pending inter-agent messages in your mailbox.";

pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;
pub(crate) type WorkspaceRuntimeOpPermit = Box<dyn Send + 'static>;

pub(crate) trait WorkspaceRuntimeOpGate: Send + Sync {
    fn begin_runtime_op(&self, workspace_root: &Path) -> Result<WorkspaceRuntimeOpPermit>;
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadRuntimeError {
    #[error("thread is busy: {}", .0.as_str())]
    ThreadBusy(ThreadId),

    #[error("turn rejected for thread {}: {reason}", thread_id.as_str())]
    TurnRejected { thread_id: ThreadId, reason: String },

    #[error("turn interrupted for thread {}: {}", thread_id.as_str(), turn_id.as_str())]
    TurnInterrupted {
        thread_id: ThreadId,
        turn_id: TurnId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRuntimeStatus {
    Idle,
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
    pub resolved_model: Option<ResolvedModelConfig>,
    pub thinking_mode: Option<ThinkingMode>,
    pub clear_thinking_mode: bool,
    pub turn_mode: TurnMode,
}

#[derive(Debug, Clone)]
pub(crate) enum ThreadOp {
    UserInput {
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
    },
    GoalContinuation {
        turn_id: TurnId,
        goal_id: String,
    },
    Interrupt {
        turn_id: Option<TurnId>,
    },
    ApprovalDecision {
        turn_id: Option<TurnId>,
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    },
    SubmitUserInput {
        turn_id: Option<TurnId>,
        request_id: ApprovalId,
        dismissed: bool,
    },
    GoalRuntimeEffect {
        effect: GoalRuntimeEffect,
    },
    ManualCompaction,
    Shutdown,
}

pub enum ThreadOpResult {
    UserInput {
        turn_id: TurnId,
        final_turn: AssistantTurn,
    },
    Interrupted {
        turn_id: TurnId,
    },
    ApprovalDecision {
        turn_id: TurnId,
        approval_id: ApprovalId,
        status: ApprovalStatus,
    },
    UserInputSubmitted {
        turn_id: TurnId,
        request_id: ApprovalId,
        dismissed: bool,
    },
    Ack,
}

pub(crate) struct ThreadSubmission {
    op: ThreadOp,
    start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    interrupt: Option<RuntimeInterrupt>,
    _active_turn_guard: Option<ActiveRuntimeTurnGuard>,
    _workspace_runtime_op_guard: Option<WorkspaceRuntimeOpPermit>,
}

pub struct ThreadRuntimeOptions {
    pub thread_id: ThreadId,
    pub config: AgentConfig,
    agent_factory: AgentFactory,
    policy: Arc<PolicyManager>,
    subagent_control: Option<Arc<AgentControl>>,
    goal_runtime: Option<Arc<GoalRuntime>>,
    workspace_runtime_op_gate: Option<Arc<dyn WorkspaceRuntimeOpGate>>,
}

impl ThreadRuntimeOptions {
    pub fn new(thread_id: ThreadId, config: AgentConfig, agent_factory: AgentFactory) -> Self {
        Self {
            thread_id,
            config,
            agent_factory,
            policy: Arc::new(PolicyManager::default()),
            subagent_control: None,
            goal_runtime: None,
            workspace_runtime_op_gate: None,
        }
    }

    pub fn with_policy(mut self, policy: Arc<PolicyManager>) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_subagent_control(mut self, subagent_control: Arc<AgentControl>) -> Self {
        self.subagent_control = Some(subagent_control);
        self
    }

    pub(crate) fn with_goal_runtime(mut self, goal_runtime: Arc<GoalRuntime>) -> Self {
        self.goal_runtime = Some(goal_runtime);
        self
    }

    pub(crate) fn with_workspace_runtime_op_gate(
        mut self,
        gate: Arc<dyn WorkspaceRuntimeOpGate>,
    ) -> Self {
        self.workspace_runtime_op_gate = Some(gate);
        self
    }
}

pub struct ThreadRuntime {
    thread_id: ThreadId,
    op_tx: mpsc::Sender<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_rx: watch::Receiver<ThreadRuntimeStatus>,
    turn_reservation: Arc<Mutex<TurnReservationState>>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
    inbox: Arc<ThreadInbox>,
    goal_runtime: Option<Arc<GoalRuntime>>,
}

impl ThreadRuntime {
    pub fn spawn(options: ThreadRuntimeOptions) -> Result<Arc<Self>> {
        let (op_tx, op_rx) = mpsc::channel(THREAD_OP_CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = watch::channel(ThreadRuntimeStatus::Idle);
        let goal_runtime = options.goal_runtime.clone();
        let workspace_runtime_op_gate = options.workspace_runtime_op_gate.clone();

        let session = ThreadSession::new(
            ThreadSessionOptions::new(options.thread_id, options.config, options.agent_factory)
                .with_event_tx(event_tx.clone())
                .with_status_tx(status_tx)
                .with_policy(options.policy)
                .with_subagent_control(options.subagent_control)
                .with_goal_runtime(options.goal_runtime),
        )?;
        let next_turn_index = session.next_turn_index_seed();
        let live_state = session.live_state_handle();
        let inbox = session.inbox_handle();

        let runtime = Arc::new(Self {
            thread_id: session.thread_id().clone(),
            op_tx,
            event_tx: event_tx.clone(),
            status_rx,
            turn_reservation: Arc::new(Mutex::new(TurnReservationState {
                next_turn_index,
                active_turn: None,
            })),
            live_state,
            inbox,
            goal_runtime,
        });

        let loop_op_tx = runtime.op_tx.clone();
        let loop_thread_id = runtime.thread_id.clone();
        let loop_turn_reservation = runtime.turn_reservation.clone();
        let loop_goal_runtime = runtime.goal_runtime.clone();
        let loop_workspace_runtime_op_gate = workspace_runtime_op_gate;
        spawn_runtime_loop(async move {
            ThreadRuntimeLoop {
                op_tx: loop_op_tx,
                op_rx,
                session,
                thread_id: loop_thread_id,
                turn_reservation: loop_turn_reservation,
                goal_runtime: loop_goal_runtime,
                workspace_runtime_op_gate: loop_workspace_runtime_op_gate,
            }
            .run()
            .await;
        });

        Ok(runtime)
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    pub fn status(&self) -> ThreadRuntimeStatus {
        *self.status_rx.borrow()
    }

    async fn submit_control_and_wait(&self, op: ThreadOp) -> Result<ThreadOpResult> {
        self.submit_and_wait_internal(op, None).await
    }

    pub async fn submit_user_input_and_wait(
        &self,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<ThreadOpResult> {
        self.submit_user_input_parts_and_wait(vec![UserInput::Text { text: prompt }], turn_context)
            .await
    }

    pub async fn submit_user_input_parts_and_wait(
        &self,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<ThreadOpResult> {
        let (_turn_id, completion_rx) = self
            .start_user_input_parts_with_completion(input, turn_context)
            .await?;
        completion_rx
            .await
            .map_err(|_| anyhow!("thread runtime stopped before completing op"))?
    }

    pub(crate) async fn start_user_input_parts_with_completion(
        &self,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<(TurnId, oneshot::Receiver<Result<ThreadOpResult>>)> {
        let (completion_tx, completion_rx) = oneshot::channel();
        let turn_id = self
            .send_user_input_parts(input, turn_context, Some(completion_tx))
            .await?;
        Ok((turn_id, completion_rx))
    }

    pub(crate) async fn submit_user_input(
        &self,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<TurnId> {
        self.submit_user_input_parts(vec![UserInput::Text { text: prompt }], turn_context)
            .await
    }

    pub(crate) async fn submit_user_input_parts(
        &self,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<TurnId> {
        self.send_user_input_parts(input, turn_context, None).await
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self.submit_control_and_wait(ThreadOp::Shutdown).await? {
            ThreadOpResult::Ack => Ok(()),
            _ => Err(anyhow!("shutdown returned non-ack runtime result")),
        }
    }

    pub async fn enqueue_inter_agent_communication(
        &self,
        mail: InterAgentCommunication,
    ) -> Result<()> {
        self.inbox.enqueue(mail).await
    }

    pub async fn compact_now(&self) -> Result<()> {
        let permit = self
            .op_tx
            .reserve()
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        let guard = self.reserve_manual_compaction_turn()?;
        let (completion_tx, completion_rx) = oneshot::channel();
        permit.send(ThreadSubmission {
            op: ThreadOp::ManualCompaction,
            start_tx: None,
            completion_tx: Some(completion_tx),
            interrupt: None,
            _active_turn_guard: Some(guard),
            _workspace_runtime_op_guard: None,
        });
        match completion_rx
            .await
            .map_err(|_| anyhow!("thread runtime stopped before completing op"))??
        {
            ThreadOpResult::Ack => Ok(()),
            _ => Err(anyhow!("manual compaction returned non-ack runtime result")),
        }
    }

    pub(crate) async fn enqueue_goal_runtime_effect(
        &self,
        effect: GoalRuntimeEffect,
    ) -> Result<()> {
        if matches!(effect, GoalRuntimeEffect::None) {
            return Ok(());
        }
        self.op_tx
            .send(ThreadSubmission {
                op: ThreadOp::GoalRuntimeEffect { effect },
                start_tx: None,
                completion_tx: None,
                interrupt: None,
                _active_turn_guard: None,
                _workspace_runtime_op_guard: None,
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        Ok(())
    }

    async fn send_user_input_parts(
        &self,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    ) -> Result<TurnId> {
        let permit = self
            .op_tx
            .reserve()
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        let (start_tx, start_rx) = oneshot::channel();
        let (turn_id, guard) = self.reserve_next_turn(interrupt_tx, interrupted.clone())?;
        permit.send(ThreadSubmission {
            op: ThreadOp::UserInput {
                turn_id: turn_id.clone(),
                input,
                turn_context,
            },
            start_tx: Some(start_tx),
            completion_tx,
            interrupt: Some(RuntimeInterrupt {
                interrupt_rx,
                interrupted,
            }),
            _active_turn_guard: Some(guard),
            _workspace_runtime_op_guard: None,
        });
        start_rx
            .await
            .map_err(|_| anyhow!("thread runtime stopped before starting turn"))?
    }

    async fn submit_and_wait_internal(
        &self,
        op: ThreadOp,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let (completion_tx, completion_rx) = oneshot::channel();
        self.op_tx
            .send(ThreadSubmission {
                op,
                start_tx: None,
                completion_tx: Some(completion_tx),
                interrupt,
                _active_turn_guard: None,
                _workspace_runtime_op_guard: None,
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        completion_rx
            .await
            .map_err(|_| anyhow!("thread runtime stopped before completing op"))?
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.event_tx.subscribe()
    }

    pub fn live_view(&self) -> ThreadSessionLiveView {
        ThreadSession::live_view_from_state(self.thread_id.clone(), &self.live_state)
    }

    pub(crate) fn active_turn_id(&self) -> Option<TurnId> {
        self.turn_reservation.lock().ok().and_then(|state| {
            state
                .active_turn
                .as_ref()
                .and_then(|record| record.public_turn_id.clone())
        })
    }

    pub(crate) async fn apply_goal_runtime_event(
        &self,
        event: GoalRuntimeEvent<'_>,
    ) -> Result<GoalRuntimeEffect> {
        let Some(goal_runtime) = self.goal_runtime.as_ref() else {
            return Ok(GoalRuntimeEffect::None);
        };
        goal_runtime.apply(event).await
    }

    pub(crate) async fn interrupt_active_turn(
        &self,
        requested_turn_id: Option<&TurnId>,
    ) -> Result<TurnId> {
        let record = self
            .turn_reservation
            .lock()
            .ok()
            .and_then(|state| state.active_turn.clone())
            .ok_or_else(|| anyhow!("thread has no active turn"))?;
        let public_turn_id =
            record
                .public_turn_id
                .clone()
                .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: "active operation is not interruptible".to_string(),
                })?;
        if let Some(requested_turn_id) = requested_turn_id {
            if requested_turn_id != &public_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("active turn is {}", public_turn_id.as_str()),
                }
                .into());
            }
        }

        let did_send_interrupt = record
            .interrupt_tx
            .lock()
            .expect("active turn interrupt mutex poisoned")
            .take()
            .map(|interrupt_tx| interrupt_tx.send(()).is_ok())
            .unwrap_or(false);
        if !did_send_interrupt {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "active turn is already interrupting or completed".to_string(),
            }
            .into());
        }
        record.interrupted.notified().await;
        Ok(public_turn_id)
    }

    pub(crate) async fn interrupt_waiting_approval_turn(
        &self,
        requested_turn_id: Option<TurnId>,
    ) -> Result<TurnId> {
        match self
            .submit_control_and_wait(ThreadOp::Interrupt {
                turn_id: requested_turn_id,
            })
            .await?
        {
            ThreadOpResult::Interrupted { turn_id } => Ok(turn_id),
            _ => Err(anyhow!("interrupt returned non-interrupted runtime result")),
        }
    }

    pub(crate) async fn approval_decision(
        &self,
        requested_turn_id: Option<TurnId>,
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    ) -> Result<ThreadOpResult> {
        self.submit_control_and_wait(ThreadOp::ApprovalDecision {
            turn_id: requested_turn_id,
            approval_id,
            status,
            note,
        })
        .await
    }

    pub(crate) async fn submit_user_input_response(
        &self,
        requested_turn_id: Option<TurnId>,
        request_id: ApprovalId,
        dismissed: bool,
    ) -> Result<ThreadOpResult> {
        self.submit_control_and_wait(ThreadOp::SubmitUserInput {
            turn_id: requested_turn_id,
            request_id,
            dismissed,
        })
        .await
    }

    pub async fn wait_until_terminated(&self) {
        let mut status_rx = self.status_rx.clone();
        loop {
            if *status_rx.borrow() == ThreadRuntimeStatus::Stopped {
                return;
            }
            if status_rx.changed().await.is_err() {
                return;
            }
        }
    }

    fn reserve_next_turn(
        &self,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<(TurnId, ActiveRuntimeTurnGuard)> {
        reserve_next_turn_from_state(
            &self.turn_reservation,
            &self.thread_id,
            interrupt_tx,
            interrupted,
        )
    }

    fn reserve_manual_compaction_turn(&self) -> Result<ActiveRuntimeTurnGuard> {
        let (interrupt_tx, _interrupt_rx) = oneshot::channel();
        reserve_turn_record_from_state(
            &self.turn_reservation,
            &self.thread_id,
            None,
            interrupt_tx,
            Arc::new(Notify::new()),
        )
    }
}

fn reserve_next_turn_from_state(
    turn_reservation: &Arc<Mutex<TurnReservationState>>,
    thread_id: &ThreadId,
    interrupt_tx: oneshot::Sender<()>,
    interrupted: Arc<Notify>,
) -> Result<(TurnId, ActiveRuntimeTurnGuard)> {
    let mut state = turn_reservation
        .lock()
        .expect("turn reservation mutex poisoned");
    if state.active_turn.is_some() {
        return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
    }
    let turn_id = TurnId::new(format!("turn_{}", state.next_turn_index));
    state.next_turn_index = state.next_turn_index.saturating_add(1);
    state.active_turn = Some(ActiveRuntimeTurnRecord {
        public_turn_id: Some(turn_id.clone()),
        interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
        interrupted,
    });

    Ok((
        turn_id,
        ActiveRuntimeTurnGuard {
            turn_reservation: turn_reservation.clone(),
        },
    ))
}

fn reserve_turn_record_from_state(
    turn_reservation: &Arc<Mutex<TurnReservationState>>,
    thread_id: &ThreadId,
    public_turn_id: Option<TurnId>,
    interrupt_tx: oneshot::Sender<()>,
    interrupted: Arc<Notify>,
) -> Result<ActiveRuntimeTurnGuard> {
    let mut state = turn_reservation
        .lock()
        .expect("turn reservation mutex poisoned");
    if state.active_turn.is_some() {
        return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
    }
    state.active_turn = Some(ActiveRuntimeTurnRecord {
        public_turn_id,
        interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
        interrupted,
    });

    Ok(ActiveRuntimeTurnGuard {
        turn_reservation: turn_reservation.clone(),
    })
}

struct TurnReservationState {
    next_turn_index: u64,
    active_turn: Option<ActiveRuntimeTurnRecord>,
}

#[derive(Clone)]
struct ActiveRuntimeTurnRecord {
    public_turn_id: Option<TurnId>,
    interrupt_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    interrupted: Arc<Notify>,
}

struct ActiveRuntimeTurnGuard {
    turn_reservation: Arc<Mutex<TurnReservationState>>,
}

impl Drop for ActiveRuntimeTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.turn_reservation.lock() {
            state.active_turn = None;
        }
    }
}

fn spawn_runtime_loop<F>(future: F)
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

struct ThreadRuntimeLoop {
    op_tx: mpsc::Sender<ThreadSubmission>,
    op_rx: mpsc::Receiver<ThreadSubmission>,
    session: ThreadSession,
    thread_id: ThreadId,
    turn_reservation: Arc<Mutex<TurnReservationState>>,
    goal_runtime: Option<Arc<GoalRuntime>>,
    workspace_runtime_op_gate: Option<Arc<dyn WorkspaceRuntimeOpGate>>,
}

impl ThreadRuntimeLoop {
    async fn run(mut self) {
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
        let (turn_id, guard) = reserve_next_turn_from_state(
            &self.turn_reservation,
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
        let (turn_id, guard) = reserve_next_turn_from_state(
            &self.turn_reservation,
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
        self.turn_reservation.lock().ok().and_then(|state| {
            state
                .active_turn
                .as_ref()
                .and_then(|record| record.public_turn_id.clone())
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::app_server::protocol::ThreadGoalStatus;
    use crate::events::RuntimeEventKind;
    use crate::index_db::{IndexDb, ProjectUpsert};
    use crate::llm::{LlmClient, LlmRequestOptions, MockLlm};
    use crate::registry::ToolRegistry;
    use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
    use crate::runtime::goal::runtime::GoalRuntime;
    use crate::runtime::turn_mode::TurnMode;
    use crate::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
    use crate::tools::ToolSpec;
    use crate::types::{ConversationMessage, LlmCompletion, TokenUsage, ToolCall};

    struct BlockingFirstLlm {
        calls: AtomicUsize,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    struct PromptRecordingLlm {
        prompts: Arc<Mutex<Vec<String>>>,
    }

    struct RestoreGateProbe {
        restoring: Arc<std::sync::atomic::AtomicBool>,
        attempts: AtomicUsize,
    }

    struct RestoreGatePermit;

    impl WorkspaceRuntimeOpGate for RestoreGateProbe {
        fn begin_runtime_op(
            &self,
            _workspace_root: &std::path::Path,
        ) -> Result<WorkspaceRuntimeOpPermit> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.restoring.load(Ordering::SeqCst) {
                return Err(anyhow!("checkpoint restore is in progress"));
            }
            Ok(Box::new(RestoreGatePermit))
        }
    }

    #[async_trait]
    impl LlmClient for BlockingFirstLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            if call_index == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(AssistantTurn {
                text: Some(format!("turn {} complete", call_index + 1)),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    #[async_trait]
    impl LlmClient for PromptRecordingLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(serde_json::to_string(messages).expect("serialize prompt"));
            Ok(AssistantTurn {
                text: Some("processed mail".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
        }
    }

    fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
        let rollout_paths = rollout_paths(&config.workspace_root, thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: config.workspace_root.clone(),
                initial_cwd: config.cwd.clone(),
                permission_profile: crate::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-06-05T00:00:00Z".to_string(),
            })])
            .expect("write rollout meta");
    }

    fn blocking_first_runtime(
        thread_id: ThreadId,
        config: AgentConfig,
        started: Arc<Notify>,
        release: Arc<Notify>,
    ) -> Arc<ThreadRuntime> {
        ThreadRuntime::spawn(ThreadRuntimeOptions::new(
            thread_id,
            config,
            blocking_agent_factory(started, release),
        ))
        .expect("spawn runtime")
    }

    fn blocking_agent_factory(started: Arc<Notify>, release: Arc<Notify>) -> AgentFactory {
        Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(BlockingFirstLlm {
                    calls: AtomicUsize::new(0),
                    started: started.clone(),
                    release: release.clone(),
                }),
                ToolRegistry::new(),
            ))
        })
    }

    async fn wait_for_turn_completed(
        events: &mut broadcast::Receiver<RuntimeEvent>,
        turn_id: &TurnId,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.expect("runtime event");
                if event.turn_id.as_ref() == Some(turn_id)
                    && matches!(event.kind, crate::events::RuntimeEventKind::TurnCompleted)
                {
                    return;
                }
            }
        })
        .await
        .expect("turn completed event");
    }

    async fn wait_for_runtime_error(
        events: &mut broadcast::Receiver<RuntimeEvent>,
        turn_id: &TurnId,
    ) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.expect("runtime event");
                if event.turn_id.as_ref() != Some(turn_id) {
                    continue;
                }
                if let crate::events::RuntimeEventKind::RuntimeError { message } = event.kind {
                    return message;
                }
            }
        })
        .await
        .expect("runtime error event")
    }

    async fn wait_until_no_active_turn(runtime: &ThreadRuntime) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if runtime.active_turn_id().is_none() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("active turn cleared");
    }

    async fn wait_for_goal_status(db: &IndexDb, thread_id: &ThreadId, status: ThreadGoalStatus) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let goal = db
                    .get_thread_goal(thread_id)
                    .await
                    .expect("goal lookup")
                    .expect("goal exists");
                let current = match goal.status {
                    crate::index_db::ThreadGoalStatusRecord::Active => ThreadGoalStatus::Active,
                    crate::index_db::ThreadGoalStatusRecord::Paused => ThreadGoalStatus::Paused,
                    crate::index_db::ThreadGoalStatusRecord::Blocked => ThreadGoalStatus::Blocked,
                    crate::index_db::ThreadGoalStatusRecord::UsageLimited => {
                        ThreadGoalStatus::UsageLimited
                    }
                    crate::index_db::ThreadGoalStatusRecord::BudgetLimited => {
                        ThreadGoalStatus::BudgetLimited
                    }
                    crate::index_db::ThreadGoalStatusRecord::Complete => ThreadGoalStatus::Complete,
                };
                if current == status {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("goal reached expected status");
    }

    fn usage(total_tokens: i64) -> TokenUsage {
        TokenUsage {
            total_tokens,
            ..TokenUsage::default()
        }
    }

    #[tokio::test]
    async fn active_goal_auto_continues_until_update_goal_complete() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_goal_auto_continue");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .expect("index db");
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Goal Project".to_string(),
                path: dir.path().to_path_buf(),
            })
            .await
            .expect("project");
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal thread', 'Goal preview', 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(&project.id)
        .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
        .execute(db.pool())
        .await
        .expect("thread row");
        db.insert_thread_goal(&thread_id, "finish automatically", None)
            .await
            .expect("insert goal")
            .expect("new goal");

        let completions = vec![
            LlmCompletion {
                turn: AssistantTurn {
                    text: Some("made progress".to_string()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                token_usage: Some(usage(10)),
            },
            LlmCompletion {
                turn: AssistantTurn {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "call_update_goal".to_string(),
                        name: "update_goal".to_string(),
                        arguments: serde_json::json!({ "status": "complete" }),
                        thought_signature: None,
                    }],
                    reasoning: vec![],
                },
                token_usage: Some(usage(20)),
            },
            LlmCompletion {
                turn: AssistantTurn {
                    text: Some("goal complete".to_string()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                token_usage: Some(usage(25)),
            },
        ];
        let completions = Arc::new(Mutex::new(Some(completions)));
        let factory: AgentFactory = Arc::new(move |config| {
            let completions = completions
                .lock()
                .expect("completions mutex")
                .take()
                .expect("agent created once");
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new_completions(completions)),
                ToolRegistry::new(),
            ))
        });
        let runtime = ThreadRuntime::spawn(
            ThreadRuntimeOptions::new(thread_id.clone(), config, factory)
                .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone()))),
        )
        .expect("runtime");
        let mut events = runtime.subscribe_events();

        runtime
            .submit_user_input_and_wait("start".to_string(), None)
            .await
            .expect("initial turn");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.expect("runtime event");
                if matches!(
                    event.kind,
                    RuntimeEventKind::ThreadGoalContinuationStarted { .. }
                ) {
                    return;
                }
            }
        })
        .await
        .expect("continuation started");
        wait_for_goal_status(&db, &thread_id, ThreadGoalStatus::Complete).await;
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn pending_trigger_mail_starts_turn_after_op_completes() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_pending_trigger_mail_starts_turn");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let prompts_for_factory = prompts.clone();
        let factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(PromptRecordingLlm {
                    prompts: prompts_for_factory.clone(),
                }),
                ToolRegistry::new(),
            ))
        });
        let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
            thread_id.clone(),
            config,
            factory,
        ))
        .expect("runtime");
        let mut events = runtime.subscribe_events();

        runtime
            .enqueue_inter_agent_communication(InterAgentCommunication {
                author_thread_id: ThreadId::new("thread_child"),
                author_path: "/root/child".to_string(),
                recipient_thread_id: thread_id.clone(),
                recipient_path: "/root".to_string(),
                other_recipients: Vec::new(),
                content: "trigger child result".to_string(),
                trigger_turn: true,
                source_turn_id: Some(TurnId::new("turn_child")),
                created_at: "2026-06-12T00:00:00Z".to_string(),
            })
            .await
            .expect("enqueue mail");

        let _ = runtime
            .submit_control_and_wait(ThreadOp::Interrupt { turn_id: None })
            .await;
        wait_for_turn_completed(&mut events, &TurnId::new("turn_1")).await;

        let prompts = prompts.lock().expect("prompts");
        let prompt = prompts.first().expect("mail turn prompt");
        assert!(prompt.contains("inter_agent_communication"));
        assert!(prompt.contains("trigger child result"));
        assert!(prompt.contains(PENDING_MAIL_TURN_PROMPT));
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn checkpoint_restore_guard_blocks_goal_continuation_until_later_idle_check() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_checkpoint_restore_guard_goal_continue");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .expect("index db");
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Goal Restore Guard Project".to_string(),
                path: dir.path().to_path_buf(),
            })
            .await
            .expect("project");
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, 'Goal restore guard', 'Goal restore preview', 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(&project.id)
        .bind(format!("/tmp/{}/rollout.jsonl", thread_id.as_str()))
        .execute(db.pool())
        .await
        .expect("thread row");
        db.insert_thread_goal(&thread_id, "continue when restore clears", None)
            .await
            .expect("insert goal")
            .expect("new goal");

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let restoring = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = Arc::new(RestoreGateProbe {
            restoring: restoring.clone(),
            attempts: AtomicUsize::new(0),
        });
        let runtime = ThreadRuntime::spawn(
            ThreadRuntimeOptions::new(
                thread_id.clone(),
                config,
                blocking_agent_factory(started.clone(), release.clone()),
            )
            .with_goal_runtime(Arc::new(GoalRuntime::new(db.clone())))
            .with_workspace_runtime_op_gate(gate.clone()),
        )
        .expect("runtime");

        runtime
            .enqueue_goal_runtime_effect(GoalRuntimeEffect::ScheduleContinuation)
            .await
            .expect("queue goal effect");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), started.notified())
                .await
                .is_err(),
            "goal continuation must not start while checkpoint restore guard is active"
        );
        assert_eq!(runtime.active_turn_id(), None);
        assert!(gate.attempts.load(Ordering::SeqCst) > 0);

        restoring.store(false, Ordering::SeqCst);
        runtime
            .enqueue_goal_runtime_effect(GoalRuntimeEffect::ScheduleContinuation)
            .await
            .expect("queue goal effect after restore");
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .expect("goal continuation starts after restore guard drops");

        release.notify_one();
        wait_until_no_active_turn(&runtime).await;
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn rejected_busy_submit_does_not_consume_turn_id() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_rejected_submit_no_burn");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = blocking_first_runtime(thread_id, config, started.clone(), release.clone());

        let first_runtime = runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .submit_user_input_and_wait("first".to_string(), None)
                .await
        });

        started.notified().await;
        let rejected = runtime
            .submit_user_input("rejected while busy".to_string(), None)
            .await
            .expect_err("busy runtime should reject second submit");
        assert!(rejected.to_string().contains("thread is busy"));

        release.notify_one();
        let first_result = first.await.expect("first task").expect("first turn");
        let ThreadOpResult::UserInput { turn_id, .. } = first_result else {
            panic!("expected user input result");
        };
        assert_eq!(turn_id, TurnId::new("turn_1"));

        let next_result = runtime
            .submit_user_input_and_wait("next accepted".to_string(), None)
            .await
            .expect("next turn");
        let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
            panic!("expected user input result");
        };
        assert_eq!(turn_id, TurnId::new("turn_2"));
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn concurrent_submits_allocate_and_reserve_atomically() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_concurrent_submit_atomic");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime = blocking_first_runtime(thread_id, config, started.clone(), release.clone());
        let mut events = runtime.subscribe_events();

        let submit_a_runtime = runtime.clone();
        let submit_a = tokio::spawn(async move {
            submit_a_runtime
                .submit_user_input("concurrent a".to_string(), None)
                .await
        });
        let submit_b_runtime = runtime.clone();
        let submit_b = tokio::spawn(async move {
            submit_b_runtime
                .submit_user_input("concurrent b".to_string(), None)
                .await
        });

        let results = [
            submit_a.await.expect("submit a task"),
            submit_b.await.expect("submit b task"),
        ];
        let accepted: Vec<TurnId> = results
            .iter()
            .filter_map(|result| result.as_ref().ok().cloned())
            .collect();
        let rejected = results.iter().filter(|result| result.is_err()).count();
        assert_eq!(accepted, vec![TurnId::new("turn_1")]);
        assert_eq!(rejected, 1);

        started.notified().await;
        release.notify_one();
        wait_for_turn_completed(&mut events, &TurnId::new("turn_1")).await;

        let next_result = runtime
            .submit_user_input_and_wait("after concurrent".to_string(), None)
            .await
            .expect("next turn");
        let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
            panic!("expected user input result");
        };
        assert_eq!(turn_id, TurnId::new("turn_2"));
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn manual_compaction_reservation_rejects_concurrent_submit() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_manual_compaction_busy");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ])
            .expect("seed compaction history");
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime =
            blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

        let compact_runtime = runtime.clone();
        let compact = tokio::spawn(async move { compact_runtime.compact_now().await });
        started.notified().await;
        assert_eq!(runtime.active_turn_id(), None);

        let rejected = runtime
            .submit_user_input("rejected while compacting".to_string(), None)
            .await
            .expect_err("manual compaction should reserve the runtime");
        assert!(matches!(
            rejected.downcast_ref::<ThreadRuntimeError>(),
            Some(ThreadRuntimeError::ThreadBusy(busy_thread_id)) if busy_thread_id == &thread_id
        ));

        release.notify_one();
        compact
            .await
            .expect("compaction task")
            .expect("manual compaction");
        wait_until_no_active_turn(&runtime).await;
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn interrupt_during_manual_compaction_is_rejected_without_sentinel() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_compact_interrupt");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ])
            .expect("seed compaction history");
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime =
            blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

        let compact_runtime = runtime.clone();
        let compact = tokio::spawn(async move { compact_runtime.compact_now().await });
        started.notified().await;

        let rejected = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            runtime.interrupt_active_turn(None),
        )
        .await
        .expect("interrupt should not hang")
        .expect_err("manual compaction should not be interruptible");
        let message = rejected.to_string();
        assert!(message.contains("active operation is not interruptible"));
        assert!(!message.contains("manual_compaction"));

        release.notify_one();
        compact
            .await
            .expect("compaction task")
            .expect("manual compaction");
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn compact_now_rejects_while_user_turn_running() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_compact_rejected_busy_turn");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runtime =
            blocking_first_runtime(thread_id.clone(), config, started.clone(), release.clone());

        let first_runtime = runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .submit_user_input_and_wait("first".to_string(), None)
                .await
        });
        started.notified().await;
        assert_eq!(runtime.active_turn_id(), Some(TurnId::new("turn_1")));

        let rejected = runtime
            .compact_now()
            .await
            .expect_err("running turn should reject manual compaction");
        assert!(matches!(
            rejected.downcast_ref::<ThreadRuntimeError>(),
            Some(ThreadRuntimeError::ThreadBusy(busy_thread_id)) if busy_thread_id == &thread_id
        ));

        release.notify_one();
        first.await.expect("first task").expect("first turn");
        runtime.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn pre_start_failure_persists_returned_turn_id_before_error() {
        let dir = tempdir().expect("tempdir");
        let thread_id = ThreadId::new("thread_pre_start_failure_persists_id");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let factory_calls = Arc::new(AtomicUsize::new(0));
        let factory_call_counter = factory_calls.clone();
        let factory: AgentFactory = Arc::new(move |config| {
            let call_index = factory_call_counter.fetch_add(1, Ordering::SeqCst);
            if call_index == 1 {
                return Err(anyhow!("agent swap failed before sampling"));
            }
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("next turn complete".to_string()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }])),
                ToolRegistry::new(),
            ))
        });
        let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
            thread_id.clone(),
            config.clone(),
            factory,
        ))
        .expect("spawn runtime");
        let mut events = runtime.subscribe_events();
        let override_model = ResolvedModelConfig::from_provider_profile(
            "openai",
            "override-model",
            None,
            ResolvedCredential::None,
            None,
        );

        let failed_turn_id = runtime
            .submit_user_input(
                "use override model".to_string(),
                Some(ThreadTurnContext {
                    cwd: None,
                    resolved_model: Some(override_model),
                    thinking_mode: None,
                    clear_thinking_mode: false,
                    turn_mode: TurnMode::Default,
                }),
            )
            .await
            .expect("turn id is returned after TurnStarted is persisted");

        assert_eq!(failed_turn_id, TurnId::new("turn_1"));
        let message = wait_for_runtime_error(&mut events, &failed_turn_id).await;
        assert!(message.contains("agent swap failed before sampling"));
        wait_until_no_active_turn(&runtime).await;

        let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
        let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
            .await
            .expect("read rollout");
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(event)
                if event.turn_id.as_ref() == Some(&failed_turn_id)
                    && matches!(event.kind, crate::events::RuntimeEventKind::TurnStarted)
        )));
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(event)
                if event.turn_id.as_ref() == Some(&failed_turn_id)
                    && matches!(event.kind, crate::events::RuntimeEventKind::RuntimeError { .. })
        )));

        let next_result = runtime
            .submit_user_input_and_wait("next accepted".to_string(), None)
            .await
            .expect("next turn");
        let ThreadOpResult::UserInput { turn_id, .. } = next_result else {
            panic!("expected user input result");
        };
        assert_eq!(turn_id, TurnId::new("turn_2"));
        runtime.shutdown().await.expect("shutdown");
    }
}
