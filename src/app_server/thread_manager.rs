use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::agent::Agent;
use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    AgentRunResponse, BoundaryCapability, BoundaryOp, BoundaryOpResponse, EventsReplayParams,
    EventsReplayResponse, EventsSubscribeParams, IgnoredOverrideField, InitializeParams,
    InitializeResponse, ReplaySnapshotView, RunParams, RuntimeEventKindFilter, ThreadItem,
    ThreadReadParams, ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse,
    ThreadStartParams, ThreadStartResponse, ThreadStatus, ThreadView, TurnContextOverrides,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse, TurnState,
    TurnStatus, TurnView, BOUNDARY_PROTOCOL_VERSION,
};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::exec_session::ExecSessionManager;
use crate::llm::{LlmClient, LlmRequestOptions, OpenAiCompatibleLlm};
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntime, ThreadRuntimeError, ThreadRuntimeOptions,
    ThreadTurnContext,
};
use crate::runtime::thread_session::RuntimeOverlay;
use crate::session::ThreadSnapshot;
use crate::state::rollout::{
    events_from_rollout_items, rollout_paths, snapshot_from_rollout_items,
    thread_meta_from_snapshot, RolloutItem, RolloutStore,
};
use crate::types::{AssistantTurn, LlmCompletion, ThreadId, TurnId};

type RegistryFactory = Arc<dyn Fn() -> ToolRegistry + Send + Sync>;

trait LlmFactory: Send + Sync {
    fn build(&self, config: &AgentConfig) -> Result<Box<dyn LlmClient>>;
}

struct EnvLlmFactory;

impl LlmFactory for EnvLlmFactory {
    fn build(&self, config: &AgentConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(OpenAiCompatibleLlm::from_env_with_model(
            config.model.clone(),
        )?))
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
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(messages, tools, options).await
    }
}

pub struct StartThreadOptions {
    pub config: AgentConfig,
    pub initial_history: InitialHistory,
}

pub enum InitialHistory {
    New,
    Resume { thread_id: ThreadId },
}

pub struct NewThread {
    pub thread_id: ThreadId,
    #[allow(dead_code)]
    pub runtime: Arc<ThreadRuntime>,
}

struct TurnStartStarted {
    thread_id: ThreadId,
    turn_id: TurnId,
}

struct LoadedRuntime {
    runtime: Arc<ThreadRuntime>,
    workspace_root: PathBuf,
}

struct StoredThreadState {
    snapshot: ThreadSnapshot,
    events: Vec<RuntimeEvent>,
}

pub struct ThreadManager {
    base_config: AgentConfig,
    llm_factory: Arc<dyn LlmFactory>,
    registry_factory: RegistryFactory,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    loaded_threads: Arc<Mutex<HashMap<String, Arc<ThreadRuntime>>>>,
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
            loaded_threads: Arc::new(Mutex::new(HashMap::new())),
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
            loaded_threads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        let workspace_root = params.workspace_root.clone();
        let thinking_mode = params.thinking_mode;
        let thread_id = match params.thread_id {
            Some(thread_id) => {
                self.thread_resume(ThreadResumeParams {
                    thread_id,
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                })?
                .thread
                .id
            }
            None => {
                self.thread_start(ThreadStartParams {
                    workspace_root: workspace_root.clone(),
                    cwd: params.cwd,
                })?
                .thread
                .id
            }
        };

