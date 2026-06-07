use std::sync::Arc;

use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::{PolicyManager, PolicyMode};
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::{ThreadId, ToolCall};
use serde_json::json;
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, ThreadId, ToolContext) {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_policy_1");
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        thread_id: Some(thread_id.clone()),
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        mailbox_rx: None,
        goal_api: None,
    };
    (dir, thread_id, ctx)
}

#[tokio::test]
async fn safe_commands_execute_immediately_under_enforced_policy() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_safe".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "printf 'ok'" }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.meta.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn risky_commands_return_review_required_without_legacy_event_log() {
    let (dir, _thread_id, ctx) = test_context();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_risky".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "rm -rf scratch" }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "review_required");
    let meta = result.meta.unwrap();
    assert_eq!(meta["approval_status"], "pending");
    assert_eq!(meta["policy_decision"], "review_required");
    assert_eq!(meta["permission_profile"], "full_access");
    assert_eq!(meta["filesystem_sandbox"], "none");
    assert_eq!(meta["network_sandbox"], "none");
    assert_eq!(meta["env_isolation"], "none");
    assert!(meta["approval_id"].as_str().is_some());
    assert!(dir.path().join("scratch").exists());
}

#[tokio::test]
async fn denied_commands_include_permission_profile_metadata() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_denied".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "mkfs /dev/fake" }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    let meta = result.meta.unwrap();
    assert_eq!(meta["approval_status"], "denied");
    assert_eq!(meta["policy_decision"], "deny");
    assert_eq!(meta["permission_profile"], "full_access");
    assert_eq!(meta["filesystem_sandbox"], "none");
    assert_eq!(meta["network_sandbox"], "none");
    assert_eq!(meta["env_isolation"], "none");
}

#[tokio::test]
async fn approved_requests_execute_and_denied_requests_stop_execution() {
    let (dir, _thread_id, ctx) = test_context();
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
                thought_signature: None,
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
                thought_signature: None,
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
                thought_signature: None,
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
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;
    assert_eq!(denied.status.as_str(), "error");
    let denied_meta = denied.meta.unwrap();
    assert_eq!(denied_meta["permission_profile"], "full_access");
    assert_eq!(denied_meta["filesystem_sandbox"], "none");
    assert_eq!(denied_meta["network_sandbox"], "none");
    assert_eq!(denied_meta["env_isolation"], "none");
    assert!(dir.path().join("denied").exists());
}
