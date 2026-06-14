use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::ToolContext;
use exagent::registry::ToolRegistry;
use exagent::runtime::agent_profile::AgentToolPolicy;
use exagent::tools::write_file::WriteFileTool;
use exagent::types::ToolCall;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn registry_returns_error_result_for_unknown_tool() {
    let registry = ToolRegistry::new();
    let call = ToolCall {
        id: "call_1".into(),
        name: "does_not_exist".into(),
        arguments: json!({}),
        thought_signature: None,
    };

    let result = registry.execute(call, None).await;
    assert_eq!(result.tool_name, "does_not_exist");
    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("Unknown tool"));
}

#[tokio::test]
async fn registry_execute_denies_tool_blocked_by_agent_policy() {
    let dir = tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        thread_id: None,
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: AgentToolPolicy::read_only_basic_collaboration(),
        inbox: None,
        goal_api: None,
        memory_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_blocked".into(),
                name: "write_file".into(),
                arguments: json!({
                    "path": "blocked.txt",
                    "content": "should not write"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("denied by agent profile"));
    assert!(!dir.path().join("blocked.txt").exists());
}
