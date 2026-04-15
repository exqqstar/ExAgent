use std::sync::Arc;
use std::time::Duration;

use exagent::config::AgentConfig;
use exagent::events::{ExecOutputStream, RuntimeEventKind};
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::run_command::RunCommandTool;
use exagent::transcript::replay_session;
use exagent::types::{SessionId, ToolCall};
use serde_json::json;
use tempfile::tempdir;

fn test_context() -> (tempfile::TempDir, SessionId, ToolContext) {
    let dir = tempdir().unwrap();
    let session_id = SessionId::new("session_exec_1");
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
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
async fn persistent_exec_session_accepts_stdin_across_multiple_calls() {
    let (_dir, _session_id, ctx) = test_context();
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
            },
            Some(&ctx),
        )
        .await;

    let exec_session_id = started.meta.as_ref().unwrap()["exec_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(started.meta.as_ref().unwrap()["lifecycle"], "running");

    tokio::time::sleep(Duration::from_millis(100)).await;

    registry
        .execute(
            ToolCall {
                id: "call_write".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": exec_session_id,
                    "stdin": "phase2\n"
                }),
            },
            Some(&ctx),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let polled = registry
        .execute(
            ToolCall {
                id: "call_poll".into(),
                name: "run_command".into(),
                arguments: json!({
                    "exec_session_id": started.meta.unwrap()["exec_session_id"]
                }),
            },
            Some(&ctx),
        )
        .await;

    let meta = polled.meta.unwrap();
    assert_eq!(meta["lifecycle"], "running");
    assert!(meta["stdout"].as_str().unwrap().contains("ready"));
    assert!(meta["stdout"].as_str().unwrap().contains("echo:phase2"));
}

#[tokio::test]
async fn persistent_exec_session_streams_output_into_runtime_events() {
    let (dir, session_id, ctx) = test_context();
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);

    registry
        .execute(
            ToolCall {
                id: "call_stream".into(),
                name: "run_command".into(),
                arguments: json!({
                    "command": "printf 'stdout-line\\n'; printf 'stderr-line\\n' >&2; sleep 30",
                    "persistent": true
                }),
            },
            Some(&ctx),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let replay = replay_session(dir.path(), &session_id).unwrap();
    assert!(replay.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ExecOutput {
            stream: ExecOutputStream::Stdout,
            chunk,
            ..
        } if chunk.contains("stdout-line")
    )));
    assert!(replay.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ExecOutput {
            stream: ExecOutputStream::Stderr,
            chunk,
            ..
        } if chunk.contains("stderr-line")
    )));
}

#[tokio::test]
async fn persistent_exec_session_terminate_marks_session_closed() {
    let (_dir, _session_id, ctx) = test_context();
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
            },
            Some(&ctx),
        )
        .await;

    let meta = terminated.meta.unwrap();
    assert_eq!(meta["lifecycle"], "terminated");
    assert_eq!(meta["exit_code"], serde_json::Value::Null);
}
