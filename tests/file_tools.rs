use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::{
    apply_patch::ApplyPatchTool, read_file::ReadFileTool, search_files::SearchFilesTool,
    write_file::WriteFileTool,
};
use exagent::types::{ToolCall, ToolStatus};
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "notes.txt", "start_line": 2, "end_line": 3}),
                thought_signature: None,
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_2".into(),
                name: "write_file".into(),
                arguments: json!({"path": "nested/out.txt", "content": "hello"}),
                thought_signature: None,
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
async fn apply_patch_updates_existing_file_with_begin_patch_format() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ApplyPatchTool);

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
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_patch".into(),
                name: "apply_patch".into(),
                arguments: json!({
                    "patch": "*** Begin Patch\n*** Update File: notes.txt\n@@\n alpha\n-beta\n+delta\n gamma\n*** End Patch\n"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "alpha\ndelta\ngamma\n"
    );
    assert_eq!(result.meta.unwrap()["changed_files"][0], "notes.txt");
}

#[tokio::test]
async fn apply_patch_rejects_multi_file_patch_without_partial_mutation() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("first.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(dir.path().join("second.txt"), "one\ntwo\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ApplyPatchTool);

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
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_patch_atomic".into(),
                name: "apply_patch".into(),
                arguments: json!({
                    "patch": "*** Begin Patch\n*** Update File: first.txt\n@@\n alpha\n-beta\n+delta\n*** Update File: second.txt\n@@\n-missing\n+changed\n*** End Patch\n"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Error);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("first.txt")).unwrap(),
        "alpha\nbeta\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("second.txt")).unwrap(),
        "one\ntwo\n"
    );
}

#[tokio::test]
async fn apply_patch_rejects_move_to_existing_file_without_clobbering() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("source.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(dir.path().join("target.txt"), "keep me\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ApplyPatchTool);

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
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_patch_move_collision".into(),
                name: "apply_patch".into(),
                arguments: json!({
                    "patch": "*** Begin Patch\n*** Update File: source.txt\n*** Move to: target.txt\n@@\n alpha\n-beta\n+delta\n*** End Patch\n"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result.content.contains("Move target already exists"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source.txt")).unwrap(),
        "alpha\nbeta\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
        "keep me\n"
    );
}

#[tokio::test]
async fn search_files_returns_matching_lines_under_workspace() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/auth.rs"),
        "fn login() {}\nfn logout() {}\n",
    )
    .unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(SearchFilesTool);

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
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_search".into(),
                name: "search_files".into(),
                arguments: json!({
                    "query": "login",
                    "path": "src",
                    "max_results": 10
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("src/auth.rs:1"));
    assert!(result.content.contains("fn login()"));
    assert!(!result.content.contains("fn logout()"));
}

#[cfg(unix)]
#[tokio::test]
async fn search_files_skips_symlink_escape_outside_workspace() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "login secret").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        dir.path().join("secret-link.txt"),
    )
    .unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "login",
            "path": ".",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content, "No matches found");
}

#[tokio::test]
async fn search_files_skips_invalid_utf8_and_large_files() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("invalid.txt"), [0xff, b'l', b'o', b'g']).unwrap();
    std::fs::write(
        dir.path().join("large.txt"),
        format!("login {}\n", "x".repeat(1024 * 1024)),
    )
    .unwrap();
    std::fs::write(dir.path().join("valid.txt"), "login ok").unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "login",
            "path": ".",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("valid.txt:1: login ok"));
    assert!(!result.content.contains("invalid.txt"));
    assert!(!result.content.contains("large.txt"));
}

#[tokio::test]
async fn search_files_respects_max_results_and_truncates_long_lines() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("first.txt"),
        format!("login {}\n", "a".repeat(10_000)),
    )
    .unwrap();
    std::fs::write(dir.path().join("second.txt"), "login second\n").unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "login",
            "path": ".",
            "max_results": 1
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("first.txt:1: login"));
    assert!(result.content.contains("[line truncated]"));
    assert!(result.content.len() < 1_200);
    assert!(!result.content.contains("second.txt"));
}

#[tokio::test]
async fn search_files_caps_total_formatted_output() {
    let dir = tempdir().unwrap();
    for index in 0..80 {
        std::fs::write(
            dir.path().join(format!("match-{index:02}.txt")),
            format!("login {}\n", "x".repeat(500)),
        )
        .unwrap();
    }

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "login",
            "path": ".",
            "max_results": 80
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.len() <= 16 * 1024);
    assert!(result.content.contains("[output truncated]"));
    assert_eq!(result.meta.unwrap()["truncated"], true);
}

async fn execute_search_files(
    workspace_root: &std::path::Path,
    arguments: serde_json::Value,
) -> exagent::types::ToolResult {
    let mut registry = ToolRegistry::new();
    registry.register(SearchFilesTool);

    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: workspace_root.to_path_buf(),
            cwd: workspace_root.to_path_buf(),
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
    };

    registry
        .execute(
            ToolCall {
                id: "call_search".into(),
                name: "search_files".into(),
                arguments,
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_read".into(),
                name: "read_file".into(),
                arguments: json!({"path": file.display().to_string()}),
                thought_signature: None,
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
async fn read_file_accepts_absolute_path_that_normalizes_from_root_parent() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "inside").unwrap();
    let canonical_file = std::fs::canonicalize(&file).unwrap();
    let root_parent_path = format!("/..{}", canonical_file.display());

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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_root_parent_read".into(),
                name: "read_file".into(),
                arguments: json!({"path": root_parent_path}),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "success");
    assert_eq!(result.content, "inside");
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_write".into(),
                name: "write_file".into(),
                arguments: json!({"path": file.display().to_string(), "content": "hello"}),
                thought_signature: None,
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_3".into(),
                name: "read_file".into(),
                arguments: json!({"path": "../outside.txt"}),
                thought_signature: None,
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_absolute_escape".into(),
                name: "read_file".into(),
                arguments: json!({"path": file.display().to_string()}),
                thought_signature: None,
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
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        inbox: None,
        goal_api: None,
    };

    let result = registry
        .execute(
            ToolCall {
                id: "call_symlink_escape".into(),
                name: "read_file".into(),
                arguments: json!({"path": "link.txt"}),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("workspace"));
}
