use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::convert::Infallible;
use std::path::{Path, PathBuf};

pub use crate::app_server::protocol::{
    AgentRunResponse, AgentTreeParams as AgentTreeRequest, AgentTreeResponse, BoundaryOp,
    BoundaryOpResponse, CheckpointRestoreParams as CheckpointRestoreRequest,
    CheckpointRestoreResponse, EventsReplayParams as EventsReplayRequest, EventsReplayResponse,
    EventsSubscribeParams as EventsSubscribeRequest, InitializeParams as InitializeRequest,
    InitializeResponse, RunParams as RunRequest, ThreadCompactParams as ThreadCompactRequest,
    ThreadCompactResponse, ThreadReadParams as ThreadReadRequest, ThreadReadResponse,
    ThreadResumeParams as ThreadResumeRequest, ThreadResumeResponse,
    ThreadStartParams as ThreadStartRequest, ThreadStartResponse,
    TurnInterruptParams as TurnInterruptRequest, TurnInterruptResponse,
    TurnStartParams as TurnStartRequest, TurnStartResponse,
};
use crate::app_server::AppServerError;
use crate::app_server::{AppServerBoundary, AppServerService};
use crate::config::AgentConfig;
use crate::events::{
    redact_runtime_event_for_public_boundary, redact_runtime_events_for_public_boundary,
    RuntimeEvent,
};
use crate::index_db::IndexDb;
use crate::index_db::ProjectUpsert;
use crate::resolver::EnvModelResolver;
use crate::runtime::thread_runtime::ThreadRuntimeError;
use crate::state::rollout::rollout_paths;
use crate::types::ThreadId;

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

struct HeadlessApiBoundary {
    service: AppServerService,
    db: IndexDb,
}

