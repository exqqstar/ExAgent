use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

pub use crate::app_server::protocol::{
    AgentRunResponse, BoundaryOp, BoundaryOpResponse, CollectParams as CollectRequest,
    CollectResponse, EventsReplayParams as EventsReplayRequest, EventsReplayResponse,
    ForkParams as ForkRequest, InitializeParams as InitializeRequest, InitializeResponse,
    InspectParams as InspectRequest, InspectResponse, RunParams as RunRequest,
    ThreadReadParams as ThreadReadRequest, ThreadReadResponse,
    ThreadResumeParams as ThreadResumeRequest, ThreadResumeResponse,
    ThreadSpawnChildParams as ThreadSpawnChildRequest, ThreadSpawnChildResponse,
    ThreadStartParams as ThreadStartRequest, ThreadStartResponse,
    TurnInterruptParams as TurnInterruptRequest, TurnInterruptResponse,
    TurnStartParams as TurnStartRequest, TurnStartResponse,
};
use crate::app_server::AppServerError;
use crate::app_server::{AppServerBoundary, AppServerService};

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
}

pub fn build_router(boundary: Arc<dyn AppServerBoundary>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/initialize", post(initialize))
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
        .with_state(ApiState { boundary })
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
    State(state): State<ApiState>,
    Json(request): Json<InitializeRequest>,
) -> impl IntoResponse {
    json_result::<BoundaryOpResponse>(
        state
            .boundary
            .submit_boundary_op(BoundaryOp::Initialize(request))
            .await,
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
        None => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