        self.turn_start_and_wait(TurnStartParams {
            thread_id,
            prompt: params.prompt,
            workspace_root,
            turn_context: thinking_mode.map(|thinking_mode| TurnContextOverrides {
                cwd: None,
                thinking_mode: Some(thinking_mode),
            }),
        })
        .await
    }

    pub fn initialize(&self, _params: InitializeParams) -> InitializeResponse {
        InitializeResponse {
            protocol_version: BOUNDARY_PROTOCOL_VERSION.to_string(),
            supported_ops: vec![
                BoundaryCapability::Initialize,
                BoundaryCapability::ThreadStart,
                BoundaryCapability::ThreadResume,
                BoundaryCapability::ThreadRead,
                BoundaryCapability::TurnStart,
                BoundaryCapability::TurnInterrupt,
                BoundaryCapability::EventsReplay,
            ],
            supported_streams: vec![BoundaryCapability::EventsSubscribe],
        }
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        let config = OverridePolicy::merge_thread_start(
            &self.base_config,
            RuntimeOverrides {
                workspace_root: params.workspace_root,
                cwd: params.cwd,
            },
        )?;
        let new_thread = self.start_thread_with_options(StartThreadOptions {
            config,
            initial_history: InitialHistory::New,
        })?;

        Ok(ThreadStartResponse {
            thread: ThreadView {
                id: new_thread.thread_id,
                status: ThreadStatus::Idle,
                active_turn: None,
                turns: vec![],
            },
        })
    }

    pub fn thread_read(&self, params: ThreadReadParams) -> Result<ThreadReadResponse> {
        let requested_workspace_root = params.workspace_root.clone();
        let config = OverridePolicy::merge_thread_read(&self.base_config, params.workspace_root)?;
        self.thread_read_resolved(
            params.thread_id,
            requested_workspace_root.is_some(),
            &config.workspace_root,
        )
    }

    fn thread_read_resolved(
        &self,
        thread_id: ThreadId,
        requested_workspace_root: bool,
        workspace_root: &Path,
    ) -> Result<ThreadReadResponse> {
        if let Some(loaded) =
            self.resolve_loaded_runtime(&thread_id, requested_workspace_root, workspace_root)?
        {
            let runtime = loaded.runtime;
            let live_view = runtime.live_view();
            return Ok(self.thread_read_from_state_view(
                thread_id,
                live_view.overlay,
                live_view.events,
            ));
        }

        let Some(stored) = read_thread_state_from_storage(workspace_root, &thread_id)? else {
            return Err(AppServerError::ThreadNotFound(thread_id).into());
        };
        Ok(self.thread_read_from_state_view(thread_id, RuntimeOverlay::default(), stored.events))
    }

    fn thread_read_from_state_view(
        &self,
        thread_id: ThreadId,
        overlay: RuntimeOverlay,
        events: Vec<RuntimeEvent>,
    ) -> ThreadReadResponse {
        let active_turn = self.active_turn_state(&thread_id);
        let latest_turn = latest_turn_state(&events);
        let status = if active_turn.is_some() {
            ThreadStatus::Running
        } else if overlay.has_pending_approval() {
            ThreadStatus::WaitingApproval
        } else if latest_turn
            .as_ref()
            .is_some_and(|turn| turn.status == TurnStatus::Failed)
        {
            ThreadStatus::Failed
        } else {
            ThreadStatus::Idle
        };

        ThreadReadResponse {
            thread: build_thread_view(thread_id, status, active_turn, events),
        }
    }

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<ThreadResumeResponse> {
        let ignored_overrides = ignored_resume_overrides(&params);
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config = OverridePolicy::merge_thread_resume(&self.base_config, params.workspace_root)?;
        let workspace_root = config.workspace_root.clone();
        if let Some(loaded) = self.resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &workspace_root,
        )? {
            let thread =
                self.thread_read_resolved(params.thread_id, false, &loaded.workspace_root)?;
            return Ok(ThreadResumeResponse {
                thread: thread.thread,
                ignored_overrides,
            });
        }

        let new_thread = self.start_thread_with_options(StartThreadOptions {
            config,
            initial_history: InitialHistory::Resume {
                thread_id: params.thread_id,
            },
        })?;
        let thread = self.thread_read_resolved(new_thread.thread_id, false, &workspace_root)?;

        Ok(ThreadResumeResponse {
            thread: thread.thread,
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

    async fn turn_start_and_wait(&self, params: TurnStartParams) -> Result<AgentRunResponse> {
        let (thread_id, _workspace_root, final_turn) =
            self.run_turn_through_runtime(params).await?;
        Ok(agent_run_response(thread_id, final_turn))
    }

    pub async fn turn_interrupt(
        &self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResponse> {
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config =
            OverridePolicy::merge_turn_start(&self.base_config, params.workspace_root.clone())?;
        if let Some(loaded) = self.resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )? {
            let runtime = loaded.runtime;
            if runtime.active_turn_id().is_some() {
                let turn_id = runtime
                    .interrupt_active_turn(params.turn_id.as_ref())
                    .await
                    .map_err(map_thread_runtime_error)?;
                return Ok(TurnInterruptResponse {
                    thread_id: params.thread_id,
                    interrupted_turn: Some(TurnState {
                        turn_id,
                        status: TurnStatus::Interrupted,
                    }),
                });
            }
            let turn_id = runtime
                .interrupt_waiting_approval_turn(params.turn_id.clone())
                .await
                .map_err(map_thread_runtime_error)?;
            return Ok(TurnInterruptResponse {
                thread_id: params.thread_id,
                interrupted_turn: Some(TurnState {
                    turn_id,
                    status: TurnStatus::Interrupted,
                }),
            });
        }

        if thread_exists_in_storage(&config.workspace_root, &params.thread_id) {
            return Err(AppServerError::TurnRejected {
                thread_id: params.thread_id,
                reason: "thread has no active turn".to_string(),
            }
            .into());
        }

        Err(AppServerError::ThreadNotFound(params.thread_id).into())
    }

    async fn turn_start_direct(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        let TurnStartStarted {
            thread_id, turn_id, ..
        } = self.start_turn_in_background(params).await?;

        Ok(TurnStartResponse {
            thread_id,
            turn: TurnView {
                id: turn_id,
                status: TurnStatus::InProgress,
                items: vec![],
            },
        })
    }

    async fn run_turn_through_runtime(
        &self,
        params: TurnStartParams,
    ) -> Result<(ThreadId, PathBuf, AssistantTurn)> {
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config = OverridePolicy::merge_turn_start(&self.base_config, params.workspace_root)?;
        let thread_id = params.thread_id;
        let runtime = self.ensure_runtime_loaded(&thread_id, config, requested_workspace_root)?;
        let turn_id = runtime.next_turn_id();
        let live_view = runtime.live_view();
        let runtime_workspace_root = live_view.snapshot.workspace_root.clone();
        let turn_context = resolve_turn_context(&live_view.snapshot, params.turn_context)?;
        let prompt = params.prompt;
        let result = runtime
            .submit_user_input_and_wait(turn_id.clone(), prompt, turn_context)
            .await
            .map_err(map_thread_runtime_error)?;
        let ThreadOpResult::UserInput { final_turn, .. } = result else {
            return Err(AppServerError::InvalidRequest(
                "turn_start returned non-user-input runtime result".into(),
            )
            .into());
        };

        Ok((thread_id, runtime_workspace_root, final_turn))
    }

    async fn start_turn_in_background(&self, params: TurnStartParams) -> Result<TurnStartStarted> {
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config = OverridePolicy::merge_turn_start(&self.base_config, params.workspace_root)?;
        let thread_id = params.thread_id;
        let runtime = self.ensure_runtime_loaded(&thread_id, config, requested_workspace_root)?;
        let turn_id = runtime.next_turn_id();
        let live_view = runtime.live_view();
        let turn_context = resolve_turn_context(&live_view.snapshot, params.turn_context)?;
        runtime
            .submit_user_input(turn_id.clone(), params.prompt, turn_context)
            .await
            .map_err(map_thread_runtime_error)?;

        Ok(TurnStartStarted { thread_id, turn_id })
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
            BoundaryOp::EventsReplay(params) => self
                .events_replay(params)
                .map(BoundaryOpResponse::EventsReplayed),
        }
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config =
            OverridePolicy::merge_events_replay(&self.base_config, params.workspace_root.clone())?;
        let workspace_root = self
            .resolve_loaded_runtime(
                &params.thread_id,
                requested_workspace_root,
                &config.workspace_root,
            )?
            .map(|loaded| loaded.workspace_root)
            .unwrap_or_else(|| config.workspace_root.clone());
        let (events, snapshot) = if let Some(loaded) = self.resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )? {
            let live_view = loaded.runtime.live_view();
            (
                filter_replay_events(live_view.events, &params),
                params
                    .include_snapshot
                    .then(|| replay_snapshot_view(live_view.snapshot, &live_view.overlay)),
            )
        } else {
            let Some(stored) = read_thread_state_from_storage(&workspace_root, &params.thread_id)?
            else {
                return Err(AppServerError::ThreadNotFound(params.thread_id).into());
            };
            (
                filter_replay_events(stored.events, &params),
                params
                    .include_snapshot
                    .then(|| replay_snapshot_view(stored.snapshot, &RuntimeOverlay::default())),
            )
        };

        Ok(EventsReplayResponse {
            thread_id: params.thread_id,
            events,
            snapshot,
        })
    }

    pub fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>> {
        let requested_workspace_root = params.workspace_root.clone();
        let requested_workspace_root = requested_workspace_root.is_some();
        let config =
            OverridePolicy::merge_events_replay(&self.base_config, params.workspace_root.clone())?;
        if let Some(loaded) = self.resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )? {
            return Ok(loaded.runtime.subscribe_events());
        }
        if !thread_exists_in_storage(&config.workspace_root, &params.thread_id) {
            return Err(AppServerError::ThreadNotFound(params.thread_id).into());
        }
        let runtime =
            self.ensure_runtime_loaded(&params.thread_id, config, requested_workspace_root)?;
        Ok(runtime.subscribe_events())
    }

    fn runtime_agent_factory(&self) -> AgentFactory {
        let llm_factory = self.llm_factory.clone();
        let registry_factory = self.registry_factory.clone();
        let exec_sessions = self.exec_sessions.clone();
        let policy = self.policy.clone();

        Arc::new(move |config: AgentConfig| {
            let llm = llm_factory.build(&config)?;
            Ok(Agent::with_runtime(
                config,
                llm,
                (registry_factory)(),
                exec_sessions.clone(),
                policy.clone(),
            ))
        })
    }

    fn start_thread_with_options(&self, options: StartThreadOptions) -> Result<NewThread> {
        match options.initial_history {
            InitialHistory::New => {
                let thread_id = crate::transcript::new_thread_id();
                let snapshot = ThreadSnapshot::new_thread(
                    thread_id.clone(),
                    options.config.workspace_root.clone(),
                    options.config.cwd.clone(),
                );
                let rollout_paths = rollout_paths(&snapshot.workspace_root, &thread_id);
                RolloutStore::new(rollout_paths.rollout_path).append_items_blocking(&[
                    RolloutItem::ThreadMeta(thread_meta_from_snapshot(&snapshot)),
                ])?;
                let runtime = self.ensure_runtime_loaded(&thread_id, options.config, false)?;

                Ok(NewThread { thread_id, runtime })
            }
            InitialHistory::Resume { thread_id } => {
                if !thread_exists_in_storage(&options.config.workspace_root, &thread_id) {
                    return Err(AppServerError::ThreadNotFound(thread_id).into());
                }
                let runtime = self.ensure_runtime_loaded(&thread_id, options.config, false)?;

                Ok(NewThread { thread_id, runtime })
            }
        }
    }

    fn ensure_runtime_loaded(
        &self,
        thread_id: &ThreadId,
        config: AgentConfig,
        requested_workspace_root: bool,
    ) -> Result<Arc<ThreadRuntime>> {
        if let Some(loaded) = self.resolve_loaded_runtime(
            thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )? {
            return Ok(loaded.runtime);
        }

        let runtime = ThreadRuntime::spawn(
            ThreadRuntimeOptions::new(thread_id.clone(), config, self.runtime_agent_factory())
                .with_policy(self.policy.clone()),
        )?;
        self.loaded_threads
            .lock()
            .expect("loaded threads mutex poisoned")
            .insert(thread_id.as_str().to_string(), runtime.clone());
        Ok(runtime)
    }

    fn runtime_for(&self, thread_id: &ThreadId) -> Option<Arc<ThreadRuntime>> {
        self.loaded_threads
            .lock()
            .ok()
            .and_then(|loaded_threads| loaded_threads.get(thread_id.as_str()).cloned())
    }

    fn resolve_loaded_runtime(
        &self,
        thread_id: &ThreadId,
        requested_workspace_root: bool,
        workspace_root: &Path,
    ) -> Result<Option<LoadedRuntime>> {
        let Some(runtime) = self.runtime_for(thread_id) else {
            return Ok(None);
        };
        let live_workspace_root = runtime.live_view().snapshot.workspace_root;
        if requested_workspace_root && live_workspace_root != workspace_root {
            return Err(workspace_mismatch_error(
                thread_id,
                workspace_root,
                &live_workspace_root,
            ));
        }
        Ok(Some(LoadedRuntime {
            runtime,
            workspace_root: live_workspace_root,
        }))
    }

    #[cfg(test)]
    fn is_thread_loaded(&self, thread_id: &ThreadId) -> bool {
        self.runtime_for(thread_id).is_some()
    }

    fn active_turn_state(&self, thread_id: &ThreadId) -> Option<TurnState> {
        self.runtime_for(thread_id)
            .and_then(|runtime| runtime.active_turn_id())
            .map(|turn_id| TurnState {
                turn_id,
                status: TurnStatus::InProgress,
            })
    }
}

