use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{anyhow, Result};
use std::future::Future;
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

use crate::agent::Agent;
use crate::config::{AgentConfig, ThinkingMode};
use crate::events::RuntimeEvent;
use crate::policy::PolicyManager;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::thread_session::{
    RuntimeInterrupt, ThreadSession, ThreadSessionLiveState, ThreadSessionLiveView,
    ThreadSessionOptions,
};
use crate::session::{ApprovalId, ApprovalStatus};
use crate::types::{AssistantTurn, ThreadId, TurnId};

const THREAD_OP_CHANNEL_CAPACITY: usize = 64;
const THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;

pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;

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
}

#[derive(Debug, Clone)]
pub enum ThreadOp {
    UserInput {
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
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
    Ack,
}

pub struct ThreadSubmission {
    pub op: ThreadOp,
    completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    interrupt: Option<RuntimeInterrupt>,
    _active_turn_guard: Option<ActiveRuntimeTurnGuard>,
}

pub struct ThreadRuntimeOptions {
    pub thread_id: ThreadId,
    pub config: AgentConfig,
    agent_factory: AgentFactory,
    policy: Arc<PolicyManager>,
}

impl ThreadRuntimeOptions {
    pub fn new(thread_id: ThreadId, config: AgentConfig, agent_factory: AgentFactory) -> Self {
        Self {
            thread_id,
            config,
            agent_factory,
            policy: Arc::new(PolicyManager::default()),
        }
    }

    pub fn with_policy(mut self, policy: Arc<PolicyManager>) -> Self {
        self.policy = policy;
        self
    }
}

pub struct ThreadRuntime {
    thread_id: ThreadId,
    op_tx: mpsc::Sender<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_rx: watch::Receiver<ThreadRuntimeStatus>,
    active_turn: Arc<Mutex<Option<ActiveRuntimeTurnRecord>>>,
    live_state: Arc<RwLock<ThreadSessionLiveState>>,
}

impl ThreadRuntime {
    pub fn spawn(options: ThreadRuntimeOptions) -> Result<Arc<Self>> {
        let (op_tx, op_rx) = mpsc::channel(THREAD_OP_CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = watch::channel(ThreadRuntimeStatus::Idle);

        let session = ThreadSession::new(
            ThreadSessionOptions::new(options.thread_id, options.config, options.agent_factory)
                .with_event_tx(event_tx.clone())
                .with_status_tx(status_tx)
                .with_policy(options.policy),
        )?;
        let live_state = session.live_state_handle();

        let runtime = Arc::new(Self {
            thread_id: session.thread_id().clone(),
            op_tx,
            event_tx: event_tx.clone(),
            status_rx,
            active_turn: Arc::new(Mutex::new(None)),
            live_state,
        });

        spawn_runtime_loop(async move {
            ThreadRuntimeLoop { op_rx, session }.run().await;
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
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<ThreadOpResult> {
        let (completion_tx, completion_rx) = oneshot::channel();
        self.send_user_input(turn_id, prompt, turn_context, Some(completion_tx))
            .await?;
        completion_rx
            .await
            .map_err(|_| anyhow!("thread runtime stopped before completing op"))?
    }

    pub(crate) async fn submit_user_input(
        &self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
    ) -> Result<()> {
        self.send_user_input(turn_id, prompt, turn_context, None)
            .await
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self.submit_control_and_wait(ThreadOp::Shutdown).await? {
            ThreadOpResult::Ack => Ok(()),
            _ => Err(anyhow!("shutdown returned non-ack runtime result")),
        }
    }

    async fn send_user_input(
        &self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        completion_tx: Option<oneshot::Sender<Result<ThreadOpResult>>>,
    ) -> Result<()> {
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        let guard = self.reserve_active_turn(turn_id.clone(), interrupt_tx, interrupted.clone())?;
        self.op_tx
            .send(ThreadSubmission {
                op: ThreadOp::UserInput {
                    turn_id,
                    prompt,
                    turn_context,
                },
                completion_tx,
                interrupt: Some(RuntimeInterrupt {
                    interrupt_rx,
                    interrupted,
                }),
                _active_turn_guard: Some(guard),
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))?;
        Ok(())
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
                completion_tx: Some(completion_tx),
                interrupt,
                _active_turn_guard: None,
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

    pub fn next_turn_id(&self) -> TurnId {
        ThreadSession::next_turn_id_from_state(&self.live_state)
    }

    pub(crate) fn active_turn_id(&self) -> Option<TurnId> {
        self.active_turn
            .lock()
            .ok()
            .and_then(|active_turn| active_turn.as_ref().map(|record| record.turn_id.clone()))
    }

    pub(crate) async fn interrupt_active_turn(
        &self,
        requested_turn_id: Option<&TurnId>,
    ) -> Result<TurnId> {
        let record = self
            .active_turn
            .lock()
            .ok()
            .and_then(|active_turn| active_turn.clone())
            .ok_or_else(|| anyhow!("thread has no active turn"))?;
        if let Some(requested_turn_id) = requested_turn_id {
            if requested_turn_id != &record.turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("active turn is {}", record.turn_id.as_str()),
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
        Ok(record.turn_id)
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

    fn reserve_active_turn(
        &self,
        turn_id: TurnId,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<ActiveRuntimeTurnGuard> {
        let mut active_turn = self.active_turn.lock().expect("active turn mutex poisoned");
        if active_turn.is_some() {
            return Err(ThreadRuntimeError::ThreadBusy(self.thread_id.clone()).into());
        }
        *active_turn = Some(ActiveRuntimeTurnRecord {
            turn_id,
            interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
            interrupted,
        });

        Ok(ActiveRuntimeTurnGuard {
            active_turn: self.active_turn.clone(),
        })
    }
}

#[derive(Clone)]
struct ActiveRuntimeTurnRecord {
    turn_id: TurnId,
    interrupt_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    interrupted: Arc<Notify>,
}

struct ActiveRuntimeTurnGuard {
    active_turn: Arc<Mutex<Option<ActiveRuntimeTurnRecord>>>,
}

impl Drop for ActiveRuntimeTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut active_turn) = self.active_turn.lock() {
            *active_turn = None;
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
    op_rx: mpsc::Receiver<ThreadSubmission>,
    session: ThreadSession,
}

impl ThreadRuntimeLoop {
    async fn run(mut self) {
        let _stopped = self.session.stopped_guard();
        while let Some(submission) = self.op_rx.recv().await {
            match submission.op {
                ThreadOp::Shutdown => {
                    complete(submission.completion_tx, Ok(ThreadOpResult::Ack));
                    break;
                }
                ThreadOp::UserInput {
                    turn_id,
                    prompt,
                    turn_context,
                } => {
                    let result = self
                        .session
                        .handle_user_input(turn_id, prompt, turn_context, submission.interrupt)
                        .await;
                    complete(submission.completion_tx, result);
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
            }
        }
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
