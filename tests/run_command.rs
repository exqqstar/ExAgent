use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::ToolCall;
use serde_json::json;
use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        thread_id: None,
        turn_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        policy: Arc::new(PolicyManager::default()),
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
async fn run_command_accepts_absolute_cwd_inside_workspace() {
    let (dir, ctx) = test_context();
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_cwd".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "pwd -P",
                    "cwd": nested.display().to_string()
                }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    let meta = result.meta.unwrap();
    assert_eq!(
        meta["stdout"].as_str().unwrap().trim(),
        std::fs::canonicalize(nested).unwrap().display().to_string()
    );
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

#[tokio::test]
async fn run_command_projects_long_output_with_head_and_tail_metadata() {
    let (_dir, mut ctx) = test_context();
    ctx.config.max_output_bytes = 64;
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_project_output".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaammmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz'"
                }),
            },
            Some(&ctx),
        )
        .await;

    let meta = result.meta.unwrap();
    let stdout = meta["stdout"].as_str().unwrap();
    assert_eq!(result.status.as_str(), "success");
    assert_eq!(meta["stdout_bytes"], 96);
    assert_eq!(meta["stderr_bytes"], 0);
    assert_eq!(meta["stdout_truncated"], true);
    assert_eq!(meta["stderr_truncated"], false);
    assert_eq!(meta["output_projection"]["strategy"], "head_tail_bytes");
    assert!(stdout.as_bytes().len() <= 64);
    assert!(stdout.contains("aaaaaaaaaaaaaaaa"));
    assert!(stdout.contains("zzzzzzzzzzzzzzzz"));
    assert!(!stdout.contains("mmmmmmmmmmmmmmmmmmmm"));
}

#[cfg(unix)]
#[tokio::test]
async fn run_command_timeout_kills_background_children() {
    let (dir, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let pid_file = dir.path().join("child.pid");
    let command = format!("sleep 60 & echo $! > {}; sleep 60", pid_file.display());

    let result = registry
        .execute(
            ToolCall {
                id: "call_timeout_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": command,
                    "timeout_secs": 1
                }),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert_eq!(result.meta.as_ref().unwrap()["timed_out"], true);

    let child_pid = std::fs::read_to_string(pid_file).expect("child pid should be written");
    assert!(
        wait_until_pid_exits(child_pid.trim(), Duration::from_secs(2)),
        "background child should be gone after timeout"
    );
}

#[cfg(unix)]
fn wait_until_pid_exits(pid: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(pid)
            .status()
            .expect("check process status");
        if !status.success() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
