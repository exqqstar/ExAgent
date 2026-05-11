use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::OpenAiCompatibleLlm;
use crate::orchestration::{ChildSessionSummary, CollectedChildSession};
use crate::policy::PolicyManager;
use crate::runtime::{
    RuntimeController, ThreadStartRequest, TurnContextRequest, TurnStartRequest, UserInput,
};
use crate::session::AgentRole;
use crate::types::{SessionId, ToolCall, TurnId};
use crate::workspace::{canonicalize_from_current, canonicalize_from_root};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub session_id: SessionId,
    pub snapshot_path: String,
    pub events_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectResponse {
    pub children: Vec<ChildSessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectResponse {
    pub session: CollectedChildSession,
}

#[derive(Debug, Deserialize)]
struct RunRequest {
    prompt: String,
    workspace_root: Option<String>,
    cwd: Option<String>,
    session_id: Option<SessionId>,
}

#[derive(Debug, Deserialize)]
struct ForkRequest {
    parent_session_id: SessionId,
    agent_role: AgentRole,
    prompt: String,
    workspace_root: Option<String>,
    spawned_by_turn_id: Option<TurnId>,
}

#[derive(Debug, Deserialize)]
struct InspectRequest {
    parent_session_id: SessionId,
    workspace_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CollectRequest {
    session_id: SessionId,
    workspace_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadStartApiRequest {
    workspace_root: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    agent_role: Option<AgentRole>,
    #[serde(default)]
    instructions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TurnStartApiRequest {
    input: Vec<UserInput>,
    workspace_root: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    agent_role: Option<AgentRole>,
    #[serde(default)]
    instructions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ThreadStartApiResponse {
    thread: ThreadApiResponse,
}

#[derive(Debug, Serialize)]
struct ThreadApiResponse {
    session_id: SessionId,
    status: String,
    context: TurnContextApiResponse,
}

#[derive(Debug, Serialize)]
struct TurnContextApiResponse {
    model: String,
    workspace_root: String,
    cwd: String,
    policy_mode: &'static str,
    agent_role: AgentRole,
    instructions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TurnStartApiResponse {
    turn: TurnApiResponse,
}

#[derive(Debug, Serialize)]
struct TurnApiResponse {
    turn_id: TurnId,
    status: String,
}

#[derive(Debug, Deserialize)]
struct InitializeRequest {
    client_info: ClientInfo,
    capabilities: ClientCapabilities,
}

#[derive(Debug, Deserialize)]
struct ClientInfo {
    name: String,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientCapabilities {
    #[serde(default)]
    sse: bool,
    #[serde(default)]
    interrupt: bool,
}

#[derive(Debug, Serialize)]
struct InitializeResponse {
    protocol_version: &'static str,
    server_capabilities: ServerCapabilities,
    supported_event_types: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct ServerCapabilities {
    sse: bool,
    interrupt: bool,
    compact: bool,
    thread_lifecycle: bool,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Clone)]
struct ApiState {
    runner: Arc<dyn AgentRunner>,
    runtime: RuntimeController,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(
        &self,
        prompt: &str,
        workspace_root: Option<&str>,
        cwd: Option<&str>,
        session_id: Option<&SessionId>,
    ) -> Result<AgentRunResponse>;

    async fn fork(
        &self,
        parent_session_id: &SessionId,
        agent_role: AgentRole,
        prompt: &str,
        workspace_root: Option<&str>,
        spawned_by_turn_id: Option<&TurnId>,
    ) -> Result<AgentRunResponse>;

    async fn inspect(
        &self,
        parent_session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> Result<InspectResponse>;

    async fn collect(
        &self,
        session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> Result<CollectResponse>;
}

pub struct DefaultAgentRunner {
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    session_locks: SessionLockRegistry,
}

impl Default for DefaultAgentRunner {
    fn default() -> Self {
        Self {
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
            session_locks: SessionLockRegistry::default(),
        }
    }
}

#[derive(Clone, Default)]
struct SessionLockRegistry {
    slots: Arc<StdMutex<HashMap<String, SessionLockSlot>>>,
}

struct SessionLockSlot {
    lock: Arc<AsyncMutex<()>>,
    holders: usize,
}

struct SessionLockGuard {
    key: String,
    slots: Arc<StdMutex<HashMap<String, SessionLockSlot>>>,
    _guard: OwnedMutexGuard<()>,
}

impl SessionLockRegistry {
    async fn lock(&self, session_id: &SessionId) -> SessionLockGuard {
        let key = session_id.as_str().to_string();
        let lock = {
            let mut slots = self.slots.lock().expect("session lock registry poisoned");
            let slot = slots.entry(key.clone()).or_insert_with(|| SessionLockSlot {
                lock: Arc::new(AsyncMutex::new(())),
                holders: 0,
            });
            slot.holders += 1;
            slot.lock.clone()
        };
        let guard = lock.lock_owned().await;

        SessionLockGuard {
            key,
            slots: self.slots.clone(),
            _guard: guard,
        }
    }

    #[cfg(test)]
    fn slot_count(&self) -> usize {
        self.slots
            .lock()
            .expect("session lock registry poisoned")
            .len()
    }
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let mut slots = self.slots.lock().expect("session lock registry poisoned");
        let Some(slot) = slots.get_mut(&self.key) else {
            return;
        };
        slot.holders = slot.holders.saturating_sub(1);
        if slot.holders == 0 {
            slots.remove(&self.key);
        }
    }
}

#[async_trait]
impl AgentRunner for DefaultAgentRunner {
    async fn run(
        &self,
        prompt: &str,
        workspace_root: Option<&str>,
        cwd: Option<&str>,
        session_id: Option<&SessionId>,
    ) -> Result<AgentRunResponse> {
        let _session_guard = match session_id {
            Some(session_id) => Some(self.session_locks.lock(session_id).await),
            None => None,
        };

        let config = build_config(workspace_root, cwd)?;
        let llm = OpenAiCompatibleLlm::from_env()?;
        let agent = Agent::with_runtime(
            config,
            Box::new(llm),
            crate::default_tool_registry(),
            self.exec_sessions.clone(),
            self.policy.clone(),
        );
        let output_result = match session_id {
            Some(session_id) => agent.resume(session_id, prompt).await,
            None => agent.run_with_meta(prompt).await,
        };
        let output = output_result?;

        Ok(agent_run_response(output))
    }

    async fn fork(
        &self,
        parent_session_id: &SessionId,
        agent_role: AgentRole,
        prompt: &str,
        workspace_root: Option<&str>,
        spawned_by_turn_id: Option<&TurnId>,
    ) -> Result<AgentRunResponse> {
        let _parent_session_guard = self.session_locks.lock(parent_session_id).await;

        let config = build_config(workspace_root, None)?;
        let llm = OpenAiCompatibleLlm::from_env()?;
        let agent = Agent::with_runtime(
            config,
            Box::new(llm),
            crate::default_tool_registry(),
            self.exec_sessions.clone(),
            self.policy.clone(),
        );
        let output = agent
            .fork_session(parent_session_id, agent_role, prompt, spawned_by_turn_id)
            .await?;

        Ok(agent_run_response(output))
    }

    async fn inspect(
        &self,
        parent_session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> Result<InspectResponse> {
        let config = build_config(workspace_root, None)?;
        Ok(InspectResponse {
            children: crate::orchestration::inspect_children(
                &config.workspace_root,
                parent_session_id,
            )?,
        })
    }

    async fn collect(
        &self,
        session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> Result<CollectResponse> {
        let config = build_config(workspace_root, None)?;
        Ok(CollectResponse {
            session: crate::orchestration::collect_session(&config.workspace_root, session_id)?,
        })
    }
}

pub fn build_router(runner: Arc<dyn AgentRunner>) -> Router {
    Router::new()
        .route("/initialize", post(initialize))
        .route("/health", get(health))
        .route("/threads", post(start_thread))
        .route("/threads/{session_id}/turns", post(start_turn))
        .route("/run", post(run_agent))
        .route("/fork", post(fork_agent))
        .route("/inspect", post(inspect_children))
        .route("/collect", post(collect_session))
        .with_state(ApiState {
            runner,
            runtime: RuntimeController::new(AgentConfig::default()),
        })
}

pub async fn serve(bind_addr: Option<&str>) -> Result<()> {
    let bind_addr = bind_addr
        .map(str::to_string)
        .or_else(|| std::env::var("EXAGENT_API_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:3000".to_string());

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind API listener on {bind_addr}"))?;

    tracing::info!("exagent API listening on {}", bind_addr);
    axum::serve(
        listener,
        build_router(Arc::new(DefaultAgentRunner::default())),
    )
    .await
    .context("API server stopped unexpectedly")?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn initialize(Json(request): Json<InitializeRequest>) -> Json<InitializeResponse> {
    let _client_name = request.client_info.name;
    let _client_version = request.client_info.version;
    let _client_requested_sse = request.capabilities.sse;
    let _client_requested_interrupt = request.capabilities.interrupt;

    Json(InitializeResponse {
        protocol_version: "runtime_control/v1",
        server_capabilities: ServerCapabilities {
            sse: false,
            interrupt: false,
            compact: false,
            thread_lifecycle: true,
        },
        supported_event_types: vec![
            "assistant_turn",
            "tool_result",
            "session_spawned",
            "exec_output",
            "approval_requested",
            "approval_decision",
            "compaction_written",
            "structured_result_recorded",
            "runtime_error",
        ],
    })
}

async fn start_thread(
    State(state): State<ApiState>,
    Json(request): Json<ThreadStartApiRequest>,
) -> impl IntoResponse {
    let request = ThreadStartRequest {
        context: TurnContextRequest {
            workspace_root: request.workspace_root,
            cwd: request.cwd,
            model: request.model,
            policy_mode: None,
            agent_role: request.agent_role,
            instructions: request.instructions,
        },
    };

    match state.runtime.start_thread(request).await {
        Ok(thread) => (
            StatusCode::OK,
            Json(ThreadStartApiResponse {
                thread: ThreadApiResponse {
                    session_id: thread.session_id,
                    status: thread.status,
                    context: TurnContextApiResponse {
                        model: thread.context.model,
                        workspace_root: thread.context.workspace_root.display().to_string(),
                        cwd: thread.context.cwd.display().to_string(),
                        policy_mode: policy_mode_name(thread.context.policy_mode),
                        agent_role: thread.context.agent_role,
                        instructions: thread.context.instructions,
                    },
                },
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn start_turn(
    State(state): State<ApiState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TurnStartApiRequest>,
) -> impl IntoResponse {
    let request = TurnStartRequest {
        session_id: SessionId::new(session_id),
        input: request.input,
        context: TurnContextRequest {
            workspace_root: request.workspace_root,
            cwd: request.cwd,
            model: request.model,
            policy_mode: None,
            agent_role: request.agent_role,
            instructions: request.instructions,
        },
    };

    match state.runtime.start_turn(request).await {
        Ok(turn) => (
            StatusCode::OK,
            Json(TurnStartApiResponse {
                turn: TurnApiResponse {
                    turn_id: turn.turn_id,
                    status: turn.status,
                },
            }),
        )
            .into_response(),
        Err(err) => (
            runtime_error_status(&err),
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn run_agent(
    State(state): State<ApiState>,
    Json(request): Json<RunRequest>,
) -> impl IntoResponse {
    match state
        .runner
        .run(
            &request.prompt,
            request.workspace_root.as_deref(),
            request.cwd.as_deref(),
            request.session_id.as_ref(),
        )
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn fork_agent(
    State(state): State<ApiState>,
    Json(request): Json<ForkRequest>,
) -> impl IntoResponse {
    match state
        .runner
        .fork(
            &request.parent_session_id,
            request.agent_role,
            &request.prompt,
            request.workspace_root.as_deref(),
            request.spawned_by_turn_id.as_ref(),
        )
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn inspect_children(
    State(state): State<ApiState>,
    Json(request): Json<InspectRequest>,
) -> impl IntoResponse {
    match state
        .runner
        .inspect(
            &request.parent_session_id,
            request.workspace_root.as_deref(),
        )
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn collect_session(
    State(state): State<ApiState>,
    Json(request): Json<CollectRequest>,
) -> impl IntoResponse {
    match state
        .runner
        .collect(&request.session_id, request.workspace_root.as_deref())
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

fn build_config(workspace_root: Option<&str>, cwd: Option<&str>) -> Result<AgentConfig> {
    let mut config = AgentConfig::default();

    if let Some(raw_root) = workspace_root {
        let root = canonicalize_from_current(raw_root, "Path")?;
        config.workspace_root = root.clone();
        config.cwd = root;
    }

    if let Some(raw_cwd) = cwd {
        config.cwd = canonicalize_from_root(&config.workspace_root, raw_cwd)?;
    }

    Ok(config)
}

fn agent_run_response(output: crate::agent::AgentRunOutput) -> AgentRunResponse {
    AgentRunResponse {
        text: output.final_turn.text,
        tool_calls: output.final_turn.tool_calls,
        session_id: output.session_id,
        snapshot_path: output.snapshot_path.display().to_string(),
        events_path: output.events_path.display().to_string(),
    }
}

fn policy_mode_name(policy_mode: crate::policy::PolicyMode) -> &'static str {
    match policy_mode {
        crate::policy::PolicyMode::Off => "off",
        crate::policy::PolicyMode::Advisory => "advisory",
        crate::policy::PolicyMode::Enforced => "enforced",
    }
}

fn runtime_error_status(err: &anyhow::Error) -> StatusCode {
    if err.to_string().contains("thread not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn session_lock_registry_serializes_same_session_and_cleans_up() {
        let registry = SessionLockRegistry::default();
        let session_id = SessionId::new("session_1");
        let first_guard = registry.lock(&session_id).await;
        let marker = Arc::new(Mutex::new(Vec::new()));

        let competing_registry = registry.clone();
        let competing_marker = marker.clone();
        let competing_session_id = session_id.clone();
        let task = tokio::spawn(async move {
            let _second_guard = competing_registry.lock(&competing_session_id).await;
            competing_marker.lock().await.push("second");
        });

        assert!(
            timeout(Duration::from_millis(50), async {
                loop {
                    if !marker.lock().await.is_empty() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err(),
            "second lock acquired before first lock was released"
        );
        assert_eq!(registry.slot_count(), 1);

        drop(first_guard);
        task.await.unwrap();

        assert_eq!(*marker.lock().await, vec!["second"]);
        assert_eq!(registry.slot_count(), 0);
    }
}
