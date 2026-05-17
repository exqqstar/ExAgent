use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use std::future::Future;
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

use crate::agent::{Agent, AgentRunOutput};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::types::{SessionId, TurnId};

const THREAD_OP_CHANNEL_CAPACITY: usize = 64;
const THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;

pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRuntimeStatus {
    Idle,
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
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
    Shutdown,
}

pub enum ThreadOpResult {
    UserInput {
        turn_id: TurnId,
        output: AgentRunOutput,
    },
    Interrupted {
        turn_id: TurnId,
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
    pub thread_id: SessionId,
    pub config: AgentConfig,
    agent_factory: Option<AgentFactory>,
}

impl ThreadRuntimeOptions {
    pub fn new(thread_id: SessionId, config: AgentConfig) -> Self {
        Self {
            thread_id,
            config,
            agent_factory: None,
        }
    }

    pub fn with_agent_factory(mut self, agent_factory: AgentFactory) -> Self {
        self.agent_factory = Some(agent_factory);
        self
    }
}

pub struct ThreadRuntime {
    thread_id: SessionId,
    op_tx: mpsc::Sender<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_rx: watch::Receiver<ThreadRuntimeStatus>,
    active_turn: Arc<Mutex<Option<ActiveRuntimeTurnRecord>>>,
}

impl ThreadRuntime {
    pub fn spawn(options: ThreadRuntimeOptions) -> Result<Arc<Self>> {
        let (op_tx, op_rx) = mpsc::channel(THREAD_OP_CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = watch::channel(ThreadRuntimeStatus::Idle);

        let runtime = Arc::new(Self {
            thread_id: options.thread_id.clone(),
            op_tx,
            event_tx: event_tx.clone(),
            status_rx,
            active_turn: Arc::new(Mutex::new(None)),
        });

        spawn_runtime_loop(async move {
            ThreadRuntimeLoop {
                _thread_id: options.thread_id,
                config: options.config,
                agent_factory: options.agent_factory,
                op_rx,
                event_tx,
                status_tx,
            }
            .run()
            .await;
        });

        Ok(runtime)
    }

    pub fn thread_id(&self) -> &SessionId {
        &self.thread_id
    }

    pub fn status(&self) -> ThreadRuntimeStatus {
        *self.status_rx.borrow()
    }

    pub async fn submit(&self, op: ThreadOp) -> Result<()> {
        self.op_tx
            .send(ThreadSubmission {
                op,
                completion_tx: None,
                interrupt: None,
                _active_turn_guard: None,
            })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))
    }

    pub async fn submit_and_wait(&self, op: ThreadOp) -> Result<ThreadOpResult> {
        self.submit_and_wait_internal(op, None).await
    }

    pub(crate) async fn submit_user_input_and_wait(
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
                return Err(AppServerError::TurnRejected {
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
            return Err(AppServerError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "active turn is already interrupting or completed".to_string(),
            }
            .into());
        }
        record.interrupted.notified().await;
        Ok(record.turn_id)
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
            return Err(AppServerError::ThreadBusy(self.thread_id.clone()).into());
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
    _thread_id: SessionId,
    config: AgentConfig,
    agent_factory: Option<AgentFactory>,
    op_rx: mpsc::Receiver<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
}

struct RuntimeInterrupt {
    interrupt_rx: oneshot::Receiver<()>,
    interrupted: Arc<Notify>,
}

impl ThreadRuntimeLoop {
    async fn run(mut self) {
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
                        .run_user_input(turn_id, prompt, turn_context, submission.interrupt)
                        .await;
                    complete(submission.completion_tx, result);
                }
                ThreadOp::Interrupt { turn_id } => {
                    let result = turn_id
                        .map(|turn_id| ThreadOpResult::Interrupted { turn_id })
                        .ok_or_else(|| anyhow!("thread has no active turn"));
                    complete(submission.completion_tx, result);
                }
            }
        }
        let _ = self.status_tx.send(ThreadRuntimeStatus::Stopped);
    }

    async fn run_user_input(
        &self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let _ = self.status_tx.send(ThreadRuntimeStatus::Running);
        let result = self
            .run_user_input_inner(turn_id, prompt, turn_context, interrupt)
            .await;
        let _ = self.status_tx.send(ThreadRuntimeStatus::Idle);
        result
    }

    async fn run_user_input_inner(
        &self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let session_id = self._thread_id.clone();
        self.append_and_broadcast(
            &self.config.workspace_root,
            &session_id,
            Some(&turn_id),
            RuntimeEventKind::TurnStarted,
        )?;
        let broadcasted_event_count =
            crate::transcript::read_session_events(&self.config.workspace_root, &session_id)?.len();

        let agent_factory = self
            .agent_factory
            .as_ref()
            .ok_or_else(|| anyhow!("thread runtime has no agent factory"))?;
        let agent = agent_factory(self.config.clone())?;
        let turn_cwd = turn_context.and_then(|context| context.cwd);
        let output = if let Some(interrupt) = interrupt {
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = agent.resume_with_turn_id_cwd(&session_id, &prompt, runtime_turn_id, turn_cwd) => {
                    match result {
                        Ok(output) => output,
                        Err(err) => {
                            self.broadcast_events_since(&session_id, broadcasted_event_count)?;
                            let message = err.to_string();
                            self.append_and_broadcast(
                                &self.config.workspace_root,
                                &session_id,
                                Some(&turn_id),
                                RuntimeEventKind::RuntimeError { message },
                            )?;
                            return Err(err);
                        }
                    }
                }
                _ = interrupt.interrupt_rx => {
                    self.append_and_broadcast(
                        &self.config.workspace_root,
                        &session_id,
                        Some(&turn_id),
                        RuntimeEventKind::TurnInterrupted,
                    )?;
                    interrupt.interrupted.notify_one();
                    return Err(AppServerError::TurnInterrupted { thread_id: session_id, turn_id }.into());
                }
            }
        } else {
            let result = agent
                .resume_with_turn_id_cwd(&session_id, &prompt, turn_id.clone(), turn_cwd)
                .await;
            match result {
                Ok(output) => output,
                Err(err) => {
                    self.broadcast_events_since(&session_id, broadcasted_event_count)?;
                    let message = err.to_string();
                    self.append_and_broadcast(
                        &self.config.workspace_root,
                        &session_id,
                        Some(&turn_id),
                        RuntimeEventKind::RuntimeError { message },
                    )?;
                    return Err(err);
                }
            }
        };

        self.broadcast_events_since(&session_id, broadcasted_event_count)?;
        self.append_and_broadcast(
            &self.config.workspace_root,
            &session_id,
            Some(&turn_id),
            RuntimeEventKind::TurnCompleted,
        )?;

        Ok(ThreadOpResult::UserInput { turn_id, output })
    }

    fn append_and_broadcast(
        &self,
        workspace_root: &std::path::Path,
        session_id: &SessionId,
        turn_id: Option<&TurnId>,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent> {
        let event =
            crate::transcript::append_runtime_event(workspace_root, session_id, turn_id, kind)?;
        let _ = self.event_tx.send(event.clone());
        Ok(event)
    }

    fn broadcast_events_since(&self, session_id: &SessionId, event_count: usize) -> Result<()> {
        for event in
            crate::transcript::read_session_events(&self.config.workspace_root, session_id)?
                .into_iter()
                .skip(event_count)
        {
            let _ = self.event_tx.send(event);
        }
        Ok(())
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