fn resolve_turn_context(
    snapshot: &ThreadSnapshot,
    overrides: Option<TurnContextOverrides>,
) -> Result<Option<ThreadTurnContext>> {
    let Some(overrides) = overrides else {
        return Ok(None);
    };
    let thinking_mode = overrides.thinking_mode;
    let resolved_snapshot = OverridePolicy::apply_turn_context(snapshot, overrides)?;
    Ok(Some(ThreadTurnContext {
        cwd: Some(resolved_snapshot.cwd),
        thinking_mode,
    }))
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

fn workspace_mismatch_error(
    thread_id: &ThreadId,
    requested_workspace_root: &Path,
    active_workspace_root: &Path,
) -> anyhow::Error {
    AppServerError::InvalidRequest(format!(
        "thread {} belongs to workspace `{}`, but request targeted workspace `{}`",
        thread_id.as_str(),
        active_workspace_root.display(),
        requested_workspace_root.display()
    ))
    .into()
}

fn boundary_response_name(response: &BoundaryOpResponse) -> &'static str {
    match response {
        BoundaryOpResponse::Initialized(_) => "initialized",
        BoundaryOpResponse::ThreadStarted(_) => "thread_started",
        BoundaryOpResponse::ThreadRead(_) => "thread_read",
        BoundaryOpResponse::ThreadResumed(_) => "thread_resumed",
        BoundaryOpResponse::TurnStarted(_) => "turn_started",
        BoundaryOpResponse::TurnInterrupted(_) => "turn_interrupted",
        BoundaryOpResponse::EventsReplayed(_) => "events_replayed",
    }
}

