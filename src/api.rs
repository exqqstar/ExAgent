use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

pub use crate::app_server::protocol::{
    AgentRunResponse, BoundaryOp, BoundaryOpResponse, CollectParams as CollectRequest,
    CollectResponse, EventsReplayParams as EventsReplayRequest, EventsReplayResponse,
    ForkParams as ForkRequest, InspectParams as InspectRequest, InspectResponse,
    RunParams as RunRequest, ThreadReadParams as ThreadReadRequest, ThreadReadResponse,
    ThreadResumeParams as ThreadResumeRequest, ThreadResumeResponse,
    ThreadSpawnChildParams as ThreadSpawnChildRequest, ThreadSpawnChildResponse,
    ThreadStartParams as ThreadStartRequest, ThreadStartResponse,
    TurnInterruptParams as TurnInterruptRequest, TurnInterruptResponse,
    TurnStartParams as TurnStartRequest, TurnStartResponse,
};
use crate::app_server::AppServerError;
use crate::app_server::{AppServerBoundary, AppServerService};
use crate::runtime::{
    RuntimeController, ThreadStartRequest as RuntimeThreadStartRequest, TurnContextRequest,
    TurnStartRequest as RuntimeTurnStartRequest, UserInput,
};
use crate::session::AgentRole;
use crate::types::{SessionId, TurnId};

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
    boundary: Arc<dyn AppServerBoundary>,
    runtime: RuntimeController,
}

pub fn build_router(boundary: Arc<dyn AppServerBoundary>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/initialize", post(initialize))
        .route("/threads", post(start_thread))
        .route("/threads/{session_id}/turns", post(start_turn))
        .route("/run", post(run_agent))
        .route("/fork", post(fork_agent))
        .route("/inspect", post(inspect_children))
        .route("/collect", post(collect_session))
        .route("/thread/start", post(thread_start))
        .route("/thread/read", post(thread_read))
        .route("/thread/resume", post(thread_resume))
        .route("/turn/start", post(turn_start))
        .route("/turn/interrupt", post(turn_interrupt))
        .route("/thread/op", post(thread_op))
        .route("/thread/spawn_child", post(thread_spawn_child))
        .route("/thread_spawn_child", post(thread_spawn_child))
        .route("/events/replay", post(events_replay))
        .route("/events_replay", post(events_replay))
        .with_state(ApiState {
            boundary,
            runtime: RuntimeController::new(crate::config::AgentConfig::default()),
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
    axum::serve(listener, build_router(Arc::new(AppServerService::new())))
        .await
        .context("API server stopped unexpectedly")?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn initialize(
    State(_state): State<ApiState>,
    Json(request): Json<InitializeRequest>,
) -> impl IntoResponse {
    let _client_name = request.client_info.name;
    let _client_version = request.client_info.version;
    let _client_supports_sse = request.capabilities.sse;
    let _client_supports_interrupt = request.capabilities.interrupt;

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
    json_result(
        state
            .runtime
            .start_thread(RuntimeThreadStartRequest {
                context: turn_context_request(
                    request.workspace_root,
                    request.cwd,
                    request.model,
                    request.agent_role,
                    request.instructions,
                ),
            })
            .await
            .map(thread_start_api_response),
    )
}

async fn start_turn(
    State(state): State<ApiState>,
    AxumPath(session_id): AxumPath<SessionId>,
    Json(request): Json<TurnStartApiRequest>,
) -> impl IntoResponse {
    json_result(
        state
            .runtime
            .start_turn(RuntimeTurnStartRequest {
                session_id,
                input: request.input,
                context: turn_context_request(
                    request.workspace_root,
                    request.cwd,
                    request.model,
                    request.agent_role,
                    request.instructions,
                ),
            })
            .await
            .map(turn_start_api_response),
    )
}

async fn run_agent(
    State(state): State<ApiState>,
    Json(request): Json<RunRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.run(request).await)
}

async fn fork_agent(
    State(state): State<ApiState>,
    Json(request): Json<ForkRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.fork(request).await)
}

async fn inspect_children(
    State(state): State<ApiState>,
    Json(request): Json<InspectRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.inspect(request).await)
}

