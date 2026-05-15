use std::sync::Arc;
use std::sync::Mutex;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use exagent::api::build_router;
use exagent::app_server::protocol::{
    AgentRunResponse, BoundaryCapability, BoundaryOp, BoundaryOpResponse, CollectParams,
    CollectResponse, EventsReplayParams, EventsReplayResponse, ForkParams, IgnoredOverrideField,
    InitializeResponse, InspectParams, InspectResponse, RunParams, ThreadReadParams,
    ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse, ThreadSpawnChildParams,
    ThreadSpawnChildResponse, ThreadStartParams, ThreadStartResponse, ThreadStatus,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
    BOUNDARY_PROTOCOL_VERSION,
};
use exagent::app_server::AppServerBoundary;
use exagent::app_server::AppServerError;
use exagent::cli::{parse_cli_command, CliCommand};
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::orchestration::{
    ChildLifecycleStatus, ChildSessionSummary, CollectedChildSession, CollectedOutput,
    CollectedOutputKind,
};
use exagent::result_contract::{
    StructuredResultPayload, StructuredSessionResult, STRUCTURED_RESULT_SCHEMA_VERSION,
};
use exagent::session::AgentRole;
use exagent::types::{EventId, SessionId, ToolCall, TurnId};
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::util::ServiceExt;

struct StubRunner {
    response: AgentRunResponse,
    inspect_response: InspectResponse,
    collect_response: CollectResponse,
    thread_start_response: ThreadStartResponse,
    thread_read_response: ThreadReadResponse,
    thread_resume_response: ThreadResumeResponse,
    turn_start_response: TurnStartResponse,
    thread_spawn_child_response: ThreadSpawnChildResponse,
    events_replay_response: EventsReplayResponse,
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl AppServerBoundary for StubRunner {
    async fn run(&self, params: RunParams) -> anyhow::Result<AgentRunResponse> {
        self.calls.lock().unwrap().push("run".into());
        assert_eq!(params.prompt, "continue phase2");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("."));
        assert_eq!(
            params.session_id.as_ref().map(SessionId::as_str),
            Some("session_123")
        );

        Ok(self.response.clone())
    }

    async fn fork(&self, params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        self.calls.lock().unwrap().push("fork".into());
        assert_eq!(params.parent_session_id.as_str(), "session_123");
        assert_eq!(params.agent_role, AgentRole::Spec);
        assert_eq!(params.prompt, "draft goals");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(
            params.spawned_by_turn_id.as_ref().map(TurnId::as_str),
            Some("turn_1")
        );

        Ok(self.response.clone())
    }

    async fn inspect(&self, params: InspectParams) -> anyhow::Result<InspectResponse> {
        self.calls.lock().unwrap().push("inspect".into());
        assert_eq!(params.parent_session_id.as_str(), "session_parent");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.inspect_response.clone())
    }

    async fn collect(&self, params: CollectParams) -> anyhow::Result<CollectResponse> {
        self.calls.lock().unwrap().push("collect".into());
        assert_eq!(params.session_id.as_str(), "session_child");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.collect_response.clone())
    }

    async fn thread_start(&self, params: ThreadStartParams) -> anyhow::Result<ThreadStartResponse> {
        self.calls.lock().unwrap().push("thread_start".into());
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("nested"));

        Ok(self.thread_start_response.clone())
    }

    async fn thread_read(&self, params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        self.calls.lock().unwrap().push("thread_read".into());
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.thread_read_response.clone())
    }

    async fn thread_resume(
        &self,
        params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        self.calls.lock().unwrap().push("thread_resume".into());
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("ignored"));

        Ok(self.thread_resume_response.clone())
    }

    async fn turn_start(&self, params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        self.calls.lock().unwrap().push("turn_start".into());
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.prompt, "continue phase2");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.turn_start_response.clone())
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt should not be called in these api adapter tests");
    }

    async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
        self.calls.lock().unwrap().push("thread_spawn_child".into());
        assert_eq!(params.parent_thread_id.as_str(), "session_123");
        assert_eq!(params.agent_role, AgentRole::Spec);
        assert_eq!(params.prompt, "draft goals");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(params.cwd.as_deref(), Some("ignored"));
        assert_eq!(
            params.spawned_by_turn_id.as_ref().map(TurnId::as_str),
            Some("turn_1")
        );

        Ok(self.thread_spawn_child_response.clone())
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        self.calls.lock().unwrap().push("submit_boundary_op".into());

        match op {
            BoundaryOp::Initialize(_) => Ok(BoundaryOpResponse::Initialized(InitializeResponse {
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
            })),
            _ => panic!("unexpected submit_boundary_op call in api adapter tests"),
        }
    }

    async fn events_replay(
        &self,
        params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        self.calls.lock().unwrap().push("events_replay".into());
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.events_replay_response.clone())
    }
}