fn latest_turn_state(events: &[crate::events::RuntimeEvent]) -> Option<TurnState> {
    events.iter().rev().find_map(|event| {
        let turn_id = event.turn_id.clone()?;
        let status = match &event.kind {
            RuntimeEventKind::TurnStarted => TurnStatus::InProgress,
            RuntimeEventKind::TurnCompleted => TurnStatus::Completed,
            RuntimeEventKind::TurnInterrupted => TurnStatus::Interrupted,
            RuntimeEventKind::RuntimeError { .. } => TurnStatus::Failed,
            RuntimeEventKind::ApprovalRequested { .. } => TurnStatus::InProgress,
            RuntimeEventKind::AssistantTurn { .. } => TurnStatus::Completed,
            RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ExecOutput { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::CompactionWritten { .. }
            | RuntimeEventKind::TokenCount { .. } => TurnStatus::InProgress,
        };
        Some(TurnState { turn_id, status })
    })
}

fn build_thread_view(
    thread_id: ThreadId,
    status: ThreadStatus,
    active_turn: Option<TurnState>,
    events: Vec<RuntimeEvent>,
) -> ThreadView {
    let mut turns = build_turn_views(events);
    let active_turn_view = active_turn.map(|state| {
        let index = ensure_turn_view(&mut turns, &state.turn_id);
        turns[index].status = state.status;
        turns[index].clone()
    });

    ThreadView {
        id: thread_id,
        status,
        active_turn: active_turn_view,
        turns,
    }
}

fn build_turn_views(events: Vec<RuntimeEvent>) -> Vec<TurnView> {
    let mut turns = Vec::new();
    let mut current_turn_id: Option<TurnId> = None;

    for event in events {
        match &event.kind {
            RuntimeEventKind::TurnStarted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::InProgress;
                current_turn_id = Some(turn_id);
            }
            RuntimeEventKind::TurnCompleted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::Completed;
                if current_turn_id.as_ref() == Some(&turn_id) {
                    current_turn_id = None;
                }
            }
            RuntimeEventKind::TurnInterrupted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::Interrupted;
                if current_turn_id.as_ref() == Some(&turn_id) {
                    current_turn_id = None;
                }
            }
            RuntimeEventKind::RuntimeError { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    turns[index].status = TurnStatus::Failed;
                    if let Some(item) = thread_item_from_event(&event.kind) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::ApprovalRequested { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    if let Some(item) = thread_item_from_event(&event.kind) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::AssistantTurn { .. }
            | RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ExecOutput { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::CompactionWritten { .. }
            | RuntimeEventKind::TokenCount { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    if let Some(item) = thread_item_from_event(&event.kind) {
                        turns[index].items.push(item);
                    }
                }
            }
        }
    }

    turns
}