impl HeadlessApiBoundary {
    async fn index_thread_start(
        &self,
        params: &ThreadStartRequest,
        thread_id: &ThreadId,
    ) -> Result<()> {
        let workspace_root = params
            .workspace_root
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let project = self
            .db
            .upsert_project(ProjectUpsert {
                name: workspace_root
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Headless API")
                    .to_string(),
                path: workspace_root.clone(),
            })
            .await?;
        let rollout_path = rollout_paths(&workspace_root, thread_id).rollout_path;
        let now = unix_millis();
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  archived_at, pinned, status, created_at, updated_at, last_opened_at
)
VALUES (?, ?, ?, ?, ?, 'headless_api', NULL, 0, 'idle', ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  project_id = excluded.project_id,
  rollout_path = excluded.rollout_path,
  archived_at = NULL,
  status = excluded.status,
  updated_at = excluded.updated_at,
  last_opened_at = excluded.last_opened_at
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project.id)
        .bind(rollout_path.display().to_string())
        .bind(format!("{} title", thread_id.as_str()))
        .bind(format!("{} preview", thread_id.as_str()))
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl AppServerBoundary for HeadlessApiBoundary {
    async fn run(&self, params: RunRequest) -> Result<AgentRunResponse> {
        self.service.run(params).await
    }

    async fn thread_start(&self, params: ThreadStartRequest) -> Result<ThreadStartResponse> {
        let response = self.service.thread_start(params.clone())?;
        self.index_thread_start(&params, &response.thread.id)
            .await?;
        Ok(response)
    }

    async fn thread_read(&self, params: ThreadReadRequest) -> Result<ThreadReadResponse> {
        self.service.thread_read(params)
    }

    async fn thread_compact(&self, params: ThreadCompactRequest) -> Result<ThreadCompactResponse> {
        self.service.thread_compact(params).await
    }

    async fn thread_resume(&self, params: ThreadResumeRequest) -> Result<ThreadResumeResponse> {
        self.service.thread_resume(params)
    }

    async fn agent_tree(&self, params: AgentTreeRequest) -> Result<AgentTreeResponse> {
        self.service.agent_tree(params).await
    }

    async fn turn_start(&self, params: TurnStartRequest) -> Result<TurnStartResponse> {
        self.service.turn_start(params).await
    }

    async fn turn_interrupt(&self, params: TurnInterruptRequest) -> Result<TurnInterruptResponse> {
        self.service.turn_interrupt(params).await
    }

    async fn approval_decision(
        &self,
        params: crate::app_server::protocol::ApprovalDecisionParams,
    ) -> Result<crate::app_server::protocol::ApprovalDecisionResponse> {
        self.service.approval_decision(params).await
    }

    async fn submit_user_input(
        &self,
        params: crate::app_server::protocol::SubmitUserInputParams,
    ) -> Result<crate::app_server::protocol::SubmitUserInputResponse> {
        self.service.submit_user_input(params).await
    }

    async fn open_question_resolve(
        &self,
        params: crate::app_server::protocol::OpenQuestionResolveParams,
    ) -> Result<crate::app_server::protocol::OpenQuestionResolveResponse> {
        self.service.open_question_resolve(params).await
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> Result<BoundaryOpResponse> {
        self.service.submit_boundary_op(op).await
    }

    async fn events_replay(&self, params: EventsReplayRequest) -> Result<EventsReplayResponse> {
        self.service.events_replay(params)
    }

    async fn events_subscribe(
        &self,
        params: EventsSubscribeRequest,
    ) -> Result<tokio::sync::broadcast::Receiver<RuntimeEvent>> {
        self.service.events_subscribe(params)
    }
}

pub fn build_router(boundary: Arc<dyn AppServerBoundary>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/initialize", post(initialize))
        .route("/run", post(run_agent))
        .route("/thread/start", post(thread_start))
        .route("/thread/read", post(thread_read))
        .route("/thread/resume", post(thread_resume))
        .route("/agent/tree", post(agent_tree))
        .route("/turn/start", post(turn_start))
        .route("/turn/interrupt", post(turn_interrupt))
        .route("/thread/op", post(thread_op))
        .route("/events/replay", post(events_replay))
        .route("/events/subscribe", post(events_subscribe))
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

    let boundary = build_headless_boundary().await?;

    tracing::info!("exagent API listening on {}", bind_addr);
    axum::serve(listener, build_router(boundary))
        .await
        .context("API server stopped unexpectedly")?;

    Ok(())
}

async fn build_headless_boundary() -> Result<Arc<dyn AppServerBoundary>> {
    let state_dir = std::env::var_os("EXAGENT_API_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".exagent-api")
        });
    build_headless_boundary_with_state_dir(&state_dir).await
}

async fn build_headless_boundary_with_state_dir(
    state_dir: &Path,
) -> Result<Arc<dyn AppServerBoundary>> {
    tokio::fs::create_dir_all(state_dir)
        .await
        .with_context(|| format!("failed to create API state dir {}", state_dir.display()))?;
    let db = IndexDb::open(state_dir.join("exagent.sqlite")).await?;
    let service = AppServerService::with_config_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(EnvModelResolver),
        db.clone(),
    );
    Ok(Arc::new(HeadlessApiBoundary { service, db }))
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
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

async fn agent_tree(
    State(state): State<ApiState>,
    Json(request): Json<AgentTreeRequest>,
) -> impl IntoResponse {
    json_result::<AgentTreeResponse>(state.boundary.agent_tree(request).await)
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
    json_result::<BoundaryOpResponse>(
        state
            .boundary
            .submit_boundary_op(request)
            .await
            .map(redact_boundary_op_response_for_public_boundary),
    )
}

async fn events_replay(
    State(state): State<ApiState>,
    Json(request): Json<EventsReplayRequest>,
) -> impl IntoResponse {
    json_result(
        state
            .boundary
            .events_replay(request)
            .await
            .map(redact_events_replay_response_for_public_boundary),
    )
}

async fn events_subscribe(
    State(state): State<ApiState>,
    Json(request): Json<EventsSubscribeRequest>,
) -> axum::response::Response {
    let mut rx = match state.boundary.events_subscribe(request.clone()).await {
        Ok(rx) => rx,
        Err(err) => return json_result::<()>(Err(err)),
    };
    let replay = match state
        .boundary
        .events_replay(EventsReplayRequest {
            thread_id: request.thread_id.clone(),
            workspace_root: request.workspace_root.clone(),
            after_event_id: request.after_event_id,
            limit: None,
            include_snapshot: false,
            event_kinds: vec![],
        })
        .await
    {
        Ok(replay) => redact_events_replay_response_for_public_boundary(replay),
        Err(err) => return json_result::<()>(Err(err)),
    };

    let stream = async_stream::stream! {
        for event in replay.events {
            match Event::default().json_data(event) {
                Ok(event) => yield Ok::<Event, Infallible>(event),
                Err(_) => continue,
            }
        }

        loop {
            match rx.recv().await {
                Ok(event) => {
                    let event = redact_runtime_event_for_public_boundary(event);
                    match Event::default().json_data(event) {
                        Ok(event) => yield Ok::<Event, Infallible>(event),
                        Err(_) => continue,
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn redact_boundary_op_response_for_public_boundary(
    response: BoundaryOpResponse,
) -> BoundaryOpResponse {
    match response {
        BoundaryOpResponse::EventsReplayed(replay) => BoundaryOpResponse::EventsReplayed(
            redact_events_replay_response_for_public_boundary(replay),
        ),
        other => other,
    }
}

fn redact_events_replay_response_for_public_boundary(
    mut response: EventsReplayResponse,
) -> EventsReplayResponse {
    response.events = redact_runtime_events_for_public_boundary(response.events);
    response
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
    if let Some(err) = err.downcast_ref::<AppServerError>() {
        return match err {
            AppServerError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            AppServerError::ThreadNotFound(_) => StatusCode::NOT_FOUND,
            AppServerError::ThreadBusy(_)
            | AppServerError::TurnRejected { .. }
            | AppServerError::TurnInterrupted { .. } => StatusCode::CONFLICT,
        };
    }

    match err.downcast_ref::<ThreadRuntimeError>() {
        Some(ThreadRuntimeError::ThreadBusy(_))
        | Some(ThreadRuntimeError::TurnRejected { .. })
        | Some(ThreadRuntimeError::TurnInterrupted { .. }) => StatusCode::CONFLICT,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::{
        BoundaryOp, BoundaryOpResponse, ThreadGoalMode, ThreadGoalSetParams, ThreadGoalStatus,
        ThreadStartParams,
    };
    use tempfile::tempdir;

    #[tokio::test]
    async fn headless_boundary_configures_thread_goal_store() {
        let dir = tempdir().expect("tempdir");
        let boundary = build_headless_boundary_with_state_dir(dir.path())
            .await
            .expect("headless boundary");
        let started = boundary
            .thread_start(ThreadStartParams {
                workspace_root: Some(dir.path().display().to_string()),
                cwd: Some(dir.path().display().to_string()),
                permission_profile: None,
            })
            .await
            .expect("thread start");

        let response = boundary
            .submit_boundary_op(BoundaryOp::ThreadGoalSet(ThreadGoalSetParams {
                thread_id: started.thread.id,
                workspace_root: Some(dir.path().display().to_string()),
                objective: Some("ship headless goal".to_string()),
                status: Some(ThreadGoalStatus::Active),
                token_budget: None,
                mode: Some(ThreadGoalMode::Reviewed),
            }))
            .await
            .expect("thread goal set");

        let BoundaryOpResponse::ThreadGoalSet(response) = response else {
            panic!("unexpected response");
        };
        assert_eq!(response.goal.objective, "ship headless goal");
        assert_eq!(response.goal.status, ThreadGoalStatus::Active);
        assert_eq!(response.mode, ThreadGoalMode::Reviewed);
    }
}
