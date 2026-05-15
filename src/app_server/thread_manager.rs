use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};

use crate::agent::{Agent, AgentRunOutput};
use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    AgentRunResponse, BoundaryCapability, BoundaryOp, BoundaryOpResponse, CollectParams,
    CollectResponse, EventsReplayParams, EventsReplayResponse, ForkParams, IgnoredOverrideField,
    InitializeParams, InitializeResponse, InspectParams, InspectResponse, ReplaySnapshotView,
    RunParams, RuntimeEventKindFilter, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadSpawnChildParams, ThreadSpawnChildResponse, ThreadStartParams,
    ThreadStartResponse, ThreadStatus, TurnInterruptParams, TurnInterruptResponse, TurnStartParams,
    TurnStartResponse, TurnState, TurnStatus, BOUNDARY_PROTOCOL_VERSION,
};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::events::RuntimeEventKind;
use crate::exec_session::ExecSessionManager;
use crate::llm::{LlmClient, OpenAiCompatibleLlm};
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::session::SessionSnapshot;
use crate::types::{SessionId, TurnId};

type RegistryFactory = Arc<dyn Fn() -> ToolRegistry + Send + Sync>;

trait LlmFactory: Send + Sync {
    fn build(&self, config: &AgentConfig) -> Result<Box<dyn LlmClient>>;
}

struct EnvLlmFactory;

impl LlmFactory for EnvLlmFactory {
    fn build(&self, _config: &AgentConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(OpenAiCompatibleLlm::from_env()?))
    }
}

struct SharedLlmFactory {
    llm: Arc<dyn LlmClient>,
}

impl LlmFactory for SharedLlmFactory {
    fn build(&self, _config: &AgentConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(SharedLlmClient {
            llm: self.llm.clone(),
        }))
    }
}

struct SharedLlmClient {
    llm: Arc<dyn LlmClient>,
}