struct ForkIgnoresCwdRunner {
    response: AgentRunResponse,
}

#[async_trait::async_trait]
impl AppServerBoundary for ForkIgnoresCwdRunner {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        panic!("run should not be called in fork test");
    }

    async fn fork(&self, params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        assert_eq!(params.parent_session_id.as_str(), "session_123");
        assert_eq!(params.agent_role, AgentRole::Spec);
        assert_eq!(params.prompt, "draft goals");
        assert_eq!(params.workspace_root.as_deref(), Some("."));
        assert_eq!(
            params.spawned_by_turn_id.as_ref().map(TurnId::as_str),
            Some("turn_1")
        );

        Ok(self.response.clone())
    }

    async fn inspect(&self, _params: InspectParams) -> anyhow::Result<InspectResponse> {
        panic!("inspect should not be called in fork test");
    }

    async fn collect(&self, _params: CollectParams) -> anyhow::Result<CollectResponse> {
        panic!("collect should not be called in fork test");
    }

    async fn thread_start(
        &self,
        _params: ThreadStartParams,
    ) -> anyhow::Result<ThreadStartResponse> {
        panic!("thread_start should not be called in fork test");
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        panic!("thread_read should not be called in fork test");
    }

    async fn thread_resume(
        &self,
        _params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        panic!("thread_resume should not be called in fork test");
    }

    async fn turn_start(&self, _params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        panic!("turn_start should not be called in fork test");
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt should not be called in fork test");
    }

    async fn thread_spawn_child(
        &self,
        _params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
        panic!("thread_spawn_child should not be called in fork test");
    }

    async fn submit_boundary_op(&self, _op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        panic!("submit_boundary_op should not be called in fork test");
    }

    async fn events_replay(
        &self,
        _params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        panic!("events_replay should not be called in fork test");
    }
}

struct ErrorBoundary {
    kind: ErrorKind,
}

enum ErrorKind {
    InvalidRequest,
    ThreadNotFound,
    ThreadBusy,
}

impl ErrorBoundary {
    fn error(&self) -> anyhow::Error {
        match self.kind {
            ErrorKind::InvalidRequest => {
                AppServerError::InvalidRequest("cwd must stay within workspace_root".into()).into()
            }
            ErrorKind::ThreadNotFound => {
                AppServerError::ThreadNotFound(SessionId::new("missing-thread")).into()
            }
            ErrorKind::ThreadBusy => {
                AppServerError::ThreadBusy(SessionId::new("session_123")).into()
            }
        }
    }
}

#[async_trait::async_trait]
impl AppServerBoundary for ErrorBoundary {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        Err(self.error())
    }

    async fn fork(&self, _params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        Err(self.error())
    }

    async fn inspect(&self, _params: InspectParams) -> anyhow::Result<InspectResponse> {
        Err(self.error())
    }

    async fn collect(&self, _params: CollectParams) -> anyhow::Result<CollectResponse> {
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

    async fn thread_resume(
        &self,
        _params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
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

    async fn thread_spawn_child(
        &self,
        _params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
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
}

struct BoundaryOpRunner {
    response: BoundaryOpResponse,
}

#[async_trait::async_trait]
impl AppServerBoundary for BoundaryOpRunner {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        panic!("run should not be called in thread op test");
    }

    async fn fork(&self, _params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        panic!("fork should not be called in thread op test");
    }

    async fn inspect(&self, _params: InspectParams) -> anyhow::Result<InspectResponse> {
        panic!("inspect should not be called in thread op test");
    }

    async fn collect(&self, _params: CollectParams) -> anyhow::Result<CollectResponse> {
        panic!("collect should not be called in thread op test");
    }

    async fn thread_start(
        &self,
        _params: ThreadStartParams,
    ) -> anyhow::Result<ThreadStartResponse> {
        panic!("thread_start should not be called in thread op test");
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        panic!("thread_read should not be called in thread op test");
    }

    async fn thread_resume(
        &self,
        _params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        panic!("thread_resume should not be called in thread op test");
    }

    async fn turn_start(&self, _params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        panic!("turn_start should not be called in thread op test");
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt should not be called in thread op test");
    }

    async fn thread_spawn_child(
        &self,
        _params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
        panic!("thread_spawn_child should not be called in thread op test");
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        let BoundaryOp::EventsReplay(params) = op else {
            panic!("expected events replay op");
        };
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.response.clone())
    }

    async fn events_replay(
        &self,
        _params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        panic!("events_replay should not be called in thread op test");
    }
}

