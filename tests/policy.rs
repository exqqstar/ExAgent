use std::sync::Arc;

use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::{PolicyManager, PolicyMode};
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::{SessionId, ToolCall};
use serde_json::json;
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, SessionId, ToolContext) {
    let dir = tempdir().unwrap();
    let session_id = SessionId::new("session_policy_1");
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        session_id: Some(session_id.clone()),
        turn_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        policy: Arc::new(PolicyManager::default()),
    };
    (dir, session_id, ctx)
}

#[tokio::test]
async fn safe_commands_execute_immediately_under_enforced_policy() {
    let (_dir, _session_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_safe".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "printf 'ok'" }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.meta.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn risky_commands_return_review_required_without_legacy_event_log() {
    let (dir, session_id, ctx) = test_context();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_risky".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "rm -rf scratch" }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "review_required");
    let meta = result.meta.unwrap();
    assert_eq!(meta["approval_status"], "pending");
    assert_eq!(meta["policy_decision"], "review_required");
    assert!(meta["approval_id"].as_str().is_some());
    assert!(dir.path().join("scratch").exists());

    let legacy_paths = exagent::transcript::session_paths(dir.path(), &session_id);
    assert!(!legacy_paths.events_path.exists());
}

#[tokio::test]
async fn approved_requests_execute_and_denied_requests_stop_execution() {
    let (dir, session_id, ctx) = test_context();
    std::fs::create_dir_all(dir.path().join("approved")).unwrap();
    std::fs::create_dir_all(dir.path().join("denied")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let approved_request = registry
        .execute(
            ToolCall {
                id: "call_approve_request".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "rm -rf approved" }),
            },
            Some(&ctx),
        )
        .await;
    let approved_id = approved_request.meta.unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

    let approved = registry
        .execute(
            ToolCall {
                id: "call_approve".into(),
                name: "run_command".into(),
                arguments: json!({
                    "approval_id": approved_id,
                    "decision": "approved"
                }),
            },
            Some(&ctx),
        )
        .await;
    assert_eq!(approved.status.as_str(), "success");
    assert!(!dir.path().join("approved").exists());

    let denied_request = registry
        .execute(
            ToolCall {
                id: "call_deny_request".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "rm -rf denied" }),
            },
            Some(&ctx),
        )
        .await;
    let denied_id = denied_request.meta.unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

    let denied = registry
        .execute(
            ToolCall {
                id: "call_deny".into(),
                name: "run_command".into(),
                arguments: json!({
                    "approval_id": denied_id,
                    "decision": "denied"
                }),
            },
            Some(&ctx),
        )
        .await;
    assert_eq!(denied.status.as_str(), "error");
    assert!(dir.path().join("denied").exists());

    let legacy_paths = exagent::transcript::session_paths(dir.path(), &session_id);
    assert!(!legacy_paths.events_path.exists());
}
