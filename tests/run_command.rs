use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::{PolicyManager, PolicyMode};
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::{ToolInvocation, ToolRuntimeEffect};
use exagent::types::{ThreadId, ToolCall};
use serde_json::json;
use std::path::Path;
use std::process::Command;
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
        memory_api: None,
    };
    (dir, ctx)
}

fn approval_test_context() -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        thread_id: Some(ThreadId::new("thread_run_command_approval")),
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
        memory_api: None,
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
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.tool_call_id, "call_1");
    assert_eq!(result.status.as_str(), "success");
    let meta = result.meta.unwrap();
    assert_eq!(meta["exit_code"], 0);
    assert_eq!(meta["permission_profile"], "full_access");
    assert_eq!(meta["filesystem_sandbox"], "none");
    assert_eq!(meta["network_sandbox"], "none");
    assert_eq!(meta["env_isolation"], "none");
    assert!(result.content.contains("hello"));
}

#[tokio::test]
async fn run_command_approval_is_typed_effect() {
    let (_dir, ctx) = approval_test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_approval".to_string(),
                call: ToolCall {
                    id: "call_approval".into(),
                    name: "run_command".into(),
                    arguments: json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    assert!(outcome.effects.iter().any(|effect| {
        matches!(effect, ToolRuntimeEffect::ApprovalRequested { tool_name, .. } if tool_name == "run_command")
    }));
}

#[tokio::test]
async fn run_command_pending_approval_in_git_workspace_includes_checkpoint_id() {
    let (dir, ctx) = approval_test_context();
    init_repo(dir.path());
    std::fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
    git(dir.path(), ["add", "tracked.txt"]);
    git(dir.path(), ["commit", "-m", "initial"]);
    std::fs::write(dir.path().join("tracked.txt"), "dirty\n").unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_git_checkpoint".to_string(),
                call: ToolCall {
                    id: "call_git_checkpoint".into(),
                    name: "run_command".into(),
                    arguments: json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    let meta = outcome.model_result.meta.as_ref().unwrap();
    let checkpoint_id = meta["checkpoint_id"]
        .as_str()
        .expect("git workspace approval should include checkpoint id");
    assert!(!checkpoint_id.is_empty());

    let pending = ctx.policy.list_pending().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].checkpoint_id.as_deref(), Some(checkpoint_id));
    assert!(outcome.effects.iter().any(|effect| {
        matches!(
            effect,
            ToolRuntimeEffect::ApprovalRequested {
                checkpoint_id: Some(effect_checkpoint_id),
                ..
            } if effect_checkpoint_id == checkpoint_id
        )
    }));
}

#[tokio::test]
async fn run_command_checkpoint_uses_workspace_root_not_command_cwd() {
    let (dir, ctx) = approval_test_context();
    init_repo(dir.path());
    std::fs::write(dir.path().join("root_tracked.txt"), "base\n").unwrap();
    git(dir.path(), ["add", "root_tracked.txt"]);
    git(dir.path(), ["commit", "-m", "initial"]);
    std::fs::write(dir.path().join("root_tracked.txt"), "dirty outside cwd\n").unwrap();
    std::fs::write(
        dir.path().join("root_untracked.txt"),
        "untracked outside cwd\n",
    )
    .unwrap();
    let command_cwd = dir.path().join("subdir");
    std::fs::create_dir_all(command_cwd.join("scratch")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_workspace_root_checkpoint".to_string(),
                call: ToolCall {
                    id: "call_workspace_root_checkpoint".into(),
                    name: "run_command".into(),
                    arguments: json!({
                        "command": "rm -rf scratch",
                        "cwd": command_cwd.display().to_string()
                    }),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    let meta = outcome.model_result.meta.as_ref().unwrap();
    let checkpoint_id = meta["checkpoint_id"]
        .as_str()
        .expect("git workspace approval should include checkpoint id");
    assert_eq!(
        git_stdout(
            dir.path(),
            ["show", &format!("{checkpoint_id}:root_tracked.txt")]
        ),
        "dirty outside cwd\n"
    );
    assert_eq!(
        git_stdout(
            dir.path(),
            ["show", &format!("{checkpoint_id}:root_untracked.txt")]
        ),
        "untracked outside cwd\n"
    );
}

#[tokio::test]
async fn run_command_pending_approval_without_git_workspace_has_no_checkpoint_id() {
    let (_dir, ctx) = approval_test_context();

    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_no_git_checkpoint".to_string(),
                call: ToolCall {
                    id: "call_no_git_checkpoint".into(),
                    name: "run_command".into(),
                    arguments: json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    let meta = outcome.model_result.meta.as_ref().unwrap();
    assert!(meta.get("checkpoint_id").is_none());

    let pending = ctx.policy.list_pending().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].checkpoint_id, None);
    assert!(outcome.effects.iter().any(|effect| {
        matches!(
            effect,
            ToolRuntimeEffect::ApprovalRequested {
                checkpoint_id: None,
                ..
            }
        )
    }));
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
                thought_signature: None,
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
                thought_signature: None,
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
                thought_signature: None,
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
                thought_signature: None,
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
                thought_signature: None,
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

fn init_repo(path: &Path) {
    git(path, ["init"]);
    git(path, ["config", "user.name", "ExAgent Test"]);
    git(
        path,
        ["config", "user.email", "exagent-test@example.invalid"],
    );
}

fn git<const N: usize>(cwd: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
