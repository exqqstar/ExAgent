use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use exagent::api::build_router;
use exagent::app_server::protocol::{
    AgentRunResponse, AgentTreeAgentStatus, AgentTreeNode, AgentTreeParams, AgentTreeResponse,
    ApprovalDecisionParams, ApprovalDecisionResponse, BoundaryCapability, BoundaryOp,
    BoundaryOpResponse, EventsReplayParams, EventsReplayResponse, EventsSubscribeParams,
    IgnoredOverrideField, InitializeResponse, RunParams, ThreadCompactParams,
    ThreadCompactResponse, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, ThreadStatus, ThreadView,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse, TurnStatus,
    TurnView, BOUNDARY_PROTOCOL_VERSION,
};
use exagent::app_server::{AppServerBoundary, AppServerError};
use exagent::cli::{parse_cli_command, CliCommand};
use exagent::config::{PermissionProfile, ThinkingMode};
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::runtime::agent_profile::AgentType;
use exagent::runtime::turn_mode::TurnMode;
use exagent::types::{
    AssistantTurn, EventId, ReasoningBlock, ReasoningSignature, ThreadId, ToolCall, TurnId,
};
use serde_json::{json, Value};
use tower::util::ServiceExt;

struct StubBoundary {
    response: AgentRunResponse,
    thread_start_response: ThreadStartResponse,
    thread_read_response: ThreadReadResponse,
    thread_compact_response: ThreadCompactResponse,
    thread_resume_response: ThreadResumeResponse,
    agent_tree_response: AgentTreeResponse,
    turn_start_response: TurnStartResponse,
    events_replay_response: EventsReplayResponse,
    calls: Mutex<Vec<String>>,
}

impl StubBoundary {
    fn new() -> Self {
        Self {
            response: sample_run_response("done"),
            thread_start_response: sample_thread_start_response(),
            thread_read_response: sample_thread_read_response(),
            thread_compact_response: sample_thread_compact_response(),
            thread_resume_response: sample_thread_resume_response(),
            agent_tree_response: sample_agent_tree_response(),
            turn_start_response: sample_turn_start_response(),
            events_replay_response: sample_events_replay_response(),
            calls: Mutex::new(vec![]),
        }
    }
}