fn ensure_turn_view(turns: &mut Vec<TurnView>, turn_id: &TurnId) -> usize {
    if let Some(index) = turns.iter().position(|turn| &turn.id == turn_id) {
        return index;
    }

    turns.push(TurnView {
        id: turn_id.clone(),
        status: TurnStatus::InProgress,
        items: vec![],
    });
    turns.len() - 1
}

fn view_turn_id(event: &RuntimeEvent, current_turn_id: Option<&TurnId>) -> Option<TurnId> {
    current_turn_id.cloned().or_else(|| event.turn_id.clone())
}

fn thread_item_from_event(kind: &RuntimeEventKind) -> Option<ThreadItem> {
    match kind {
        RuntimeEventKind::AssistantTurn { turn } => Some(ThreadItem::AssistantMessage {
            text: turn.text.clone(),
        }),
        RuntimeEventKind::ToolResult { result } => Some(ThreadItem::ToolResult {
            name: result.tool_name.clone(),
        }),
        RuntimeEventKind::ExecOutput { chunk, .. } => Some(ThreadItem::ExecOutput {
            text: chunk.clone(),
        }),
        RuntimeEventKind::ApprovalRequested {
            tool_name, reason, ..
        } => Some(ThreadItem::ApprovalRequested {
            tool_name: tool_name.clone(),
            reason: reason.clone(),
        }),
        RuntimeEventKind::ApprovalDecision { status, note, .. } => {
            Some(ThreadItem::ApprovalDecision {
                status: approval_status_name(status).to_string(),
                note: note.clone(),
            })
        }
        RuntimeEventKind::RuntimeError { message } => Some(ThreadItem::RuntimeError {
            message: message.clone(),
        }),
        RuntimeEventKind::CompactionWritten { .. } => Some(ThreadItem::CompactionWritten),
        RuntimeEventKind::TurnStarted
        | RuntimeEventKind::TurnCompleted
        | RuntimeEventKind::TurnInterrupted
        | RuntimeEventKind::TokenCount { .. } => None,
    }
}

