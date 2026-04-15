use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use exagent::api::{build_router, AgentRunner};
use exagent::types::{AssistantTurn, ToolCall};
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
        _workspace_root: Option<&str>,
        _cwd: Option<&str>,
    ) -> anyhow::Result<exagent::api::AgentRunResponse> {
        Ok(exagent::api::AgentRunResponse {
            text: self.turn.text.clone(),
            tool_calls: self.turn.tool_calls.clone(),
            transcript_path: format!(".exagent/runs/{}.jsonl", prompt),
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
async fn run_route_returns_final_turn_and_request_transcript() {
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
                        "prompt": "task-1",
                        "workspace_root": ".",
                        "cwd": "."
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
            "transcript_path": ".exagent/runs/task-1.jsonl"
        })
    );
}