struct ThreadReadOpRunner {
    response: BoundaryOpResponse,
}

#[async_trait::async_trait]
impl AppServerBoundary for ThreadReadOpRunner {
    async fn run(&self, _params: RunParams) -> anyhow::Result<AgentRunResponse> {
        panic!("run should not be called in thread read op test");
    }

    async fn fork(&self, _params: ForkParams) -> anyhow::Result<AgentRunResponse> {
        panic!("fork should not be called in thread read op test");
    }

    async fn inspect(&self, _params: InspectParams) -> anyhow::Result<InspectResponse> {
        panic!("inspect should not be called in thread read op test");
    }

    async fn collect(&self, _params: CollectParams) -> anyhow::Result<CollectResponse> {
        panic!("collect should not be called in thread read op test");
    }

    async fn thread_start(
        &self,
        _params: ThreadStartParams,
    ) -> anyhow::Result<ThreadStartResponse> {
        panic!("thread_start should not be called in thread read op test");
    }

    async fn thread_read(&self, _params: ThreadReadParams) -> anyhow::Result<ThreadReadResponse> {
        panic!("thread_read should not be called directly in thread read op test");
    }

    async fn thread_resume(
        &self,
        _params: ThreadResumeParams,
    ) -> anyhow::Result<ThreadResumeResponse> {
        panic!("thread_resume should not be called in thread read op test");
    }

    async fn turn_start(&self, _params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
        panic!("turn_start should not be called in thread read op test");
    }

    async fn turn_interrupt(
        &self,
        _params: TurnInterruptParams,
    ) -> anyhow::Result<TurnInterruptResponse> {
        panic!("turn_interrupt should not be called in thread read op test");
    }

    async fn thread_spawn_child(
        &self,
        _params: ThreadSpawnChildParams,
    ) -> anyhow::Result<ThreadSpawnChildResponse> {
        panic!("thread_spawn_child should not be called in thread read op test");
    }

    async fn submit_boundary_op(&self, op: BoundaryOp) -> anyhow::Result<BoundaryOpResponse> {
        let BoundaryOp::ThreadRead(params) = op else {
            panic!("expected thread read op");
        };
        assert_eq!(params.thread_id.as_str(), "session_123");
        assert_eq!(params.workspace_root.as_deref(), Some("."));

        Ok(self.response.clone())
    }

    async fn events_replay(
        &self,
        _params: EventsReplayParams,
    ) -> anyhow::Result<EventsReplayResponse> {
        panic!("events_replay should not be called in thread read op test");
    }
}

#[tokio::test]
async fn health_route_returns_ok() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

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
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/initialize")
                .header("content-type", "application/json")
                .body(Body::from(json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "type": "initialized",
            "protocol_version": BOUNDARY_PROTOCOL_VERSION,
            "supported_ops": [
                "initialize",
                "thread_start",
                "thread_resume",
                "thread_spawn_child",
                "thread_read",
                "turn_start",
                "turn_interrupt",
                "events_replay"
            ]
        })
    );
}

