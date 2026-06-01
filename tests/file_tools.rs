use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::{read_file::ReadFileTool, write_file::WriteFileTool};
use exagent::types::ToolCall;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn read_file_limits_to_requested_range() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "a\nb\nc\nd\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

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

    let result = registry
        .execute(
            ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "notes.txt", "start_line": 2, "end_line": 3}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.tool_call_id, "call_1");
    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.content, "b\nc");
}

#[tokio::test]
async fn write_file_creates_parent_directories() {
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
        exec_sessions: Arc::new(ExecSessionManager::default()),
        policy: Arc::new(PolicyManager::default()),
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_2".into(),
                name: "write_file".into(),
                arguments: json!({"path": "nested/out.txt", "content": "hello"}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.tool_call_id, "call_2");
    assert_eq!(result.status.as_str(), "success");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/out.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn read_file_accepts_absolute_path_inside_workspace() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "inside").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

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

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_read".into(),
                name: "read_file".into(),
                arguments: json!({"path": file.display().to_string()}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.content, "inside");
    let meta = result.meta.unwrap();
    assert_eq!(meta["was_absolute"], true);
    assert_eq!(meta["requested_path"], file.display().to_string());
    assert_eq!(
        meta["canonical_path"],
        std::fs::canonicalize(file).unwrap().display().to_string()
    );
}

#[tokio::test]
async fn write_file_accepts_absolute_missing_path_inside_workspace() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("nested").join("out.txt");

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
        exec_sessions: Arc::new(ExecSessionManager::default()),
        policy: Arc::new(PolicyManager::default()),
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_write".into(),
                name: "write_file".into(),
                arguments: json!({"path": file.display().to_string(), "content": "hello"}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    let meta = result.meta.unwrap();
    assert_eq!(meta["was_absolute"], true);
    assert_eq!(
        meta["canonical_path"],
        std::fs::canonicalize(&file).unwrap().display().to_string()
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "hello");
}

#[tokio::test]
async fn read_file_rejects_escape_outside_workspace() {
    let dir = tempdir().unwrap();
    let outside = dir.path().parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

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

    let result = registry
        .execute(
            ToolCall {
                id: "call_3".into(),
                name: "read_file".into(),
                arguments: json!({"path": "../outside.txt"}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.tool_call_id, "call_3");
    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("workspace"));
}

#[tokio::test]
async fn read_file_rejects_absolute_path_outside_workspace() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let file = outside.path().join("outside.txt");
    std::fs::write(&file, "secret").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

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

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_escape".into(),
                name: "read_file".into(),
                arguments: json!({"path": file.display().to_string()}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("workspace"));
}

#[cfg(unix)]
#[tokio::test]
async fn read_file_rejects_symlink_escape_outside_workspace() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, "secret").unwrap();
    std::os::unix::fs::symlink(&outside_file, dir.path().join("link.txt")).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

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

    let result = registry
        .execute(
            ToolCall {
                id: "call_symlink_escape".into(),
                name: "read_file".into(),
                arguments: json!({"path": "link.txt"}),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("workspace"));
}