#[async_trait::async_trait]
impl AppServerBoundary for StubBoundary {
    async fn run(&self, params: RunParams) -> anyhow::Result<AgentRunResponse> {
        self.calls.lock().unwrap().push("run".into());
        assert_eq!(params.prompt, "continue phase2");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("."));
        assert_eq!(
            params.thread_id.as_ref().map(ThreadId::as_str),
            Some("thread_123")
        );
        assert_eq!(params.thinking_mode, Some(ThinkingMode::Medium));
        Ok(self.response.clone())
    }

    async fn thread_start(&self, params: ThreadStartParams) -> anyhow::Result<ThreadStartResponse> {
        self.calls.lock().unwrap().push("thread_start".into());
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("nested"));
        Ok(self.thread_start_response.clone())
    }

    async fn thread_read(&self, params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        self.calls.lock().unwrap().push("thread_read".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        Ok(self.thread_read_response.clone())
    }

    async fn thread_compact(
        &self,
        params: ThreadCompactParams,
    ) -> anyhow::Result<ThreadCompactResponse> {
        self.calls.lock().unwrap().push("thread_compact".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        Ok(self.thread_compact_response.clone())
    }

    async fn thread_resume(
        &self,
        params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        self.calls.lock().unwrap().push("thread_resume".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("ignored"));
        Ok(self.thread_resume_response.clone())
    }

    async fn agent_tree(&self, params: AgentTreeParams) -> anyhow::Result<AgentTreeResponse> {
        self.calls.lock().unwrap().push("agent_tree".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        Ok(self.agent_tree_response.clone())
    }

    async fn turn_start(&self, params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        self.calls.lock().unwrap().push("turn_start".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.prompt, "continue phase2");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.turn_mode, TurnMode::Plan);
        assert_eq!(
            params
                .turn_context
                .as_ref()
                .and_then(|context| context.model.as_ref())
                .map(|model| model.model_id.as_str()),
            Some("gpt-4.1-mini")
        );
        assert_eq!(
            params
                .turn_context
                .as_ref()
                .and_then(|context| context.thinking_mode),
            Some(ThinkingMode::High)
        );
        Ok(self.turn_start_response.clone())
    }

    async fn turn_interrupt(
        &self,
        params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        self.calls.lock().unwrap().push("turn_interrupt".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.turn_id.as_ref().map(TurnId::as_str), Some("turn_1"));
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        Ok(TurnInterruptResponse {
            thread_id: params.thread_id,
            interrupted_turn: Some(exagent::app_server::protocol::TurnState {
                turn_id: TurnId::new("turn_1"),
                status: TurnStatus::Interrupted,
            }),
        })
    }

    async fn approval_decision(
        &self,
        _params: ApprovalDecisionParams,
    ) -> anyhow::Result<ApprovalDecisionResponse> {
        self.calls.lock().unwrap().push("approval_decision".into());
        Err(AppServerError::InvalidRequest(
            "approval decision is not used in these API tests".into(),
        )
        .into())
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        self.calls.lock().unwrap().push("submit_boundary_op".into());
        match op {
            BoundaryOp::Initialize(_) => {
                Ok(BoundaryOpResponse::Initialized(sample_initialize_response()))
            }
            BoundaryOp::ThreadRead(_) => Ok(BoundaryOpResponse::ThreadRead(
                self.thread_read_response.clone(),
            )),
            BoundaryOp::ThreadCompact(_) => Ok(BoundaryOpResponse::ThreadCompacted(
                self.thread_compact_response.clone(),
            )),
            BoundaryOp::EventsReplay(_) => Ok(BoundaryOpResponse::EventsReplayed(
                self.events_replay_response.clone(),
            )),
            _ => Err(AppServerError::InvalidRequest("unsupported test op".into()).into()),
        }
    }

    async fn events_replay(
        &self,
        params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        self.calls.lock().unwrap().push("events_replay".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        Ok(self.events_replay_response.clone())
    }

    async fn events_subscribe(
        &self,
        params: EventsSubscribeParams,
    ) -> anyhow::Result<tokio::sync::broadcast::Receiver<RuntimeEvent>> {
        self.calls.lock().unwrap().push("events_subscribe".into());
        assert_eq!(params.thread_id.as_str(), "thread_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        let (_tx, rx) = tokio::sync::broadcast::channel(8);
        Ok(rx)
    }
}

enum ErrorKind {
    InvalidRequest,
    ThreadNotFound,
    ThreadBusy,
}

struct ErrorBoundary {
    kind: ErrorKind,
}

impl ErrorBoundary {
    fn error(&self) -> anyhow::Error {
        match self.kind {
            ErrorKind::InvalidRequest => {
                AppServerError::InvalidRequest("cwd must stay within workspace_root".into()).into()
            }
            ErrorKind::ThreadNotFound => {
                AppServerError::ThreadNotFound(ThreadId::new("missing-thread")).into()
            }
            ErrorKind::ThreadBusy => AppServerError::ThreadBusy(ThreadId::new("thread_123")).into(),
        }
    }
}

#[async_trait::async_trait]
impl AppServerBoundary for ErrorBoundary {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        Err(self.error())
    }

    async fn thread_start(
        &self,
        _params: ThreadStartParams,
    ) -> anyhow::Result<ThreadStartResponse> {
        Err(self.error())
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        Err(self.error())
    }

    async fn thread_compact(
        &self,
        _params: ThreadCompactParams,
    ) -> anyhow::Result<ThreadCompactResponse> {
        Err(self.error())
    }

    async fn thread_resume(
        &self,
        _params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        Err(self.error())
    }

    async fn agent_tree(&self, _params: AgentTreeParams) -> anyhow::Result<AgentTreeResponse> {
        Err(self.error())
    }

    async fn turn_start(&self, _params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        Err(self.error())
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        Err(self.error())
    }

    async fn approval_decision(
        &self,
        _params: ApprovalDecisionParams,
    ) -> anyhow::Result<ApprovalDecisionResponse> {
        Err(self.error())
    }

    async fn submit_boundary_op(&self, _op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        Err(self.error())
    }

    async fn events_replay(
        &self,
        _params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        Err(self.error())
    }

    async fn events_subscribe(
        &self,
        _params: EventsSubscribeParams,
    ) -> anyhow::Result<tokio::sync::broadcast::Receiver<RuntimeEvent>> {
        Err(self.error())
    }
}

#[tokio::test]
async fn health_route_returns_ok() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"status": "ok"})
    );
}

