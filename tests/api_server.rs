use std::sync::Arc;
use std::sync::Mutex;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use exagent::api::{build_router, AgentRunResponse, AgentRunner, CollectResponse, InspectResponse};
use exagent::cli::{parse_cli_command, CliCommand};
use exagent::orchestration::{
    ChildLifecycleStatus, ChildSessionSummary, CollectedChildSession, CollectedOutput,
    CollectedOutputKind,
};
use exagent::result_contract::{
    StructuredResultPayload, StructuredSessionResult, STRUCTURED_RESULT_SCHEMA_VERSION,
};
use exagent::session::AgentRole;
use exagent::types::{SessionId, ToolCall, TurnId};
use serde_json::{json, Value};
use tower::util::ServiceExt;

struct StubRunner {
    response: AgentRunResponse,
    inspect_response: InspectResponse,
    collect_response: CollectResponse,
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl AgentRunner for StubRunner {
    async fn run(
        &self,
        prompt: &str,
        workspace_root: Option<&str>,
        cwd: Option<&str>,
        session_id: Option<&SessionId>,
    ) -> anyhow::Result<AgentRunResponse> {
        self.calls.lock().unwrap().push("run".into());
        assert_eq!(prompt, "continue phase2");
        assert_eq!(workspace_root, Some("."));
        assert_eq!(cwd, Some("."));
        assert_eq!(session_id.map(SessionId::as_str), Some("session_123"));

        Ok(self.response.clone())
    }

    async fn fork(
        &self,
        parent_session_id: &SessionId,
        agent_role: AgentRole,
        prompt: &str,
        workspace_root: Option<&str>,
        spawned_by_turn_id: Option<&TurnId>,
    ) -> anyhow::Result<AgentRunResponse> {
        self.calls.lock().unwrap().push("fork".into());
        assert_eq!(parent_session_id.as_str(), "session_123");
        assert_eq!(agent_role, AgentRole::Spec);
        assert_eq!(prompt, "draft goals");
        assert_eq!(workspace_root, Some("."));
        assert_eq!(spawned_by_turn_id.map(TurnId::as_str), Some("turn_1"));

        Ok(self.response.clone())
    }

    async fn inspect(
        &self,
        parent_session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> anyhow::Result<InspectResponse> {
        self.calls.lock().unwrap().push("inspect".into());
        assert_eq!(parent_session_id.as_str(), "session_parent");
        assert_eq!(workspace_root, Some("."));

        Ok(self.inspect_response.clone())
    }

    async fn collect(
        &self,
        session_id: &SessionId,
        workspace_root: Option<&str>,
    ) -> anyhow::Result<CollectResponse> {
        self.calls.lock().unwrap().push("collect".into());
        assert_eq!(session_id.as_str(), "session_child");
        assert_eq!(workspace_root, Some("."));

        Ok(self.collect_response.clone())
    }
}

struct ForkIgnoresCwdRunner {
    response: AgentRunResponse,
}

#[async_trait::async_trait]
impl AgentRunner for ForkIgnoresCwdRunner {
    async fn run(
        &self,
        _prompt: &str,
        _workspace_root: Option<&str>,
        _cwd: Option<&str>,
        _session_id: Option<&SessionId>,
    ) -> anyhow::Result<AgentRunResponse> {
        panic!("run should not be called in fork test");
    }

    async fn fork(
        &self,
        parent_session_id: &SessionId,
        agent_role: AgentRole,
        prompt: &str,
        workspace_root: Option<&str>,
        spawned_by_turn_id: Option<&TurnId>,
    ) -> anyhow::Result<AgentRunResponse> {
        assert_eq!(parent_session_id.as_str(), "session_123");
        assert_eq!(agent_role, AgentRole::Spec);
        assert_eq!(prompt, "draft goals");
        assert_eq!(workspace_root, Some("."));
        assert_eq!(spawned_by_turn_id.map(TurnId::as_str), Some("turn_1"));

        Ok(self.response.clone())
    }

    async fn inspect(
        &self,
        _parent_session_id: &SessionId,
        _workspace_root: Option<&str>,
    ) -> anyhow::Result<InspectResponse> {
        panic!("inspect should not be called in fork test");
    }

    async fn collect(
        &self,
        _session_id: &SessionId,
        _workspace_root: Option<&str>,
    ) -> anyhow::Result<CollectResponse> {
        panic!("collect should not be called in fork test");
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
