use std::sync::Arc;
use std::time::{Duration, Instant};

use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::exec_command::ExecCommandTool;
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::write_stdin::WriteStdinTool;
use exagent::types::{ThreadId, ToolCall};
use serde_json::json;
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, ThreadId, ToolContext) {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_exec_1");
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        thread_id: Some(thread_id.clone()),
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };
    (dir, thread_id, ctx)
}

#[tokio::test]
async fn persistent_exec_poll_projects_accumulated_output_and_reports_delta() {
    let (_dir, _thread_id, mut ctx) = test_context();
    ctx.config.max_output_bytes = 96;
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let started = registry
        .execute(
            ToolCall {
                id: "call_start_long".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "for i in $(seq 1 120); do printf 'line-%04d middle middle middle tail-%04d\\n' \"$i\" \"$i\"; done",
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(2);
    let first = loop {
        let polled = registry
            .execute(
                ToolCall {
                    id: "call_poll_long_1".into(),
                    name: "run_command".into(),
                    arguments: json!({
                        "exec_session_id": exec_session_id
                    }),
                    thought_signature: None,
                },
                Some(&ctx),
            )
            .await;
        let meta = polled.meta.as_ref().unwrap();
        if meta["stdout_bytes"].as_u64().unwrap_or_default() > 1_000 {
            break polled;
        }
        if Instant::now() >= deadline {
            break polled;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let first_meta = first.meta.as_ref().unwrap();
    let projected_stdout = first_meta["stdout"].as_str().unwrap();
    let stdout_delta = first_meta["stdout_delta"].as_str().unwrap();

    assert_eq!(
        first_meta["output_projection"]["strategy"],
        "head_tail_bytes"
    );
    assert!(first_meta["stdout_bytes"].as_u64().unwrap() > 1_000);
    assert_eq!(first_meta["stdout_truncated"], true);
    assert!(projected_stdout.as_bytes().len() <= ctx.config.max_output_bytes);
    assert!(stdout_delta.as_bytes().len() <= ctx.config.max_output_bytes);
    assert!(first.content.len() < 512);
    assert!(!projected_stdout.contains("line-0060 middle middle middle tail-0060"));

    let second = registry
        .execute(
            ToolCall {
                id: "call_poll_long_2".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;
    let second_meta = second.meta.as_ref().unwrap();

    assert_eq!(second_meta["stdout_delta"], "");
    assert_eq!(second_meta["stdout_delta_bytes"], 0);
    assert!(
        second_meta["stdout"].as_str().unwrap().as_bytes().len() <= ctx.config.max_output_bytes
    );
}

#[tokio::test]
async fn persistent_exec_session_accepts_stdin_across_multiple_calls() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let started = registry
        .execute(
            ToolCall {
                id: "call_start".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "printf 'ready\\n'; IFS= read -r line; printf 'echo:%s\\n' \"$line\"; sleep 30",
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(started.meta.as_ref().unwrap()["lifecycle"], "running");

    registry
        .execute(
            ToolCall {
                id: "call_write".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "stdin": "phase2\n"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let deadline = Instant::now() + Duration::from_secs(2);
    let meta = loop {
        let polled = registry
            .execute(
                ToolCall {
                    id: "call_poll".into(),
                    name: "run_command".into(),
                    arguments: json!({
                        "exec_session_id": exec_session_id
                    }),
                    thought_signature: None,
                },
                Some(&ctx),
            )
            .await;
        let meta = polled.meta.unwrap();
        let stdout = meta["stdout"].as_str().unwrap();
        if stdout.contains("ready") && stdout.contains("echo:phase2") {
            break meta;
        }
        if Instant::now() >= deadline {
            break meta;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(meta["lifecycle"], "running");
    assert!(meta["stdout"].as_str().unwrap().contains("ready"));
    assert!(meta["stdout"].as_str().unwrap().contains("echo:phase2"));
}

#[tokio::test]
async fn exec_command_and_write_stdin_split_persistent_interaction() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(ExecCommandTool);
    registry.register(WriteStdinTool);

    let started = registry
        .execute(
            ToolCall {
                id: "call_exec_start".into(),
                name: "exec_command".into(),
                arguments: json!({
                    "cmd": "printf 'ready\\n'; IFS= read -r line; printf 'echo:%s\\n' \"$line\"; sleep 30",
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(started.status.as_str(), "success");
    assert_eq!(started.meta.as_ref().unwrap()["persistent"], true);
    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let written = registry
        .execute(
            ToolCall {
                id: "call_exec_write".into(),
                name: "write_stdin".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "chars": "phase2\n"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;
    assert_eq!(written.status.as_str(), "success");

    let deadline = Instant::now() + Duration::from_secs(2);
    let meta = loop {
        let polled = registry
            .execute(
                ToolCall {
                    id: "call_exec_poll".into(),
                    name: "write_stdin".into(),
                    arguments: json!({
                        "exec_session_id": exec_session_id,
                        "chars": ""
                    }),
                    thought_signature: None,
                },
                Some(&ctx),
            )
            .await;
        let meta = polled.meta.unwrap();
        let stdout = meta["stdout"].as_str().unwrap();
        if stdout.contains("ready") && stdout.contains("echo:phase2") {
            break meta;
        }
        if Instant::now() >= deadline {
            break meta;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(meta["lifecycle"], "running");
    assert!(meta["stdout"].as_str().unwrap().contains("ready"));
    assert!(meta["stdout"].as_str().unwrap().contains("echo:phase2"));
}

#[tokio::test]
async fn persistent_exec_session_streams_output_without_legacy_event_log() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let started = registry
        .execute(
            ToolCall {
                id: "call_stream".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "printf 'stdout-line\\n'; printf 'stderr-line\\n' >&2; sleep 30",
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;
    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(2);
    let meta = loop {
        let polled = registry
            .execute(
                ToolCall {
                    id: "call_poll_stream".into(),
                    name: "run_command".into(),
                    arguments: json!({
                        "exec_session_id": exec_session_id
                    }),
                    thought_signature: None,
                },
                Some(&ctx),
            )
            .await;
        let meta = polled.meta.unwrap();
        let stdout = meta["stdout"].as_str().unwrap();
        let stderr = meta["stderr"].as_str().unwrap();
        if stdout.contains("stdout-line") && stderr.contains("stderr-line") {
            break meta;
        }
        if Instant::now() >= deadline {
            break meta;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert!(meta["stdout"].as_str().unwrap().contains("stdout-line"));
    assert!(meta["stderr"].as_str().unwrap().contains("stderr-line"));
}

#[tokio::test]
async fn persistent_exec_session_terminate_marks_session_closed() {
    let (_dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let started = registry
        .execute(
            ToolCall {
                id: "call_start".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "sleep 30",
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let terminated = registry
        .execute(
            ToolCall {
                id: "call_stop".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "terminate": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let meta = terminated.meta.unwrap();
    assert_eq!(meta["lifecycle"], "terminated");
    assert_eq!(meta["exit_code"], serde_json::Value::Null);
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_exec_session_terminate_kills_background_children() {
    let (dir, _thread_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    let pid_file = dir.path().join("persistent-child.pid");
    let command = format!("sleep 60 & echo $! > {}; sleep 60", pid_file.display());

    let started = registry
        .execute(
            ToolCall {
                id: "call_start_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": command,
                    "persistent": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(2);
    while !pid_file.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(pid_file.exists(), "persistent child pid should be written");

    let terminated = registry
        .execute(
            ToolCall {
                id: "call_terminate_group".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "terminate": true
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(terminated.status.as_str(), "success");
    assert_eq!(terminated.meta.as_ref().unwrap()["lifecycle"], "terminated");

    let child_pid = std::fs::read_to_string(pid_file).expect("child pid should be written");
    assert!(
        wait_until_pid_exits(child_pid.trim(), Duration::from_secs(2)),
        "persistent background child should be gone after terminate"
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