#[tokio::test]
async fn initialize_route_returns_protocol_capabilities() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(app, "/initialize", json!({})).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body,
        json!({
            "type": "initialized",
            "protocol_version": BOUNDARY_PROTOCOL_VERSION,
            "supported_ops": [
                "initialize",
                "thread_start",
                "thread_resume",
                "thread_read",
                "thread_compact",
                "agent_tree",
                "turn_start",
                "turn_interrupt",
                "approval_decision",
                "events_replay"
            ],
            "supported_streams": ["events_subscribe"],
            "supported_permission_profiles": ["full_access"]
        })
    );
}

#[tokio::test]
async fn removed_legacy_routes_are_not_public_boundary_surface() {
    let app = build_router(Arc::new(StubBoundary::new()));

    for route in ["/fork", "/inspect", "/collect", "/thread/spawn_child"] {
        let response = json_post(app.clone(), route, json!({})).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
}

#[tokio::test]
async fn run_route_accepts_existing_thread_id() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/run",
        json!({
            "prompt": "continue phase2",
            "workspace_root": ".",
            "cwd": ".",
            "thread_id": "thread_123",
            "thinking_mode": "medium"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "text": "done",
            "tool_calls": [{
                "id": "call_1",
                "name": "read_file",
                "arguments": {"path": "Cargo.toml"}
            }],
            "thread_id": "thread_123"
        })
    );
}

#[tokio::test]
async fn thread_start_route_accepts_workspace_and_cwd() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/start",
        json!({
            "workspace_root": ".",
            "cwd": "nested"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"thread": sample_thread_json()})
    );
}

#[tokio::test]
async fn thread_start_route_maps_invalid_request_errors_to_bad_request() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::InvalidRequest,
    }));

    let response = json_post(
        app,
        "/thread/start",
        json!({
            "workspace_root": ".",
            "cwd": "../outside"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        json!({"error": "invalid request: cwd must stay within workspace_root"})
    );
}

#[tokio::test]
async fn thread_read_route_accepts_thread_id_and_returns_lifecycle_state() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/read",
        json!({
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"thread": sample_thread_json()})
    );
}

#[tokio::test]
async fn thread_resume_route_accepts_thread_id_and_reports_ignored_overrides() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/resume",
        json!({
            "thread_id": "thread_123",
            "workspace_root": ".",
            "cwd": "ignored"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "thread": sample_thread_json(),
            "ignored_overrides": ["cwd"]
        })
    );
}

#[tokio::test]
async fn agent_tree_route_accepts_root_thread_id_and_returns_agent_roster() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/agent/tree",
        json!({
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"root": sample_agent_tree_json()})
    );
}

#[tokio::test]
async fn turn_start_route_accepts_thread_id_and_prompt() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/turn/start",
        json!({
            "thread_id": "thread_123",
            "prompt": "continue phase2",
            "workspace_root": ".",
            "turn_mode": "plan",
            "turn_context": {
                "model": {
                    "provider_id": "openai",
                    "model_id": "gpt-4.1-mini"
                },
                "thinking_mode": "high"
            }
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "thread_id": "thread_123",
            "turn": {
                "id": "turn_1",
                "status": "in_progress",
                "items": []
            }
        })
    );
}

#[tokio::test]
async fn turn_start_route_maps_thread_busy_errors_to_conflict() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::ThreadBusy,
    }));

    let response = json_post(
        app,
        "/turn/start",
        json!({
            "thread_id": "thread_123",
            "prompt": "continue phase2",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await,
        json!({"error": "thread is busy: thread_123"})
    );
}

#[tokio::test]
async fn turn_interrupt_route_accepts_thread_id_and_turn_id() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/turn/interrupt",
        json!({
            "thread_id": "thread_123",
            "turn_id": "turn_1",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "thread_id": "thread_123",
            "interrupted_turn": {
                "turn_id": "turn_1",
                "status": "interrupted"
            }
        })
    );
}

#[tokio::test]
async fn events_replay_route_returns_runtime_events() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/events/replay",
        json!({
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, sample_events_replay_json());
}