#[tokio::test]
async fn runtime_threads_route_is_not_public_boundary_surface() {
    let dir = tempdir().unwrap();
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/threads")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_root": dir.path(),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_route_accepts_existing_session_id() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("done".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "Cargo.toml"}),
            }],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/run")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "continue phase2",
                        "workspace_root": ".",
                        "cwd": ".",
                        "session_id": "session_123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "text": "done",
            "tool_calls": [{
                "id": "call_1",
                "name": "read_file",
                "arguments": {"path": "Cargo.toml"}
            }],
            "session_id": "session_123",
            "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
            "events_path": ".exagent/sessions/session_123/events.jsonl"
        })
    );
}

#[tokio::test]
async fn fork_route_accepts_parent_session_id_and_agent_role() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![ToolCall {
                id: "call_fork_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "docs/plan.md"}),
            }],
            session_id: SessionId::new("session_child"),
            snapshot_path: ".exagent/sessions/session_child/snapshot.json".into(),
            events_path: ".exagent/sessions/session_child/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/fork")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "parent_session_id": "session_123",
                        "agent_role": "spec",
                        "prompt": "draft goals",
                        "workspace_root": ".",
                        "spawned_by_turn_id": "turn_1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "text": "unused",
            "tool_calls": [{
                "id": "call_fork_1",
                "name": "read_file",
                "arguments": {"path": "docs/plan.md"}
            }],
            "session_id": "session_child",
            "snapshot_path": ".exagent/sessions/session_child/snapshot.json",
            "events_path": ".exagent/sessions/session_child/events.jsonl"
        })
    );
}

#[tokio::test]
async fn fork_route_ignores_cwd_override_and_keeps_parent_context_authoritative() {
    let app = build_router(Arc::new(ForkIgnoresCwdRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_child"),
            snapshot_path: ".exagent/sessions/session_child/snapshot.json".into(),
            events_path: ".exagent/sessions/session_child/events.jsonl".into(),
        },
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/fork")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "parent_session_id": "session_123",
                        "agent_role": "spec",
                        "prompt": "draft goals",
                        "workspace_root": ".",
                        "cwd": "nested",
                        "spawned_by_turn_id": "turn_1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn inspect_route_accepts_parent_session_id() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/inspect")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "parent_session_id": "session_parent",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "children": [{
                "session_id": "session_child",
                "parent_session_id": "session_parent",
                "root_session_id": "session_parent",
                "spawned_by_turn_id": "turn_1",
                "agent_role": "spec",
                "status": "completed",
                "snapshot_path": ".exagent/sessions/session_child/snapshot.json",
                "events_path": ".exagent/sessions/session_child/events.jsonl"
            }]
        })
    );
}

#[tokio::test]
async fn collect_route_accepts_child_session_id() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collect")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "session_child",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
                "session": {
                    "child": {
                        "session_id": "session_child",
                    "parent_session_id": "session_parent",
                    "root_session_id": "session_parent",
                    "spawned_by_turn_id": "turn_1",
                    "agent_role": "spec",
                    "status": "completed",
                        "snapshot_path": ".exagent/sessions/session_child/snapshot.json",
                        "events_path": ".exagent/sessions/session_child/events.jsonl"
                    },
                    "latest_useful_output": {
                        "kind": "assistant_text",
                        "content": "spec summary",
                    "tool_name": null,
                    "tool_call_id": null
                }
            }
        })
    );
}

#[tokio::test]
async fn collect_route_serializes_structured_result() {
    let app = build_router(Arc::new(StubRunner {
        response: AgentRunResponse {
            text: Some("unused".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        },
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response_with_structured_result(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collect")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "session_child",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "session": {
                "child": {
                    "session_id": "session_child",
                    "parent_session_id": "session_parent",
                    "root_session_id": "session_parent",
                    "spawned_by_turn_id": "turn_1",
                    "agent_role": "spec",
                    "status": "completed",
                    "snapshot_path": ".exagent/sessions/session_child/snapshot.json",
                    "events_path": ".exagent/sessions/session_child/events.jsonl"
                },
                "structured_result": {
                    "schema_version": "phase3_p2/v1",
                    "agent_role": "spec",
                    "session_id": "session_child",
                    "parent_session_id": "session_parent",
                    "source_turn_id": "turn_1",
                    "summary": "spec summary",
                    "assumptions": ["P1 is stable"],
                    "risks": ["scope creep"],
                    "open_questions": ["none"],
                    "payload": {
                        "kind": "spec",
                        "goals": ["add structured contracts"],
                        "non_goals": ["no planner"],
                        "acceptance_criteria": ["collect returns typed result"],
                        "contract_boundaries": ["inspect stays topology-only"]
                    }
                },
                "latest_useful_output": {
                    "kind": "assistant_text",
                    "content": "spec summary",
                    "tool_name": null,
                    "tool_call_id": null
                }
            }
        })
    );
}

#[tokio::test]
async fn thread_start_route_accepts_workspace_and_cwd() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/start")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_root": ".",
                        "cwd": "nested"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "thread_id": "session_123",
            "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
            "events_path": ".exagent/sessions/session_123/events.jsonl"
        })
    );
}

