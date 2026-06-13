use std::sync::Arc;

use axum::http::header::CONTENT_TYPE;
use axum::response::Redirect;
use axum::{routing::get, Router};
use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::{PolicyManager, PolicyMode};
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::web_fetch::WebFetchTool;
use exagent::tools::{ToolInvocation, ToolRuntimeEffect};
use exagent::types::{ThreadId, ToolCall, ToolStatus};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn web_fetch_rejects_bad_scheme_without_approval() {
    let (_dir, ctx) = tool_context(PolicyMode::Enforced);

    let outcome = execute_web_fetch_outcome(
        &ctx,
        json!({
            "url": "file:///etc/hosts"
        }),
    )
    .await;

    assert_eq!(outcome.model_result.status, ToolStatus::Error);
    assert!(outcome.model_result.content.contains("http or https"));
    assert!(ctx.policy.list_pending().await.is_empty());
}

#[tokio::test]
async fn web_fetch_enforced_policy_returns_review_required_without_network_io() {
    let (_dir, ctx) = tool_context(PolicyMode::Enforced);

    let outcome = execute_web_fetch_outcome(
        &ctx,
        json!({
            "url": "https://example.com/docs",
            "timeout_secs": 7
        }),
    )
    .await;

    assert_eq!(outcome.model_result.status, ToolStatus::ReviewRequired);
    let meta = outcome.model_result.meta.unwrap();
    assert_eq!(meta["approval_status"], "pending");
    assert_eq!(meta["policy_decision"], "review_required");
    assert_eq!(meta["url"], "https://example.com/docs");
    assert!(meta["approval_id"].as_str().is_some());
    assert!(outcome.effects.iter().any(|effect| matches!(
        effect,
        ToolRuntimeEffect::ApprovalRequested {
            tool_name,
            checkpoint_id,
            command: Some(command),
            ..
        } if tool_name == "web_fetch"
            && checkpoint_id.is_none()
            && command.command == "https://example.com/docs"
            && command.timeout_secs == Some(7)
            && !command.persistent
    )));
}

#[tokio::test]
async fn web_fetch_denied_approval_stops_without_fetching() {
    let (_dir, ctx) = tool_context(PolicyMode::Enforced);
    let request = execute_web_fetch_outcome(
        &ctx,
        json!({
            "url": "https://example.com/deny"
        }),
    )
    .await;
    let approval_id = request.model_result.meta.unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

    let denied = execute_web_fetch_outcome(
        &ctx,
        json!({
            "approval_id": approval_id,
            "decision": "denied"
        }),
    )
    .await;

    assert_eq!(denied.model_result.status, ToolStatus::Error);
    assert_eq!(denied.model_result.content, "Approval denied");
    assert!(denied
        .effects
        .iter()
        .any(|effect| { matches!(effect, ToolRuntimeEffect::ApprovalDenied { .. }) }));
    let meta = denied.model_result.meta.unwrap();
    assert_eq!(meta["approval_status"], "denied");
    assert_eq!(meta["policy_decision"], "deny");
    assert!(ctx.policy.list_pending().await.is_empty());
}

#[tokio::test]
async fn web_fetch_policy_off_fetches_html_as_markdown() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let base_url = spawn_server(
        Router::new().route(
            "/html",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body><h1>Hello</h1><a href=\"https://example.com/docs\">docs</a></body></html>",
                )
            }),
        ),
    )
    .await;

    let result = execute_web_fetch(&ctx, json!({ "url": format!("{base_url}/html") })).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("# Hello"));
    assert!(result.content.contains("[docs](https://example.com/docs)"));
    let meta = result.meta.unwrap();
    assert_eq!(meta["http_status"], 200);
    assert_eq!(meta["content_type"], "text/html; charset=utf-8");
    assert_eq!(meta["body_truncated"], false);
    assert_eq!(meta["output_truncated"], false);
}

