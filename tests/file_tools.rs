use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::tools::{
    apply_patch::ApplyPatchTool, list_dir::ListDirTool, read_file::ReadFileTool,
    search_files::SearchFilesTool, write_file::WriteFileTool,
};
use exagent::types::{ToolCall, ToolStatus};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn read_file_limits_to_requested_range() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "a\nb\nc\nd\n").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let ctx = tool_context(dir.path());

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
async fn read_file_accepts_absolute_path_under_configured_skill_root() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_path = skill_root.join("my-skill").join("SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(&skill_path, "skill body").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let ctx = tool_context_with_skill_roots(workspace.path(), vec![skill_root]);

    let result = registry
        .execute(
            ToolCall {
                id: "call_skill_read".into(),
                name: "read_file".into(),
                arguments: json!({"path": skill_path.display().to_string()}),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.content, "skill body");
}

#[tokio::test]
async fn read_file_rejects_path_outside_workspace_and_skill_roots() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let outside_dir = root_parent.path().join("outside");
    let outside_path = outside_dir.join("secret.txt");
    std::fs::create_dir_all(&skill_root).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();
    std::fs::write(&outside_path, "secret").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let ctx = tool_context_with_skill_roots(workspace.path(), vec![skill_root]);

    let result = registry
        .execute(
            ToolCall {
                id: "call_skill_read_escape".into(),
                name: "read_file".into(),
                arguments: json!({"path": outside_path.display().to_string()}),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result.content.contains("workspace"));
}

#[tokio::test]
async fn write_file_rejects_configured_skill_root_path() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_path = skill_root.join("my-skill").join("SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(&skill_path, "original").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);

    let ctx = tool_context_with_skill_roots(workspace.path(), vec![skill_root]);

    let result = registry
        .execute(
            ToolCall {
                id: "call_skill_write".into(),
                name: "write_file".into(),
                arguments: json!({
                    "path": skill_path.display().to_string(),
                    "content": "changed"
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Error);
    assert!(result.content.contains("workspace"));
    assert_eq!(std::fs::read_to_string(skill_path).unwrap(), "original");
}

#[tokio::test]
async fn write_file_creates_parent_directories() {
    let dir = tempdir().unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

#[tokio::test]
async fn search_files_supports_regex_query_and_reports_query_mode() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/tools.rs"),
        "fn run_command() {}\nfn read_file() {}\n",
    )
    .unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": r"fn \w+_command",
            "path": "src",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("src/tools.rs:1: fn run_command()"));
    assert!(!result.content.contains("read_file"));
    assert_eq!(result.meta.unwrap()["query_mode"], "regex");
}

#[tokio::test]
async fn search_files_invalid_regex_falls_back_to_literal() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("projection.rs"),
        "let value = outcome.meta[\"x\"](\n",
    )
    .unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "outcome.meta[\"x\"](",
            "path": ".",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result
        .content
        .contains("projection.rs:1: let value = outcome.meta[\"x\"]("));
    assert_eq!(result.meta.unwrap()["query_mode"], "literal");
}