#[tokio::test]
async fn events_replay_route_maps_missing_thread_errors_to_not_found() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::ThreadNotFound,
    }));

    let response = json_post(
        app,
        "/events/replay",
        json!({
            "thread_id": "missing-thread",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await,
        json!({"error": "thread not found: missing-thread"})
    );
}

#[tokio::test]
async fn events_subscribe_route_streams_replay_events_then_closes() {
    let runner = Arc::new(StubBoundary::new());
    let app = build_router(runner.clone());

    let response = json_post(
        app,
        "/events/subscribe",
        json!({
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("data:"));
    assert!(body.contains("assistant_turn"));
    assert!(body.contains("turn complete"));
    assert!(body.contains("call_1"));
    assert!(body.contains("read_file"));
    assert!(!body.contains("hidden reasoning"));
    assert!(!body.contains("hidden-tool-signature"));
    assert!(!body.contains("reasoning"));
    assert!(!body.contains("thought_signature"));
    assert_eq!(
        runner.calls.lock().unwrap().as_slice(),
        ["events_subscribe", "events_replay"]
    );
}

#[tokio::test]
async fn thread_op_route_accepts_events_replay_as_first_class_op() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/op",
        json!({
            "type": "events_replay",
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "type": "events_replayed",
            "thread_id": "thread_123",
            "events": sample_events_replay_json()["events"].clone()
        })
    );
}

#[tokio::test]
async fn thread_op_route_accepts_thread_read_as_boundary_op() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/op",
        json!({
            "type": "thread_read",
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "type": "thread_read",
            "thread": sample_thread_json()
        })
    );
}

#[tokio::test]
async fn thread_op_route_accepts_thread_compact_as_boundary_op() {
    let app = build_router(Arc::new(StubBoundary::new()));

    let response = json_post(
        app,
        "/thread/op",
        json!({
            "type": "thread_compact",
            "thread_id": "thread_123",
            "workspace_root": "."
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "type": "thread_compacted",
            "thread_id": "thread_123",
            "latest_compaction": null
        })
    );
}

#[test]
fn parse_cli_resume_command_reads_thread_id_and_prompt() {
    let command = parse_cli_command(vec![
        "resume".to_string(),
        "thread_123".to_string(),
        "continue phase2".to_string(),
    ])
    .unwrap();

    assert_eq!(
        command,
        CliCommand::Resume {
            thread_id: ThreadId::new("thread_123"),
            prompt: "continue phase2".into(),
        }
    );
}

#[test]
fn parse_cli_rejects_removed_legacy_commands() {
    for args in [
        vec!["fork", "thread_parent", "spec", "draft"],
        vec!["inspect", "thread_parent"],
        vec!["collect", "thread_child"],
    ] {
        assert!(parse_cli_command(args.into_iter().map(str::to_string).collect()).is_err());
    }
}

async fn json_post(app: axum::Router, uri: &str, body: Value) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice::<Value>(&body).unwrap()
}

fn sample_initialize_response() -> InitializeResponse {
    InitializeResponse {
        protocol_version: BOUNDARY_PROTOCOL_VERSION.to_string(),
        supported_ops: vec![
            BoundaryCapability::Initialize,
            BoundaryCapability::ThreadStart,
            BoundaryCapability::ThreadResume,
            BoundaryCapability::ThreadRead,
            BoundaryCapability::ThreadCompact,
            BoundaryCapability::AgentTree,
            BoundaryCapability::TurnStart,
            BoundaryCapability::TurnInterrupt,
            BoundaryCapability::ApprovalDecision,
            BoundaryCapability::EventsReplay,
        ],
        supported_streams: vec![BoundaryCapability::EventsSubscribe],
        supported_permission_profiles: vec![PermissionProfile::FullAccess],
    }
}