#[tokio::test]
async fn web_fetch_returns_json_raw_text() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let base_url = spawn_server(Router::new().route(
        "/json",
        get(|| async { ([(CONTENT_TYPE, "application/json")], "{\"ok\":true}") }),
    ))
    .await;

    let result = execute_web_fetch(&ctx, json!({ "url": format!("{base_url}/json") })).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content, "{\"ok\":true}");
    assert_eq!(result.meta.unwrap()["content_type"], "application/json");
}

#[tokio::test]
async fn web_fetch_reports_redirect_final_url() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let base_url = spawn_server(
        Router::new()
            .route("/redirect", get(|| async { Redirect::temporary("/final") }))
            .route(
                "/final",
                get(|| async { ([(CONTENT_TYPE, "text/plain")], "final body") }),
            ),
    )
    .await;

    let result = execute_web_fetch(&ctx, json!({ "url": format!("{base_url}/redirect") })).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content, "final body");
    assert!(result.meta.unwrap()["final_url"]
        .as_str()
        .unwrap()
        .ends_with("/final"));
}

#[tokio::test]
async fn web_fetch_rejects_cross_origin_redirects() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let final_url = spawn_server(Router::new().route(
        "/metadata",
        get(|| async { ([(CONTENT_TYPE, "text/plain")], "internal metadata") }),
    ))
    .await;
    let redirect_target = final_url.clone();
    let redirect_url = spawn_server(Router::new().route(
        "/redirect",
        get(move || {
            let redirect_target = redirect_target.clone();
            async move { Redirect::temporary(&redirect_target) }
        }),
    ))
    .await;

    let result =
        execute_web_fetch(&ctx, json!({ "url": format!("{redirect_url}/redirect") })).await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result.content.contains("cross-origin redirect"));
    assert!(result.content.contains(&final_url));
}

#[tokio::test]
async fn web_fetch_caps_large_response_bodies_while_streaming() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let body = Arc::new("a".repeat(2 * 1024 * 1024 + 512));
    let route_body = body.clone();
    let base_url = spawn_server(Router::new().route(
        "/large",
        get(move || {
            let body = route_body.clone();
            async move { ([(CONTENT_TYPE, "text/plain")], body.as_str().to_string()) }
        }),
    ))
    .await;

    let result = execute_web_fetch(&ctx, json!({ "url": format!("{base_url}/large") })).await;

    assert_eq!(result.status, ToolStatus::Success);
    let meta = result.meta.unwrap();
    assert_eq!(meta["body_bytes"], 2 * 1024 * 1024);
    assert_eq!(meta["body_truncated"], true);
    assert_eq!(meta["output_truncated"], true);
    assert!(result.content.contains("[output truncated]"));
}

#[tokio::test]
async fn web_fetch_rejects_unsupported_content_type() {
    let (_dir, ctx) = tool_context(PolicyMode::Off);
    let base_url = spawn_server(Router::new().route(
        "/image",
        get(|| async { ([(CONTENT_TYPE, "image/png")], vec![0_u8, 1, 2, 3]) }),
    ))
    .await;

    let result = execute_web_fetch(&ctx, json!({ "url": format!("{base_url}/image") })).await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result
        .content
        .contains("unsupported content type: image/png"));
}

async fn execute_web_fetch(
    ctx: &ToolContext,
    arguments: serde_json::Value,
) -> exagent::types::ToolResult {
    execute_web_fetch_outcome(ctx, arguments).await.model_result
}

async fn execute_web_fetch_outcome(
    ctx: &ToolContext,
    arguments: serde_json::Value,
) -> exagent::tools::ToolOutcome {
    let mut registry = ToolRegistry::new();
    registry.register(WebFetchTool);

    registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_call_web_fetch".into(),
                call: ToolCall {
                    id: "call_web_fetch".into(),
                    name: "web_fetch".into(),
                    arguments,
                    thought_signature: None,
                },
            },
            ctx,
        )
        .await
}

fn tool_context(policy_mode: PolicyMode) -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_web_fetch");
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode,
            ..AgentConfig::default()
        },
        thread_id: Some(thread_id),
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };
    (dir, ctx)
}

async fn spawn_server(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}