#[async_trait]
impl LlmClient for SharedLlmClient {
    async fn complete(
        &self,
        messages: &[crate::types::ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<crate::types::AssistantTurn> {
        self.llm.complete(messages, tools).await
    }
}

#[derive(Clone)]
struct ActiveTurnRecord {
    turn_id: TurnId,
    interrupt_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    interrupted: Arc<Notify>,
}

pub struct ThreadManager {
    base_config: AgentConfig,
    llm_factory: Arc<dyn LlmFactory>,
    registry_factory: RegistryFactory,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    active_turns: Arc<Mutex<HashMap<String, ActiveTurnRecord>>>,
}

impl Default for ThreadManager {
    fn default() -> Self {
        Self::from_env(AgentConfig::default())
    }
}

impl ThreadManager {
    pub fn from_env(base_config: AgentConfig) -> Self {
        Self {
            base_config,
            llm_factory: Arc::new(EnvLlmFactory),
            registry_factory: Arc::new(crate::default_tool_registry),
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_llm<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            base_config,
            llm_factory: Arc::new(SharedLlmFactory {
                llm: Arc::from(llm),
            }),
            registry_factory: Arc::new(registry_factory),
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
            active_turns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        let workspace_root = params.workspace_root.clone();
        let thread_id = match params.session_id {
            Some(thread_id) => {
                self.thread_resume(ThreadResumeParams {
                    thread_id,
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                })?
                .thread
                .thread_id
            }
            None => {
                self.thread_start(ThreadStartParams {
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                })?
                .thread_id
            }
        };

        Ok(self
            .turn_start(TurnStartParams {
                thread_id,
                prompt: params.prompt,
                workspace_root,
                turn_context: None,
            })
            .await?
            .output)
    }

    pub fn initialize(&self, _params: InitializeParams) -> InitializeResponse {
        InitializeResponse {
            protocol_version: BOUNDARY_PROTOCOL_VERSION.to_string(),
            supported_ops: vec![
                BoundaryCapability::Initialize,
                BoundaryCapability::ThreadStart,
                BoundaryCapability::ThreadResume,
                BoundaryCapability::ThreadSpawnChild,
                BoundaryCapability::ThreadRead,
                BoundaryCapability::TurnStart,
                BoundaryCapability::TurnInterrupt,
                BoundaryCapability::EventsReplay,
            ],
        }
    }

    pub async fn fork(&self, params: ForkParams) -> Result<AgentRunResponse> {
        Ok(self
            .thread_spawn_child(ThreadSpawnChildParams {
                parent_thread_id: params.parent_session_id,
                agent_role: params.agent_role,
                prompt: params.prompt,
                workspace_root: params.workspace_root,
                cwd: None,
                spawned_by_turn_id: params.spawned_by_turn_id,
            })
            .await?
            .output)
    }

    pub fn inspect(&self, params: InspectParams) -> Result<InspectResponse> {
        let config = OverridePolicy::merge_thread_read(&self.base_config, params.workspace_root)?;
        Ok(InspectResponse {
            children: crate::orchestration::inspect_children(
                &config.workspace_root,
                &params.parent_session_id,
            )?,
        })
    }

    pub fn collect(&self, params: CollectParams) -> Result<CollectResponse> {
        let config = OverridePolicy::merge_thread_read(&self.base_config, params.workspace_root)?;
        Ok(CollectResponse {
            session: crate::orchestration::collect_session(
                &config.workspace_root,
                &params.session_id,
            )?,
        })
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        let config = OverridePolicy::merge_thread_start(
            &self.base_config,
            RuntimeOverrides {
                workspace_root: params.workspace_root,
                cwd: params.cwd,
            },
        )?;
        let thread_id = crate::transcript::new_session_id();
        let snapshot =
            SessionSnapshot::new_thread(thread_id.clone(), config.workspace_root, config.cwd);
        let paths = crate::transcript::session_paths(&snapshot.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;

        Ok(ThreadStartResponse {
            thread_id,
            snapshot_path: paths.snapshot_path,
            events_path: paths.events_path,
        })
    }

    pub fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse> {
        let config = OverridePolicy::merge_thread_read(&self.base_config, params.workspace_root)?;
        self.thread_read_resolved(params.thread_id, &config.workspace_root)
    }

    fn thread_read_resolved(
        &self,
        thread_id: SessionId,
        workspace_root: &std::path::Path,
    ) -> Result<ThreadReadResponse> {
        let paths = crate::transcript::session_paths(workspace_root, &thread_id);
        if !paths.snapshot_path.exists() {
            return Err(AppServerError::ThreadNotFound(thread_id).into());
        }

        let snapshot = crate::transcript::read_session_snapshot(workspace_root, &thread_id)?;
        let events = crate::transcript::read_session_events(workspace_root, &thread_id)?;
        let active_turn = self.active_turn_state(&thread_id);
        let latest_turn = latest_turn_state(&events);
        let has_pending_approval = snapshot
            .pending_approvals
            .iter()
            .any(|approval| matches!(approval.status, crate::session::ApprovalStatus::Pending));
        let status = if active_turn.is_some() {
            ThreadStatus::Running
        } else if has_pending_approval {
            ThreadStatus::WaitingApproval
        } else if latest_turn
            .as_ref()
            .is_some_and(|turn| turn.status == TurnStatus::Failed)
        {
            ThreadStatus::Failed
        } else {
            ThreadStatus::Idle
        };

        Ok(ThreadReadResponse {
            thread_id,
            status,
            active_turn,
            latest_turn,
            snapshot_path: paths.snapshot_path,
            events_path: paths.events_path,
        })
    }

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        let ignored_overrides = ignored_resume_overrides(&params);
        let config = OverridePolicy::merge_thread_resume(&self.base_config, params.workspace_root)?;
        let thread = self.thread_read_resolved(params.thread_id, &config.workspace_root)?;

        Ok(ThreadResumeResponse {
            thread,
            ignored_overrides,
        })
    }

    pub async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        match self
            .submit_boundary_op(BoundaryOp::TurnStart(params))
            .await?
        {
            BoundaryOpResponse::TurnStarted(response) => Ok(response),
            response => Err(unexpected_boundary_response("turn_start", &response).into()),
        }
    }

    pub async fn turn_interrupt(
        &self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResponse> {
        let config =
            OverridePolicy::merge_turn_start(&self.base_config, params.workspace_root.clone())?;
        let paths = crate::transcript::session_paths(&config.workspace_root, &params.thread_id);
        if !paths.snapshot_path.exists() {
            return Err(AppServerError::ThreadNotFound(params.thread_id).into());
        }

        let Some(record) = self.active_turn_record(&params.thread_id) else {
            return self
                .interrupt_waiting_approval_turn(params, &config.workspace_root)
                .await;
        };
        if let Some(requested_turn_id) = params.turn_id.as_ref() {
            if requested_turn_id != &record.turn_id {
                return Err(AppServerError::TurnRejected {
                    thread_id: params.thread_id,
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
                thread_id: params.thread_id,
                reason: "active turn is already interrupting or completed".to_string(),
            }
            .into());
        }
        record.interrupted.notified().await;

        Ok(TurnInterruptResponse {
            thread_id: params.thread_id,
            interrupted_turn: Some(TurnState {
                turn_id: record.turn_id,
                status: TurnStatus::Interrupted,
            }),
        })
    }

    async fn interrupt_waiting_approval_turn(
        &self,
        params: TurnInterruptParams,
        workspace_root: &std::path::Path,
    ) -> Result<TurnInterruptResponse> {
        let mut snapshot =
            crate::transcript::read_session_snapshot(workspace_root, &params.thread_id)?;
        let had_pending_approval = snapshot
            .pending_approvals
            .iter()
            .any(|approval| matches!(approval.status, crate::session::ApprovalStatus::Pending));
        if !had_pending_approval {
            return Err(AppServerError::TurnRejected {
                thread_id: params.thread_id,
                reason: "thread has no active turn".to_string(),
            }
            .into());
        }

        let latest_turn = latest_turn_state(&crate::transcript::read_session_events(
            workspace_root,
            &params.thread_id,
        )?);
        let turn_id = params
            .turn_id
            .clone()
            .or_else(|| latest_turn.as_ref().map(|turn| turn.turn_id.clone()))
            .ok_or_else(|| AppServerError::TurnRejected {
                thread_id: params.thread_id.clone(),
                reason: "waiting approval has no turn id".to_string(),
            })?;
        if let Some(latest_turn) = latest_turn.as_ref() {
            if latest_turn.turn_id != turn_id {
                return Err(AppServerError::TurnRejected {
                    thread_id: params.thread_id,
                    reason: format!("waiting approval turn is {}", latest_turn.turn_id.as_str()),
                }
                .into());
            }
        }

        snapshot
            .pending_approvals
            .retain(|approval| !matches!(approval.status, crate::session::ApprovalStatus::Pending));
        let paths = crate::transcript::session_paths(workspace_root, &params.thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;
        self.policy
            .cancel_pending_for_session(&params.thread_id)
            .await;
        crate::transcript::append_runtime_event(
            workspace_root,
            &params.thread_id,
            Some(&turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;

        Ok(TurnInterruptResponse {
            thread_id: params.thread_id,
            interrupted_turn: Some(TurnState {
                turn_id,
                status: TurnStatus::Interrupted,
            }),
        })
    }

    async fn turn_start_direct(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        let config = OverridePolicy::merge_turn_start(&self.base_config, params.workspace_root)?;
        let thread_id = params.thread_id;
        let turn_id = next_turn_id(&config.workspace_root, &thread_id)?;
        let ActiveTurnReservation {
            _guard: _active_turn,
            interrupt_rx,
            interrupted,
        } = self.reserve_active_turn(&thread_id, turn_id.clone())?;
        let turn_cwd = if let Some(turn_context) = params.turn_context {
            let snapshot =
                crate::transcript::read_session_snapshot(&config.workspace_root, &thread_id)?;
            let snapshot = OverridePolicy::apply_turn_context(&snapshot, turn_context)?;
            Some(snapshot.cwd)
        } else {
            None
        };
        crate::transcript::append_runtime_event(
            &config.workspace_root,
            &thread_id,
            Some(&turn_id),
            RuntimeEventKind::TurnStarted,
        )?;
        let agent = self.agent_for(config.clone())?;
        let run_thread_id = thread_id.clone();
        let prompt = params.prompt;
        let output = tokio::select! {
            result = agent
                .resume_with_turn_cwd(&run_thread_id, &prompt, turn_cwd)
                => match result {
                    Ok(output) => output,
                    Err(err) => {
                        let message = err.to_string();
                        let _ = crate::transcript::append_runtime_event(
                            &config.workspace_root,
                            &thread_id,
                            Some(&turn_id),
                            RuntimeEventKind::RuntimeError { message },
                        );
                        return Err(err);
                    }
                },
            _ = interrupt_rx => {
                let append_result = crate::transcript::append_runtime_event(
                    &config.workspace_root,
                    &thread_id,
                    Some(&turn_id),
                    RuntimeEventKind::TurnInterrupted,
                );
                interrupted.notify_one();
                append_result?;
                return Err(AppServerError::TurnInterrupted { thread_id, turn_id }.into());
            }
        };
        crate::transcript::append_runtime_event(
            &config.workspace_root,
            &thread_id,
            Some(&turn_id),
            RuntimeEventKind::TurnCompleted,
        )?;

        Ok(TurnStartResponse {
            thread_id,
            turn_id,
            output: agent_run_response(output),
        })
    }

    pub async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse> {
        match self
            .submit_boundary_op(BoundaryOp::ThreadSpawnChild(params))
            .await?
        {
            BoundaryOpResponse::ThreadChildSpawned(response) => Ok(response),
            response => Err(unexpected_boundary_response("thread_spawn_child", &response).into()),
        }
    }

    async fn thread_spawn_child_direct(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse> {
        let ignored_overrides = ignored_child_overrides(&params);
        let config =
            OverridePolicy::merge_thread_spawn_child(&self.base_config, params.workspace_root)?;
        let agent = self.agent_for(config)?;
        let output = agent
            .fork_session(
                &params.parent_thread_id,
                params.agent_role.clone(),
                &params.prompt,
                params.spawned_by_turn_id.as_ref(),
            )
            .await?;
        let child_thread_id = output.session_id.clone();

        Ok(ThreadSpawnChildResponse {
            parent_thread_id: params.parent_thread_id,
            child_thread_id,
            agent_role: params.agent_role,
            ignored_overrides,
            output: agent_run_response(output),
        })
    }

    pub async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        match op {
            BoundaryOp::Initialize(params) => {
                Ok(BoundaryOpResponse::Initialized(self.initialize(params)))
            }
            BoundaryOp::ThreadStart(params) => self
                .thread_start(params)
                .map(BoundaryOpResponse::ThreadStarted),
            BoundaryOp::ThreadRead(params) => {
                self.thread_read(params).map(BoundaryOpResponse::ThreadRead)
            }
            BoundaryOp::ThreadResume(params) => self
                .thread_resume(params)
                .map(BoundaryOpResponse::ThreadResumed),
            BoundaryOp::TurnStart(params) => self
                .turn_start_direct(params)
                .await
                .map(BoundaryOpResponse::TurnStarted),
            BoundaryOp::TurnInterrupt(params) => self
                .turn_interrupt(params)
                .await
                .map(BoundaryOpResponse::TurnInterrupted),
            BoundaryOp::ThreadSpawnChild(params) => self
                .thread_spawn_child_direct(params)
                .await
                .map(BoundaryOpResponse::ThreadChildSpawned),
            BoundaryOp::EventsReplay(params) => self
                .events_replay(params)
                .map(BoundaryOpResponse::EventsReplayed),
        }
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        let config =
            OverridePolicy::merge_events_replay(&self.base_config, params.workspace_root.clone())?;
        let paths = crate::transcript::session_paths(&config.workspace_root, &params.thread_id);
        if !paths.snapshot_path.exists() {
            return Err(AppServerError::ThreadNotFound(params.thread_id).into());
        }
        let events = filter_replay_events(
            crate::transcript::replay_session(&config.workspace_root, &params.thread_id)?,
            &params,
        );
        let snapshot = if params.include_snapshot {
            let snapshot = crate::transcript::read_session_snapshot(
                &config.workspace_root,
                &params.thread_id,
            )?;
            Some(replay_snapshot_view(snapshot))
        } else {
            None
        };

        Ok(EventsReplayResponse {
            thread_id: params.thread_id,
            events,
            snapshot,
        })
    }

    fn agent_for(&self, config: AgentConfig) -> Result<Agent> {
        let llm = self.llm_factory.build(&config)?;
        Ok(Agent::with_runtime(
            config,
            llm,
            (self.registry_factory)(),
            self.exec_sessions.clone(),
            self.policy.clone(),
        ))
    }

    fn active_turn_state(&self, thread_id: &SessionId) -> Option<TurnState> {
        self.active_turn_record(thread_id).map(|record| TurnState {
            turn_id: record.turn_id,
            status: TurnStatus::Running,
        })
    }

    fn active_turn_record(&self, thread_id: &SessionId) -> Option<ActiveTurnRecord> {
        self.active_turns
            .lock()
            .ok()
            .and_then(|active_turns| active_turns.get(thread_id.as_str()).cloned())
    }

    fn reserve_active_turn(
        &self,
        thread_id: &SessionId,
        turn_id: TurnId,
    ) -> Result<ActiveTurnReservation> {
        let mut active_turns = self
            .active_turns
            .lock()
            .expect("active turn mutex poisoned");
        if active_turns.contains_key(thread_id.as_str()) {
            return Err(AppServerError::ThreadBusy(thread_id.clone()).into());
        }
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        active_turns.insert(
            thread_id.as_str().to_string(),
            ActiveTurnRecord {
                turn_id,
                interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
                interrupted: interrupted.clone(),
            },
        );

        Ok(ActiveTurnReservation {
            _guard: ActiveTurnGuard {
                active_turns: self.active_turns.clone(),
                thread_id: thread_id.as_str().to_string(),
            },
            interrupt_rx,
            interrupted,
        })
    }
}

struct ActiveTurnReservation {
    _guard: ActiveTurnGuard,
    interrupt_rx: oneshot::Receiver<()>,
    interrupted: Arc<Notify>,
}

struct ActiveTurnGuard {
    active_turns: Arc<Mutex<HashMap<String, ActiveTurnRecord>>>,
    thread_id: String,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut active_turns) = self.active_turns.lock() {
            active_turns.remove(&self.thread_id);
        }
    }
}

fn ignored_child_overrides(params: &ThreadSpawnChildParams) -> Vec<IgnoredOverrideField> {
    let mut ignored = Vec::new();
    if params.cwd.is_some() {
        ignored.push(IgnoredOverrideField::Cwd);
    }
    ignored
}

fn ignored_resume_overrides(params: &ThreadResumeParams) -> Vec<IgnoredOverrideField> {
    let mut ignored = Vec::new();
    if params.cwd.is_some() {
        ignored.push(IgnoredOverrideField::Cwd);
    }
    ignored
}

fn unexpected_boundary_response(operation: &str, response: &BoundaryOpResponse) -> AppServerError {
    AppServerError::InvalidRequest(format!(
        "{operation} returned unexpected {} response",
        boundary_response_name(response)
    ))
}

fn boundary_response_name(response: &BoundaryOpResponse) -> &'static str {
    match response {
        BoundaryOpResponse::Initialized(_) => "initialized",
        BoundaryOpResponse::ThreadStarted(_) => "thread_started",
        BoundaryOpResponse::ThreadRead(_) => "thread_read",
        BoundaryOpResponse::ThreadResumed(_) => "thread_resumed",
        BoundaryOpResponse::TurnStarted(_) => "turn_started",
        BoundaryOpResponse::TurnInterrupted(_) => "turn_interrupted",
        BoundaryOpResponse::ThreadChildSpawned(_) => "thread_child_spawned",
        BoundaryOpResponse::EventsReplayed(_) => "events_replayed",
    }
}

fn next_turn_id(workspace_root: &std::path::Path, thread_id: &SessionId) -> Result<TurnId> {
    let snapshot = crate::transcript::read_session_snapshot(workspace_root, thread_id)?;
    let assistant_turn_count = snapshot
        .conversation
        .iter()
        .filter(|message| matches!(message.role, crate::types::MessageRole::Assistant))
        .count();
    Ok(TurnId::new(format!("turn_{}", assistant_turn_count + 1)))
}

fn latest_turn_state(events: &[crate::events::RuntimeEvent]) -> Option<TurnState> {
    events.iter().rev().find_map(|event| {
        let turn_id = event.turn_id.clone()?;
        let status = match &event.kind {
            RuntimeEventKind::TurnStarted => TurnStatus::Running,
            RuntimeEventKind::TurnCompleted => TurnStatus::Completed,
            RuntimeEventKind::TurnInterrupted => TurnStatus::Interrupted,
            RuntimeEventKind::RuntimeError { .. } => TurnStatus::Failed,
            RuntimeEventKind::ApprovalRequested { .. } => TurnStatus::WaitingApproval,
            RuntimeEventKind::AssistantTurn { .. } => TurnStatus::Completed,
            RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ExecOutput { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::SessionSpawned { .. }
            | RuntimeEventKind::CompactionWritten { .. }
            | RuntimeEventKind::StructuredResultRecorded { .. } => TurnStatus::Running,
        };
        Some(TurnState { turn_id, status })
    })
}

fn filter_replay_events(
    events: Vec<crate::events::RuntimeEvent>,
    params: &EventsReplayParams,
) -> Vec<crate::events::RuntimeEvent> {
    let after_index = params.after_event_id.as_ref().and_then(|after_event_id| {
        events
            .iter()
            .position(|event| &event.event_id == after_event_id)
    });

    let events = events
        .into_iter()
        .enumerate()
        .filter(|(index, _event)| after_index.map_or(true, |after_index| *index > after_index))
        .map(|(_index, event)| event)
        .filter(|event| {
            params.event_kinds.is_empty()
                || params
                    .event_kinds
                    .iter()
                    .any(|filter| runtime_event_kind_matches(filter, &event.kind))
        });

    match params.limit {
        Some(limit) => events.take(limit).collect(),
        None => events.collect(),
    }
}

fn runtime_event_kind_matches(filter: &RuntimeEventKindFilter, kind: &RuntimeEventKind) -> bool {
    matches!(
        (filter, kind),
        (
            RuntimeEventKindFilter::TurnStarted,
            RuntimeEventKind::TurnStarted
        ) | (
            RuntimeEventKindFilter::TurnCompleted,
            RuntimeEventKind::TurnCompleted,
        ) | (
            RuntimeEventKindFilter::TurnInterrupted,
            RuntimeEventKind::TurnInterrupted,
        ) | (
            RuntimeEventKindFilter::AssistantTurn,
            RuntimeEventKind::AssistantTurn { .. },
        ) | (
            RuntimeEventKindFilter::ToolResult,
            RuntimeEventKind::ToolResult { .. },
        ) | (
            RuntimeEventKindFilter::SessionSpawned,
            RuntimeEventKind::SessionSpawned { .. },
        ) | (
            RuntimeEventKindFilter::ExecOutput,
            RuntimeEventKind::ExecOutput { .. },
        ) | (
            RuntimeEventKindFilter::ApprovalRequested,
            RuntimeEventKind::ApprovalRequested { .. },
        ) | (
            RuntimeEventKindFilter::ApprovalDecision,
            RuntimeEventKind::ApprovalDecision { .. },
        ) | (
            RuntimeEventKindFilter::CompactionWritten,
            RuntimeEventKind::CompactionWritten { .. },
        ) | (
            RuntimeEventKindFilter::StructuredResultRecorded,
            RuntimeEventKind::StructuredResultRecorded { .. },
        ) | (
            RuntimeEventKindFilter::RuntimeError,
            RuntimeEventKind::RuntimeError { .. },
        )
    )
}

fn replay_snapshot_view(snapshot: SessionSnapshot) -> ReplaySnapshotView {
    ReplaySnapshotView {
        thread_id: snapshot.session_id,
        cwd: snapshot.cwd,
        latest_compaction: snapshot.latest_compaction,
        open_exec_session_count: snapshot.open_exec_sessions.len(),
        conversation_message_count: snapshot.conversation.len(),
        pending_approval_count: snapshot.pending_approvals.len(),
    }
}

fn agent_run_response(output: AgentRunOutput) -> AgentRunResponse {
    AgentRunResponse {
        text: output.final_turn.text,
        tool_calls: output.final_turn.tool_calls,
        session_id: output.session_id,
        snapshot_path: output.snapshot_path,
        events_path: output.events_path,
    }
}