fn sample_agent_tree_response() -> AgentTreeResponse {
    AgentTreeResponse {
        root: AgentTreeNode {
            thread_id: Some(ThreadId::new("thread_123")),
            parent_thread_id: None,
            root_thread_id: ThreadId::new("thread_123"),
            depth: 0,
            agent_path: "root".into(),
            status: AgentTreeAgentStatus::Idle,
            agent_type: None,
            agent_role: None,
            agent_nickname: None,
            last_task_message: None,
            last_activity: None,
            children: vec![AgentTreeNode {
                thread_id: Some(ThreadId::new("thread_child")),
                parent_thread_id: Some(ThreadId::new("thread_123")),
                root_thread_id: ThreadId::new("thread_123"),
                depth: 1,
                agent_path: "root/researcher".into(),
                status: AgentTreeAgentStatus::Running,
                agent_type: Some(AgentType::Explorer),
                agent_role: Some("research role".into()),
                agent_nickname: Some("Rhea".into()),
                last_task_message: Some("map the inspector state".into()),
                last_activity: Some("also check activeSessionId consumers".into()),
                children: vec![],
            }],
        },
    }
}

fn sample_run_response(text: &str) -> AgentRunResponse {
    AgentRunResponse {
        text: Some(text.into()),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "Cargo.toml"}),
            thought_signature: None,
        }],
        thread_id: ThreadId::new("thread_123"),
    }
}

fn sample_thread_start_response() -> ThreadStartResponse {
    ThreadStartResponse {
        thread: sample_thread_view(),
    }
}

fn sample_thread_read_response() -> ThreadReadResponse {
    ThreadReadResponse {
        thread: sample_thread_view(),
    }
}

fn sample_thread_compact_response() -> ThreadCompactResponse {
    ThreadCompactResponse {
        thread_id: ThreadId::new("thread_123"),
        latest_compaction: None,
    }
}

fn sample_thread_resume_response() -> ThreadResumeResponse {
    ThreadResumeResponse {
        thread: sample_thread_view(),
        ignored_overrides: vec![IgnoredOverrideField::Cwd],
    }
}

fn sample_turn_start_response() -> TurnStartResponse {
    TurnStartResponse {
        thread_id: ThreadId::new("thread_123"),
        turn: TurnView {
            id: TurnId::new("turn_1"),
            status: TurnStatus::InProgress,
            items: vec![],
        },
    }
}

fn sample_thread_view() -> ThreadView {
    ThreadView {
        id: ThreadId::new("thread_123"),
        status: ThreadStatus::Idle,
        active_turn: None,
        turns: vec![],
        goal: None,
    }
}

fn sample_events_replay_response() -> EventsReplayResponse {
    EventsReplayResponse {
        thread_id: ThreadId::new("thread_123"),
        events: vec![RuntimeEvent {
            event_id: EventId::new("evt_1"),
            thread_id: ThreadId::new("thread_123"),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::AssistantTurn {
                turn: AssistantTurn {
                    text: Some("turn complete".into()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: json!({"path": "Cargo.toml"}),
                        thought_signature: Some(json!("hidden-tool-signature")),
                    }],
                    reasoning: vec![ReasoningBlock {
                        text: "hidden reasoning".into(),
                        signature: Some(ReasoningSignature::GeminiThoughtSignature(
                            "hidden-reasoning-signature".into(),
                        )),
                        redacted: false,
                    }],
                },
            },
        }],
        snapshot: None,
    }
}

fn sample_thread_json() -> Value {
    json!({
        "id": "thread_123",
        "status": "idle",
        "active_turn": null,
        "turns": []
    })
}

fn sample_agent_tree_json() -> Value {
    json!({
        "thread_id": "thread_123",
        "root_thread_id": "thread_123",
        "depth": 0,
        "agent_path": "root",
        "status": "idle",
        "children": [
            {
                "thread_id": "thread_child",
                "parent_thread_id": "thread_123",
                "root_thread_id": "thread_123",
                "depth": 1,
                "agent_path": "root/researcher",
                "status": "running",
                "agent_type": "explorer",
                "agent_role": "research role",
                "agent_nickname": "Rhea",
                "last_task_message": "map the inspector state",
                "last_activity": "also check activeSessionId consumers"
            }
        ]
    })
}

fn sample_events_replay_json() -> Value {
    json!({
        "thread_id": "thread_123",
        "events": [{
            "event_id": "evt_1",
            "thread_id": "thread_123",
            "turn_id": "turn_1",
            "kind": {
                "type": "assistant_turn",
                "turn": {
                    "text": "turn complete",
                    "tool_calls": [{
                        "id": "call_1",
                        "name": "read_file",
                        "arguments": {"path": "Cargo.toml"}
                    }]
                }
            }
        }]
    })
}
