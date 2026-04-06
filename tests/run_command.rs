use exagent::config::AgentConfig;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::ToolCall;
use serde_json::json;
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
    };
    (dir, ctx)
}

#[tokio::test]
async fn run_command_captures_stdout_and_exit_code() {
    let (_dir, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_1".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "printf 'hello'" }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.tool_call_id, "call_1");
    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.meta.unwrap()["exit_code"], 0);
    assert!(result.content.contains("hello"));
}

#[tokio::test]
async fn run_command_returns_error_status_on_non_zero_exit() {
    let (_dir, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_2".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "printf 'nope' >&2; exit 7" }),
            },
            Some(&ctx),
        )
        .await;

    let meta = result.meta.unwrap();
    assert_eq!(result.status.as_str(), "error");
    assert_eq!(meta["exit_code"], 7);
    assert_eq!(meta["timed_out"], false);
    assert!(meta["stderr"].as_str().unwrap().contains("nope"));
}

#[tokio::test]
async fn run_command_times_out_long_running_process() {
    let (_dir, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_3".into(),
                name: "run_command".into(),
                arguments: json!({ "command": "sleep 2", "timeout_secs": 1 }),
            },
            Some(&ctx),
        )
        .await;

    let meta = result.meta.unwrap();
    assert_eq!(result.status.as_str(), "error");
    assert_eq!(meta["timed_out"], true);
    assert!(result.content.contains("timed out"));
}
