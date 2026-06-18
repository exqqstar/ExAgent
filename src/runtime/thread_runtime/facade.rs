use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

use super::actor::{spawn_runtime_loop, ThreadRuntimeLoop, ThreadSubmission};
use super::op::{ThreadOp, ThreadOpResult, ThreadRuntimeStatus, ThreadTurnContext};
use super::reservation::{ActiveRuntimeTurnGuard, TurnReservations};
use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::policy::PolicyManager;
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEffect, GoalRuntimeEvent};
use crate::runtime::memory::MemoryRuntime;
use crate::runtime::subagent::{AgentControl, InterAgentCommunication};
use crate::runtime::thread_session::{
    RuntimeInterrupt, ThreadInbox, ThreadSession, ThreadSessionLiveState, ThreadSessionLiveView,
    ThreadSessionOptions,
};
use crate::session::{ApprovalId, ApprovalStatus};
use crate::types::{ThreadId, TurnId, UserInput};

const THREAD_OP_CHANNEL_CAPACITY: usize = 64;
const THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;
pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;
pub(crate) type WorkspaceRuntimeOpPermit = Box<dyn Send + 'static>;

pub(crate) trait WorkspaceRuntimeOpGate: Send + Sync {
    fn begin_runtime_op(&self, workspace_root: &Path) -> Result<WorkspaceRuntimeOpPermit>;
}
pub struct ThreadRuntimeOptions {
    pub thread_id: ThreadId,
    pub config: AgentConfig,
    agent_factory: AgentFactory,
    policy: Arc<PolicyManager>,
    subagent_control: Option<Arc<AgentControl>>,
    goal_runtime: Option<Arc<GoalRuntime>>,
    memory_runtime: Option<Arc<MemoryRuntime>>,
    forge_review_store: Option<ReviewStore>,
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
            memory_runtime: None,
            forge_review_store: None,
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

    pub(crate) fn with_memory_runtime(mut self, memory_runtime: Arc<MemoryRuntime>) -> Self {
        self.memory_runtime = Some(memory_runtime);
        self
    }

    pub(crate) fn with_forge_review_store(mut self, store: ReviewStore) -> Self {
        self.forge_review_store = Some(store);
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
    turn_reservation: TurnReservations,
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
                .with_goal_runtime(options.goal_runtime)
                .with_memory_runtime(options.memory_runtime)
                .with_forge_review_store(options.forge_review_store),
        )?;
        let next_turn_index = session.next_turn_index_seed();
        let live_state = session.live_state_handle();
        let inbox = session.inbox_handle();

        let runtime = Arc::new(Self {
            thread_id: session.thread_id().clone(),
            op_tx,
            event_tx: event_tx.clone(),
            status_rx,
            turn_reservation: TurnReservations::new(next_turn_index),
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

    pub(super) async fn submit_control_and_wait(&self, op: ThreadOp) -> Result<ThreadOpResult> {
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
        self.turn_reservation.active_turn_id()
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
        self.turn_reservation
            .signal_interrupt(&self.thread_id, requested_turn_id)
            .await
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
        self.turn_reservation
            .reserve_next(&self.thread_id, interrupt_tx, interrupted)
    }

    fn reserve_manual_compaction_turn(&self) -> Result<ActiveRuntimeTurnGuard> {
        let (interrupt_tx, _interrupt_rx) = oneshot::channel();
        self.turn_reservation.reserve_record(
            &self.thread_id,
            None,
            interrupt_tx,
            Arc::new(Notify::new()),
        )
    }
}