#[tokio::test]
async fn search_files_supports_case_insensitive_query() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "Login Handler\n").unwrap();

    let result = execute_search_files(
        dir.path(),
        json!({
            "query": "login handler",
            "path": ".",
            "case_insensitive": true,
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("notes.txt:1: Login Handler"));
    assert_eq!(result.meta.unwrap()["case_insensitive"], true);
}

#[tokio::test]
async fn search_files_respects_gitignore_and_glob() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n*.txt\n").unwrap();
    std::fs::write(dir.path().join("target/generated.rs"), "ignored_marker\n").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "kept_marker\n").unwrap();
    std::fs::write(dir.path().join("main.rs"), "kept_marker\n").unwrap();

    let ignored_result = execute_search_files(
        dir.path(),
        json!({
            "query": "ignored_marker",
            "path": ".",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(ignored_result.status, ToolStatus::Success);
    assert_eq!(ignored_result.content, "No matches found");

    let glob_result = execute_search_files(
        dir.path(),
        json!({
            "query": "kept_marker",
            "path": ".",
            "glob": "*.rs",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(glob_result.status, ToolStatus::Success);
    assert!(glob_result.content.contains("main.rs:1: kept_marker"));
    assert!(!glob_result.content.contains("notes.txt"));
    assert_eq!(glob_result.meta.unwrap()["glob"], "*.rs");
}

#[tokio::test]
async fn search_files_accepts_configured_skill_root_path() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_path = skill_root.join("my-skill").join("SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(&skill_path, "alpha\nskill-marker line\n").unwrap();

    let result = execute_search_files_with_skill_roots(
        workspace.path(),
        vec![skill_root.clone()],
        json!({
            "query": "skill-marker",
            "path": skill_root.display().to_string(),
            "max_results": 10
        }),
    )
    .await;

    let canonical_skill_path = std::fs::canonicalize(skill_path).unwrap();
    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains(&format!(
        "{}:2: skill-marker line",
        canonical_skill_path.display()
    )));
}

#[tokio::test]
async fn search_files_glob_applies_to_configured_skill_root_display_path() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_rs = skill_root.join("my-skill").join("scripts").join("tool.rs");
    let skill_txt = skill_root.join("my-skill").join("notes.txt");
    std::fs::create_dir_all(skill_rs.parent().unwrap()).unwrap();
    std::fs::write(&skill_rs, "skill-marker\n").unwrap();
    std::fs::write(&skill_txt, "skill-marker\n").unwrap();

    let result = execute_search_files_with_skill_roots(
        workspace.path(),
        vec![skill_root.clone()],
        json!({
            "query": "skill-marker",
            "path": skill_root.display().to_string(),
            "glob": "**/*.rs",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains(&format!(
        "{}:1: skill-marker",
        std::fs::canonicalize(skill_rs).unwrap().display()
    )));
    assert!(!result.content.contains("notes.txt"));
}

#[tokio::test]
async fn search_files_workspace_query_does_not_include_skill_roots() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_path = skill_root.join("my-skill").join("SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::write(
        workspace.path().join("notes.txt"),
        "skill-marker workspace\n",
    )
    .unwrap();
    std::fs::write(&skill_path, "skill-marker skill\n").unwrap();

    let result = execute_search_files_with_skill_roots(
        workspace.path(),
        vec![skill_root],
        json!({
            "query": "skill-marker",
            "path": ".",
            "max_results": 10
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result
        .content
        .contains("notes.txt:1: skill-marker workspace"));
    assert!(!result.content.contains("skill-marker skill"));
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
    execute_search_files_with_skill_roots(workspace_root, Vec::new(), arguments).await
}

async fn execute_search_files_with_skill_roots(
    workspace_root: &std::path::Path,
    skills_user_roots: Vec<PathBuf>,
    arguments: serde_json::Value,
) -> exagent::types::ToolResult {
    let mut registry = ToolRegistry::new();
    registry.register(SearchFilesTool);

    let ctx = tool_context_with_skill_roots(workspace_root, skills_user_roots);

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

async fn execute_list_dir(
    workspace_root: &std::path::Path,
    arguments: serde_json::Value,
) -> exagent::types::ToolResult {
    execute_list_dir_with_skill_roots(workspace_root, Vec::new(), arguments).await
}

async fn execute_list_dir_with_skill_roots(
    workspace_root: &std::path::Path,
    skills_user_roots: Vec<PathBuf>,
    arguments: serde_json::Value,
) -> exagent::types::ToolResult {
    let mut registry = ToolRegistry::new();
    registry.register(ListDirTool);

    let ctx = tool_context_with_skill_roots(workspace_root, skills_user_roots);

    registry
        .execute(
            ToolCall {
                id: "call_list_dir".into(),
                name: "list_dir".into(),
                arguments,
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await
}

#[tokio::test]
async fn list_dir_default_depth_lists_children_and_grandchildren_only() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/nested/deeper")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "lib").unwrap();
    std::fs::write(dir.path().join("src/nested/mod.rs"), "mod").unwrap();
    std::fs::write(dir.path().join("src/nested/deeper/file.rs"), "deep").unwrap();

    let result = execute_list_dir(dir.path(), json!({ "path": "." })).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains("src/"));
    assert!(result.content.contains("src/lib.rs"));
    assert!(result.content.contains("src/nested/"));
    assert!(!result.content.contains("src/nested/mod.rs"));
    assert!(!result.content.contains("src/nested/deeper/"));
}

#[tokio::test]
async fn list_dir_respects_gitignore_glob_and_entry_cap() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.path().join("target/generated.rs"), "ignored").unwrap();
    std::fs::write(dir.path().join("a.rs"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();
    std::fs::write(dir.path().join("c.rs"), "c").unwrap();

    let result = execute_list_dir(
        dir.path(),
        json!({
            "path": ".",
            "glob": "*.rs",
            "depth": 1,
            "max_entries": 1
        }),
    )
    .await;

    assert_eq!(result.status, ToolStatus::Success);
    assert!(result.content.contains(".rs"));
    assert!(!result.content.contains("b.txt"));
    assert!(!result.content.contains("target"));
    assert!(result.content.contains("[output truncated]"));
    assert_eq!(result.meta.unwrap()["truncated"], true);
}

#[tokio::test]
async fn list_dir_accepts_configured_skill_root_and_rejects_other_absolute_paths() {
    let workspace = tempdir().unwrap();
    let root_parent = tempdir().unwrap();
    let skill_root = root_parent.path().join("skills");
    let skill_dir = skill_root.join("my-skill");
    let outside = root_parent.path().join("outside");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "skill").unwrap();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();

    let skill_result = execute_list_dir_with_skill_roots(
        workspace.path(),
        vec![skill_root.clone()],
        json!({
            "path": skill_root.display().to_string(),
            "depth": 2
        }),
    )
    .await;

    let canonical_skill_file = std::fs::canonicalize(skill_dir.join("SKILL.md")).unwrap();
    assert_eq!(skill_result.status, ToolStatus::Success);
    assert!(skill_result
        .content
        .contains(&canonical_skill_file.display().to_string()));

    let outside_result = execute_list_dir_with_skill_roots(
        workspace.path(),
        vec![skill_root],
        json!({
            "path": outside.display().to_string(),
            "depth": 1
        }),
    )
    .await;

    assert_eq!(outside_result.status, ToolStatus::Error);
    assert!(outside_result.content.contains("workspace"));
}

fn tool_context(workspace_root: &Path) -> ToolContext {
    tool_context_with_skill_roots(workspace_root, Vec::new())
}

fn tool_context_with_skill_roots(
    workspace_root: &Path,
    skills_user_roots: Vec<PathBuf>,
) -> ToolContext {
    ToolContext {
        config: AgentConfig {
            workspace_root: workspace_root.to_path_buf(),
            cwd: workspace_root.to_path_buf(),
            skills_user_roots,
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
    }
}

#[tokio::test]
async fn read_file_accepts_absolute_path_inside_workspace() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "inside").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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

    let ctx = tool_context(dir.path());

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
