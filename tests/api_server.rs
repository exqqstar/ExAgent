use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use exagent::api::{build_router, AgentRunResponse, AgentRunner};
use exagent::cli::{parse_cli_command, CliCommand};
use exagent::types::{AssistantTurn, SessionId, ToolCall};
use serde_json::{json, Value};
use tower::util::ServiceExt;

struct StubRunner {
    turn: AssistantTurn,
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
        assert_eq!(prompt, "continue phase2");
        assert_eq!(workspace_root, Some("."));
        assert_eq!(cwd, Some("."));
        assert_eq!(session_id.map(SessionId::as_str), Some("session_123"));

        Ok(AgentRunResponse {
            text: self.turn.text.clone(),
            tool_calls: self.turn.tool_calls.clone(),
            session_id: SessionId::new("session_123"),
            snapshot_path: ".exagent/sessions/session_123/snapshot.json".into(),
            events_path: ".exagent/sessions/session_123/events.jsonl".into(),
        })
    }
}

#[tokio::test]
async fn health_route_returns_ok() {
    let app = build_router(Arc::new(StubRunner {
        turn: AssistantTurn {
            text: Some("unused".into()),
            tool_calls: vec![],
        },
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
        turn: AssistantTurn {
            text: Some("done".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "Cargo.toml"}),
            }],
        },
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