#[tokio::test]
async fn thread_start_route_maps_invalid_request_errors_to_bad_request() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::InvalidRequest,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/start")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_root": ".",
                        "cwd": "../outside"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"error": "invalid request: cwd must stay within workspace_root"})
    );
}

#[tokio::test]
async fn thread_read_route_accepts_thread_id_and_returns_lifecycle_state() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/read")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "thread_id": "session_123",
            "status": "idle",
            "active_turn": null,
            "latest_turn": null,
            "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
            "events_path": ".exagent/sessions/session_123/events.jsonl"
        })
    );
}

#[tokio::test]
async fn thread_resume_route_accepts_thread_id_and_reports_ignored_overrides() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/resume")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "workspace_root": ".",
                        "cwd": "ignored"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "thread": {
                "thread_id": "session_123",
                "status": "idle",
                "active_turn": null,
                "latest_turn": null,
                "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
                "events_path": ".exagent/sessions/session_123/events.jsonl"
            },
            "ignored_overrides": ["cwd"]
        })
    );
}

#[tokio::test]
async fn turn_start_route_accepts_thread_id_and_prompt() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/turn/start")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "prompt": "continue phase2",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "thread_id": "session_123",
            "turn_id": "turn_1",
            "output": {
                "text": "turn complete",
                "tool_calls": [],
                "session_id": "session_123",
                "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
                "events_path": ".exagent/sessions/session_123/events.jsonl"
            }
        })
    );
}

#[tokio::test]
async fn turn_start_route_maps_thread_busy_errors_to_conflict() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::ThreadBusy,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/turn/start")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "prompt": "rejected while running",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"error": "thread is busy: session_123"})
    );
}

#[tokio::test]
async fn thread_spawn_child_route_accepts_parent_role_and_prompt() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread_spawn_child")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "parent_thread_id": "session_123",
                        "agent_role": "spec",
                        "prompt": "draft goals",
                        "workspace_root": ".",
                        "cwd": "ignored",
                        "spawned_by_turn_id": "turn_1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "parent_thread_id": "session_123",
            "child_thread_id": "session_child",
            "agent_role": "spec",
            "ignored_overrides": ["cwd"],
            "output": {
                "text": "child complete",
                "tool_calls": [],
                "session_id": "session_child",
                "snapshot_path": ".exagent/sessions/session_child/snapshot.json",
                "events_path": ".exagent/sessions/session_child/events.jsonl"
            }
        })
    );
}

#[tokio::test]
async fn thread_spawn_child_slash_route_accepts_parent_role_and_prompt() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/spawn_child")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "parent_thread_id": "session_123",
                        "agent_role": "spec",
                        "prompt": "draft goals",
                        "workspace_root": ".",
                        "cwd": "ignored",
                        "spawned_by_turn_id": "turn_1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn events_replay_route_returns_runtime_events() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/events_replay")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "thread_id": "session_123",
            "events": [{
                "event_id": "evt_1",
                "session_id": "session_123",
                "turn_id": "turn_1",
                "kind": {
                    "type": "assistant_turn",
                    "turn": {
                        "text": "turn complete",
                        "tool_calls": []
                    }
                }
            }]
        })
    );
}