fn approval_status_name(status: &crate::session::ApprovalStatus) -> &'static str {
    match status {
        crate::session::ApprovalStatus::Pending => "pending",
        crate::session::ApprovalStatus::Approved => "approved",
        crate::session::ApprovalStatus::Denied => "denied",
    }
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

fn thread_exists_in_storage(workspace_root: &Path, thread_id: &ThreadId) -> bool {
    let rollout_paths = rollout_paths(workspace_root, thread_id);
    std::fs::metadata(&rollout_paths.rollout_path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}

fn read_thread_state_from_storage(
    workspace_root: &Path,
    thread_id: &ThreadId,
) -> Result<Option<StoredThreadState>> {
    let rollout_paths = rollout_paths(workspace_root, thread_id);
    let rollout_items = RolloutStore::read_items_blocking(&rollout_paths.rollout_path)?;
    if rollout_items.is_empty() {
        return Ok(None);
    }

    Ok(Some(StoredThreadState {
        snapshot: snapshot_from_rollout_items(thread_id, &rollout_items)?,
        events: events_from_rollout_items(&rollout_items),
    }))
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
            RuntimeEventKindFilter::TokenCount,
            RuntimeEventKind::TokenCount { .. },
        ) | (
            RuntimeEventKindFilter::RuntimeError,
            RuntimeEventKind::RuntimeError { .. },
        )
    )
}

fn replay_snapshot_view(snapshot: ThreadSnapshot, overlay: &RuntimeOverlay) -> ReplaySnapshotView {
    ReplaySnapshotView {
        thread_id: snapshot.thread_id,
        cwd: snapshot.cwd,
        latest_compaction: snapshot.latest_compaction,
        open_exec_session_count: overlay.open_exec_sessions.len(),
        conversation_message_count: snapshot.conversation.len(),
        pending_approval_count: overlay.pending_approvals.len(),
    }
}

fn agent_run_response(thread_id: ThreadId, final_turn: AssistantTurn) -> AgentRunResponse {
    AgentRunResponse {
        text: final_turn.text,
        tool_calls: final_turn.tool_calls,
        thread_id,
    }
}

