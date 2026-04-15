use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::OpenAiCompatibleLlm;
use crate::policy::PolicyManager;
use crate::types::{SessionId, ToolCall};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub session_id: SessionId,
    pub snapshot_path: String,
    pub events_path: String,
}

#[derive(Debug, Deserialize)]
struct RunRequest {
    prompt: String,
    workspace_root: Option<String>,
    cwd: Option<String>,
    session_id: Option<SessionId>,
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
}

pub struct DefaultAgentRunner {
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
}

impl Default for DefaultAgentRunner {
    fn default() -> Self {
        Self {
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
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
        let config = build_config(workspace_root, cwd)?;
        let llm = OpenAiCompatibleLlm::from_env()?;
        let agent = Agent::with_runtime(
            config,
            Box::new(llm),
            crate::default_tool_registry(),
            self.exec_sessions.clone(),
            self.policy.clone(),
        );
        let output = match session_id {
            Some(session_id) => agent.resume(session_id, prompt).await?,
            None => agent.run_with_meta(prompt).await?,
        };

        Ok(AgentRunResponse {
            text: output.final_turn.text,
            tool_calls: output.final_turn.tool_calls,
            session_id: output.session_id,
            snapshot_path: output.snapshot_path.display().to_string(),
            events_path: output.events_path.display().to_string(),
        })
    }
}

pub fn build_router(runner: Arc<dyn AgentRunner>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/run", post(run_agent))
        .with_state(ApiState { runner })
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
    axum::serve(listener, build_router(Arc::new(DefaultAgentRunner::default())))
        .await
        .context("API server stopped unexpectedly")?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
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

fn build_config(workspace_root: Option<&str>, cwd: Option<&str>) -> Result<AgentConfig> {
    let mut config = AgentConfig::default();

    if let Some(raw_root) = workspace_root {
        let root = canonicalize_from_current(raw_root)?;
        config.workspace_root = root.clone();
        config.cwd = root;
    }

    if let Some(raw_cwd) = cwd {
        config.cwd = canonicalize_from_root(&config.workspace_root, raw_cwd)?;
    }

    Ok(config)
}

fn canonicalize_from_current(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(path)
    };

    std::fs::canonicalize(&path).with_context(|| {
        format!(
            "Path does not exist or is not accessible: {}",
            path.display()
        )
    })
}

fn canonicalize_from_root(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };

    let candidate = std::fs::canonicalize(&candidate).with_context(|| {
        format!(
            "cwd does not exist or is not accessible: {}",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(root) {
        bail!("cwd must stay within workspace_root");
    }

    Ok(candidate)
}