#[tokio::test]
async fn events_replay_slash_route_returns_runtime_events() {
    let app = build_router(Arc::new(StubRunner {
        response: sample_run_response("unused"),
        inspect_response: sample_inspect_response(),
        collect_response: sample_collect_response(),
        thread_start_response: sample_thread_start_response(),
        thread_read_response: sample_thread_read_response(),
        thread_resume_response: sample_thread_resume_response(),
        turn_start_response: sample_turn_start_response(),
        thread_spawn_child_response: sample_thread_spawn_child_response(),
        events_replay_response: sample_events_replay_response(),
        calls: Mutex::new(vec![]),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/events/replay")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "session_123",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn events_replay_route_maps_missing_thread_errors_to_not_found() {
    let app = build_router(Arc::new(ErrorBoundary {
        kind: ErrorKind::ThreadNotFound,
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/events_replay")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "thread_id": "missing-thread",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"error": "thread not found: missing-thread"})
    );
}

#[tokio::test]
async fn thread_op_route_accepts_events_replay_as_first_class_op() {
    let app = build_router(Arc::new(BoundaryOpRunner {
        response: BoundaryOpResponse::EventsReplayed(sample_events_replay_response()),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/op")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "type": "events_replay",
                        "thread_id": "session_123",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "type": "events_replayed",
            "thread_id": "session_123",
            "events": [{
                "event_id": "evt_1",
                "session_id": "session_123",
                "turn_id": "turn_1",
                "kind": {
                    "type": "assistant_turn",
                    "turn": {
                        "text": "turn complete",
                        "tool_calls": []
                    }
                }
            }]
        })
    );
}

#[tokio::test]
async fn thread_op_route_accepts_thread_read_as_boundary_op() {
    let app = build_router(Arc::new(ThreadReadOpRunner {
        response: BoundaryOpResponse::ThreadRead(sample_thread_read_response()),
    }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/thread/op")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "type": "thread_read",
                        "thread_id": "session_123",
                        "workspace_root": "."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "type": "thread_read",
            "thread_id": "session_123",
            "status": "idle",
            "active_turn": null,
            "latest_turn": null,
            "snapshot_path": ".exagent/sessions/session_123/snapshot.json",
            "events_path": ".exagent/sessions/session_123/events.jsonl"
        })
    );
}

#[test]
fn parse_cli_resume_command_reads_session_id_and_prompt() {
    let args = vec![
        "resume".to_string(),
        "session_123".to_string(),
        "continue phase2".to_string(),
    ];

    let command = parse_cli_command(args).unwrap();

    assert_eq!(
        command,
        CliCommand::Resume {
            session_id: SessionId::new("session_123"),
            prompt: "continue phase2".into(),
        }
    );
}

#[test]
fn parse_cli_fork_command_reads_parent_session_id_agent_role_and_prompt() {
    let args = vec![
        "fork".to_string(),
        "session_123".to_string(),
        "spec".to_string(),
        "draft goals".to_string(),
    ];

    let command = parse_cli_command(args).unwrap();

    assert_eq!(
        command,
        CliCommand::Fork {
            parent_session_id: SessionId::new("session_123"),
            agent_role: AgentRole::Spec,
            prompt: "draft goals".into(),
        }
    );
}

#[test]
fn parse_cli_inspect_command_reads_parent_session_id() {
    let args = vec!["inspect".to_string(), "session_parent".to_string()];

    let command = parse_cli_command(args).unwrap();

    assert_eq!(
        command,
        CliCommand::Inspect {
            parent_session_id: SessionId::new("session_parent"),
        }
    );
}

#[test]
fn parse_cli_collect_command_reads_child_session_id() {
    let args = vec!["collect".to_string(), "session_child".to_string()];

    let command = parse_cli_command(args).unwrap();

    assert_eq!(
        command,
        CliCommand::Collect {
            session_id: SessionId::new("session_child"),
        }
    );
}

fn sample_run_response(text: &str) -> AgentRunResponse {
    AgentRunResponse {
        text: Some(text.into()),
        tool_calls: vec![],
        session_id: SessionId::new("session_123"),
        snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
        events_path: ".exagent/sessions/session_123/events.jsonl".into(),
    }
}

fn sample_thread_start_response() -> ThreadStartResponse {
    ThreadStartResponse {
        thread_id: SessionId::new("session_123"),
        snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
        events_path: ".exagent/sessions/session_123/events.jsonl".into(),
    }
}

fn sample_thread_read_response() -> ThreadReadResponse {
    ThreadReadResponse {
        thread_id: SessionId::new("session_123"),
        status: ThreadStatus::Idle,
        active_turn: None,
        latest_turn: None,
        snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
        events_path: ".exagent/sessions/session_123/events.jsonl".into(),
    }
}

fn sample_thread_resume_response() -> ThreadResumeResponse {
    ThreadResumeResponse {
        thread: sample_thread_read_response(),
        ignored_overrides: vec![IgnoredOverrideField::Cwd],
    }
}

fn sample_turn_start_response() -> TurnStartResponse {
    TurnStartResponse {
        thread_id: SessionId::new("session_123"),
        turn_id: TurnId::new("turn_1"),
        output: sample_run_response("turn complete"),
    }
}

fn sample_thread_spawn_child_response() -> ThreadSpawnChildResponse {
    ThreadSpawnChildResponse {
        parent_thread_id: SessionId::new("session_123"),
        child_thread_id: SessionId::new("session_child"),
        agent_role: AgentRole::Spec,
        ignored_overrides: vec![IgnoredOverrideField::Cwd],
        output: AgentRunResponse {
            text: Some("child complete".into()),
            tool_calls: vec![],
            session_id: SessionId::new("session_child"),
            snapshot_path: ".exagent/sessions/session_child/snapshot.json".into(),
            events_path: ".exagent/sessions/session_child/events.jsonl".into(),
        },
    }
}

fn sample_events_replay_response() -> EventsReplayResponse {
    EventsReplayResponse {
        thread_id: SessionId::new("session_123"),
        events: vec![RuntimeEvent {
            event_id: EventId::new("evt_1"),
            session_id: SessionId::new("session_123"),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::AssistantTurn {
                turn: exagent::types::AssistantTurn {
                    text: Some("turn complete".into()),
                    tool_calls: vec![],
                },
            },
        }],
        snapshot: None,
    }
}

fn sample_summary() -> ChildSessionSummary {
    ChildSessionSummary {
        session_id: SessionId::new("session_child"),
        parent_session_id: SessionId::new("session_parent"),
        root_session_id: SessionId::new("session_parent"),
        spawned_by_turn_id: Some(TurnId::new("turn_1")),
        agent_role: AgentRole::Spec,
        status: ChildLifecycleStatus::Completed,
        snapshot_path: ".exagent/sessions/session_child/snapshot.json".into(),
        events_path: ".exagent/sessions/session_child/events.jsonl".into(),
    }
}

fn sample_inspect_response() -> InspectResponse {
    InspectResponse {
        children: vec![sample_summary()],
    }
}

fn sample_collect_response() -> CollectResponse {
    CollectResponse {
        session: CollectedChildSession {
            child: sample_summary(),
            structured_result: None,
            latest_useful_output: Some(CollectedOutput {
                kind: CollectedOutputKind::AssistantText,
                content: "spec summary".into(),
                tool_name: None,
                tool_call_id: None,
            }),
        },
    }
}

fn sample_collect_response_with_structured_result() -> CollectResponse {
    CollectResponse {
        session: CollectedChildSession {
            child: sample_summary(),
            structured_result: Some(StructuredSessionResult {
                schema_version: STRUCTURED_RESULT_SCHEMA_VERSION.into(),
                agent_role: AgentRole::Spec,
                session_id: SessionId::new("session_child"),
                parent_session_id: Some(SessionId::new("session_parent")),
                source_turn_id: Some(TurnId::new("turn_1")),
                summary: "spec summary".into(),
                assumptions: vec!["P1 is stable".into()],
                risks: vec!["scope creep".into()],
                open_questions: vec!["none".into()],
                payload: StructuredResultPayload::Spec {
                    goals: vec!["add structured contracts".into()],
                    non_goals: vec!["no planner".into()],
                    acceptance_criteria: vec!["collect returns typed result".into()],
                    contract_boundaries: vec!["inspect stays topology-only".into()],
                },
            }),
            latest_useful_output: Some(CollectedOutput {
                kind: CollectedOutputKind::AssistantText,
                content: "spec summary".into(),
                tool_name: None,
                tool_call_id: None,
            }),
        },
    }
}