fn map_thread_runtime_error(err: anyhow::Error) -> anyhow::Error {
    let Some(runtime_error) = err.downcast_ref::<ThreadRuntimeError>() else {
        return err;
    };

    match runtime_error {
        ThreadRuntimeError::ThreadBusy(thread_id) => {
            AppServerError::ThreadBusy(thread_id.clone()).into()
        }
        ThreadRuntimeError::TurnRejected { thread_id, reason } => AppServerError::TurnRejected {
            thread_id: thread_id.clone(),
            reason: reason.clone(),
        }
        .into(),
        ThreadRuntimeError::TurnInterrupted { thread_id, turn_id } => {
            AppServerError::TurnInterrupted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            }
            .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    use crate::llm::MockLlm;

    #[test]
    fn thread_start_registers_loaded_runtime_and_thread_resume_reuses_it() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
            })
            .expect("thread start");
        assert!(manager.is_thread_loaded(&started.thread.id));
        let started_runtime = manager.runtime_for(&started.thread.id).unwrap();

        let resumed = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
            })
            .expect("thread resume");

        assert_eq!(resumed.thread.id, started.thread.id);
        let resumed_runtime = manager.runtime_for(&started.thread.id).unwrap();
        assert!(Arc::ptr_eq(&started_runtime, &resumed_runtime));
    }

    #[test]
    fn thread_start_writes_rollout_without_legacy_snapshot_or_events() {
        let dir = tempdir().unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(dir.path().display().to_string()),
                cwd: None,
            })
            .expect("thread start");

        let rollout_paths = crate::state::rollout::rollout_paths(dir.path(), &started.thread.id);
        assert!(rollout_paths.rollout_path.exists());
    }

    #[test]
    fn thread_resume_uses_loaded_runtime_workspace_when_request_omits_workspace_root() {
        let base_dir = tempdir().unwrap();
        let thread_dir = tempdir().unwrap();
        let base_root = std::fs::canonicalize(base_dir.path()).unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: base_root.clone(),
                cwd: base_root,
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
            })
            .expect("thread start");
        let resumed = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                cwd: None,
            })
            .expect("thread resume");

        assert_eq!(resumed.thread.id, started.thread.id);
    }

    #[test]
    fn thread_resume_rejects_loaded_runtime_workspace_mismatch() {
        let thread_dir = tempdir().unwrap();
        let other_dir = tempdir().unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let other_root = std::fs::canonicalize(other_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
            })
            .expect("thread start");
        let err = manager
            .thread_resume(ThreadResumeParams {
                thread_id: started.thread.id.clone(),
                workspace_root: Some(other_root.display().to_string()),
                cwd: None,
            })
            .expect_err("workspace mismatch must be rejected");

        assert!(err.to_string().contains("belongs to workspace"));
    }

    #[tokio::test]
    async fn run_turn_uses_loaded_runtime_workspace_when_request_omits_workspace_root() {
        let base_dir = tempdir().unwrap();
        let thread_dir = tempdir().unwrap();
        let base_root = std::fs::canonicalize(base_dir.path()).unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig {
                workspace_root: base_root.clone(),
                cwd: base_root.clone(),
                ..AgentConfig::default()
            },
            Box::new(MockLlm::new(vec![AssistantTurn {
                text: Some("done in loaded workspace".into()),
                tool_calls: vec![],
            }])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
            })
            .expect("thread start");
        let (thread_id, workspace_root, final_turn) = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "continue".into(),
                workspace_root: None,
                turn_context: None,
            })
            .await
            .expect("turn");
        let response = agent_run_response(thread_id, final_turn);

        assert_eq!(workspace_root, thread_root);
        let _ = base_root;
        assert_eq!(response.text.as_deref(), Some("done in loaded workspace"));
    }

    #[tokio::test]
    async fn run_turn_rejects_loaded_runtime_workspace_mismatch() {
        let thread_dir = tempdir().unwrap();
        let other_dir = tempdir().unwrap();
        let thread_root = std::fs::canonicalize(thread_dir.path()).unwrap();
        let other_root = std::fs::canonicalize(other_dir.path()).unwrap();
        let manager = ThreadManager::with_llm(
            AgentConfig::default(),
            Box::new(MockLlm::new(vec![])),
            || ToolRegistry::new(),
        );

        let started = manager
            .thread_start(ThreadStartParams {
                workspace_root: Some(thread_root.display().to_string()),
                cwd: None,
            })
            .expect("thread start");
        let err = manager
            .run_turn_through_runtime(TurnStartParams {
                thread_id: started.thread.id,
                prompt: "continue".into(),
                workspace_root: Some(other_root.display().to_string()),
                turn_context: None,
            })
            .await
            .expect_err("workspace mismatch must be rejected");

        assert!(err.to_string().contains("belongs to workspace"));
    }
}