async fn collect_session(
    State(state): State<ApiState>,
    Json(request): Json<CollectRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.collect(request).await)
}

async fn thread_start(
    State(state): State<ApiState>,
    Json(request): Json<ThreadStartRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.thread_start(request).await)
}

async fn thread_read(
    State(state): State<ApiState>,
    Json(request): Json<ThreadReadRequest>,
) -> impl IntoResponse {
    json_result::<ThreadReadResponse>(state.boundary.thread_read(request).await)
}

async fn thread_resume(
    State(state): State<ApiState>,
    Json(request): Json<ThreadResumeRequest>,
) -> impl IntoResponse {
    json_result::<ThreadResumeResponse>(state.boundary.thread_resume(request).await)
}

async fn turn_start(
    State(state): State<ApiState>,
    Json(request): Json<TurnStartRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.turn_start(request).await)
}

async fn turn_interrupt(
    State(state): State<ApiState>,
    Json(request): Json<TurnInterruptRequest>,
) -> impl IntoResponse {
    json_result::<TurnInterruptResponse>(state.boundary.turn_interrupt(request).await)
}

async fn thread_op(
    State(state): State<ApiState>,
    Json(request): Json<BoundaryOp>,
) -> impl IntoResponse {
    json_result::<BoundaryOpResponse>(state.boundary.submit_boundary_op(request).await)
}

async fn thread_spawn_child(
    State(state): State<ApiState>,
    Json(request): Json<ThreadSpawnChildRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.thread_spawn_child(request).await)
}

async fn events_replay(
    State(state): State<ApiState>,
    Json(request): Json<EventsReplayRequest>,
) -> impl IntoResponse {
    json_result(state.boundary.events_replay(request).await)
}

fn json_result<T: Serialize>(result: Result<T>) -> axum::response::Response {
    match result {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(err) => (
            status_for_error(&err),
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

fn status_for_error(err: &anyhow::Error) -> StatusCode {
    match err.downcast_ref::<AppServerError>() {
        Some(AppServerError::InvalidRequest(_)) => StatusCode::BAD_REQUEST,
        Some(AppServerError::ThreadNotFound(_)) => StatusCode::NOT_FOUND,
        Some(AppServerError::ThreadBusy(_)) => StatusCode::CONFLICT,
        Some(AppServerError::TurnRejected { .. }) => StatusCode::CONFLICT,
        Some(AppServerError::TurnInterrupted { .. }) => StatusCode::CONFLICT,
        None => runtime_error_status(err),
    }
}

fn runtime_error_status(err: &anyhow::Error) -> StatusCode {
    if err.to_string().contains("thread not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    }
}

fn turn_context_request(
    workspace_root: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    agent_role: Option<AgentRole>,
    instructions: Vec<String>,
) -> TurnContextRequest {
    TurnContextRequest {
        workspace_root,
        cwd,
        model,
        policy_mode: None,
        agent_role,
        instructions,
    }
}

fn thread_start_api_response(result: crate::runtime::ThreadStartResult) -> ThreadStartApiResponse {
    ThreadStartApiResponse {
        thread: ThreadApiResponse {
            session_id: result.session_id,
            status: result.status,
            context: turn_context_api_response(result.context),
        },
    }
}

fn turn_start_api_response(result: crate::runtime::TurnStartResult) -> TurnStartApiResponse {
    TurnStartApiResponse {
        turn: TurnApiResponse {
            turn_id: result.turn_id,
            status: result.status,
        },
    }
}

fn turn_context_api_response(context: crate::runtime::TurnContext) -> TurnContextApiResponse {
    TurnContextApiResponse {
        model: context.model,
        workspace_root: context.workspace_root.display().to_string(),
        cwd: context.cwd.display().to_string(),
        policy_mode: policy_mode_name(context.policy_mode),
        agent_role: context.agent_role,
        instructions: context.instructions,
    }
}

fn policy_mode_name(policy_mode: crate::policy::PolicyMode) -> &'static str {
    match policy_mode {
        crate::policy::PolicyMode::Off => "off",
        crate::policy::PolicyMode::Advisory => "advisory",
        crate::policy::PolicyMode::Enforced => "enforced",
    }
}
