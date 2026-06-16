use std::sync::Arc;

use exagent::config::AgentConfig;
use exagent::exec_session::ExecSessionManager;
use exagent::index_db::{IndexDb, ProjectUpsert};
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::runtime::agent_profile::AgentToolPolicy;
use exagent::runtime::memory::{MemoryRuntime, MemoryToolApi};
use exagent::state::memory::{MemoryEntryKind, MemorySaveInput, MemoryScope, MemorySearchQuery};
use exagent::tools::memory_forget::MemoryForgetTool;
use exagent::tools::memory_list::MemoryListTool;
use exagent::tools::memory_recall::MemoryRecallTool;
use exagent::tools::memory_save::MemorySaveTool;
use exagent::tools::memory_update::MemoryUpdateTool;
use exagent::tools::{ToolCapabilities, ToolHandler, ToolInvocation};
use exagent::types::{ThreadId, ToolCall, ToolStatus, TurnId};
use serde_json::json;

#[tokio::test]
async fn memory_save_creates_candidate_visible_to_list_but_not_recall() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));

    let save = MemorySaveTool;
    assert_eq!(
        save.capabilities(),
        ToolCapabilities {
            mutating: true,
            requires_approval: false,
            parallel_safe: false,
        }
    );
    let save_outcome = save
        .handle(
            invocation(
                "call_save",
                "memory_save",
                json!({
                    "scope": "project",
                    "kind": "fact",
                    "title": "Memory tools candidate gate",
                    "content": "Saved tool memory must wait for curation before recall.",
                    "files": ["src/tools/memory_save.rs"],
                    "concepts": ["candidate gate"]
                }),
            ),
            &ctx,
        )
        .await;

    assert_eq!(save_outcome.model_result.status, ToolStatus::Success);
    assert!(save_outcome
        .model_result
        .content
        .contains("pending curation"));
    let candidate_id = save_outcome.model_result.meta.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        save_outcome.model_result.meta.as_ref().unwrap()["status"],
        "candidate"
    );

    let recall_outcome = MemoryRecallTool
        .handle(
            invocation(
                "call_recall",
                "memory_recall",
                json!({ "query": "Saved tool memory", "scope": "project" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(recall_outcome.model_result.status, ToolStatus::Success);
    assert!(recall_outcome
        .model_result
        .content
        .contains("No memory hits"));

    let list_outcome = MemoryListTool
        .handle(
            invocation(
                "call_list",
                "memory_list",
                json!({ "query": "Saved tool memory", "scope": "project" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(list_outcome.model_result.status, ToolStatus::Success);
    assert!(list_outcome.model_result.content.contains(&candidate_id));
    assert!(list_outcome.model_result.content.contains("candidate"));
}

#[tokio::test]
async fn memory_save_records_server_side_source_refs_for_current_turn() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));
    let mut registry = ToolRegistry::new();
    registry.register(MemorySaveTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_save_source_refs".into(),
                name: "memory_save".into(),
                arguments: json!({
                    "scope": "project",
                    "kind": "architecture",
                    "title": "Tool-first memory provenance",
                    "content": "Memory candidates cite rollout refs captured by the server.",
                    "files": ["src/tools/memory_save.rs"],
                    "concepts": ["memory provenance"]
                }),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status, ToolStatus::Success);
    let candidate_id = result.meta.as_ref().unwrap()["id"].as_str().unwrap();
    let candidate = fixture
        .memory_api
        .runtime()
        .db()
        .memory_entry_for_tests(candidate_id)
        .await
        .unwrap();
    assert_eq!(
        candidate.status,
        exagent::state::memory::MemoryStatus::Candidate
    );
    assert_eq!(candidate.title, "Tool-first memory provenance");
    assert_eq!(
        candidate.content,
        "Memory candidates cite rollout refs captured by the server."
    );

    let value = serde_json::to_value(&candidate).unwrap();
    let refs = value["source_refs"].as_array().expect("source_refs array");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["thread_id"], "thread_memory_tools");
    assert_eq!(refs[0]["turn_id"], "turn_memory_tools");
    assert_eq!(refs[0]["tool_call_id"], "call_save_source_refs");
    assert_eq!(refs[0]["tool_invocation_id"], "inv_call_save_source_refs");
    assert!(value.get("source_observation_ids").is_none());
}

#[tokio::test]
async fn memory_save_skips_duplicate_candidate_from_same_rollout_ref_and_content() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));
    let mut registry = ToolRegistry::new();
    registry.register(MemorySaveTool);

    let args = json!({
        "scope": "project",
        "kind": "preference",
        "title": "Do not commit local instructions",
        "content": "AGENTS.md is local-only workspace guidance and must not be committed.",
        "concepts": ["local instructions"]
    });
    let first = registry
        .execute(
            ToolCall {
                id: "call_save_duplicate_a".into(),
                name: "memory_save".into(),
                arguments: args.clone(),
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;
    assert_eq!(first.status, ToolStatus::Success);
    assert_eq!(first.meta.as_ref().unwrap()["status"], "candidate");

    let second = registry
        .execute(
            ToolCall {
                id: "call_save_duplicate_b".into(),
                name: "memory_save".into(),
                arguments: args,
                thought_signature: None,
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(second.status, ToolStatus::Success);
    assert_eq!(second.meta.as_ref().unwrap()["status"], "skipped");
    assert_eq!(
        second.meta.as_ref().unwrap()["duplicate_of"],
        first.meta.as_ref().unwrap()["id"]
    );

    let list = MemoryListTool
        .handle(
            invocation(
                "call_list_duplicate",
                "memory_list",
                json!({ "query": "Do not commit local instructions", "scope": "project" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(list.model_result.status, ToolStatus::Success);
    let candidates = list.model_result.meta.as_ref().unwrap()["candidates"]
        .as_array()
        .unwrap();
    assert_eq!(candidates.len(), 1);
}

#[tokio::test]
async fn memory_save_rejects_project_id_and_global_scope() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));

    let unknown_field = MemorySaveTool
        .handle(
            invocation(
                "call_save_unknown",
                "memory_save",
                json!({
                    "scope": "project",
                    "kind": "fact",
                    "title": "Bad project id",
                    "content": "The model must not pass raw project ids.",
                    "project_id": "model_supplied_project_must_fail"
                }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(unknown_field.model_result.status, ToolStatus::Error);
    assert!(unknown_field.model_result.content.contains("project_id"));

    let global = MemorySaveTool
        .handle(
            invocation(
                "call_save_global",
                "memory_save",
                json!({
                    "scope": "global",
                    "kind": "fact",
                    "title": "Bad global candidate",
                    "content": "The model must not create global candidates."
                }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(global.model_result.status, ToolStatus::Error);
    assert!(global.model_result.content.contains("global memory"));
}

#[tokio::test]
async fn memory_forget_is_limited_to_derived_project_scope() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));
    let db = fixture.memory_api.runtime().db();
    assert_eq!(
        MemoryForgetTool.capabilities(),
        ToolCapabilities {
            mutating: true,
            requires_approval: false,
            parallel_safe: false,
        }
    );
    let other_workspace = fixture._dir.path().join("other_project");
    tokio::fs::create_dir_all(&other_workspace).await.unwrap();
    let other_project = db
        .upsert_project(ProjectUpsert {
            name: "Other".into(),
            path: other_workspace,
        })
        .await
        .unwrap();
    let other_entry = db
        .save_memory_entry_for_scope(
            Some(&other_project.id),
            None,
            MemorySaveInput {
                scope: MemoryScope::Project,
                kind: MemoryEntryKind::Fact,
                title: "Other project memory".into(),
                content: "This entry must not be deletable from the current project.".into(),
                files: vec!["src/tools/memory_forget.rs".into()],
                concepts: vec!["forget".into()],
                source_refs: vec![],
                pinned: false,
            },
            "test",
        )
        .await
        .unwrap();

    let outcome = MemoryForgetTool
        .handle(
            invocation(
                "call_forget_other",
                "memory_forget",
                json!({ "id": other_entry.id }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(outcome.model_result.status, ToolStatus::Error);

    let still_present = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(other_project.id),
            thread_id: None,
            query: "Other project memory".into(),
            mode: exagent::state::memory::MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: true,
        })
        .await
        .unwrap();
    assert_eq!(still_present.len(), 1);
}

#[tokio::test]
async fn memory_update_allows_pin_unpin_and_supersede_but_rejects_promote() {
    let fixture = fixture().await;
    let ctx = fixture.context(Some(fixture.memory_api.clone()));
    let db = fixture.memory_api.runtime().db();
    assert_eq!(
        MemoryUpdateTool.capabilities(),
        ToolCapabilities {
            mutating: true,
            requires_approval: false,
            parallel_safe: false,
        }
    );
    let old = db
        .save_memory_entry_for_scope(
            Some(&fixture.project_id),
            None,
            MemorySaveInput {
                scope: MemoryScope::Project,
                kind: MemoryEntryKind::Fact,
                title: "Tool update old policy".into(),
                content: "Tool update old content toololdgone should no longer be recalled.".into(),
                files: vec!["src/tools/memory_update.rs".into()],
                concepts: vec!["tool update old".into()],
                source_refs: vec![],
                pinned: false,
            },
            "test",
        )
        .await
        .unwrap();

    let pin_outcome = MemoryUpdateTool
        .handle(
            invocation(
                "call_pin",
                "memory_update",
                json!({ "id": old.id.clone(), "action": "pin" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(pin_outcome.model_result.status, ToolStatus::Success);
    assert_eq!(
        pin_outcome.model_result.meta.as_ref().unwrap()["pinned"],
        true
    );

    let unpin_outcome = MemoryUpdateTool
        .handle(
            invocation(
                "call_unpin",
                "memory_update",
                json!({ "id": old.id.clone(), "action": "unpin" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(unpin_outcome.model_result.status, ToolStatus::Success);
    assert_eq!(
        unpin_outcome.model_result.meta.as_ref().unwrap()["pinned"],
        false
    );

    let promote_outcome = MemoryUpdateTool
        .handle(
            invocation(
                "call_promote",
                "memory_update",
                json!({ "id": old.id.clone(), "action": "promote" }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(promote_outcome.model_result.status, ToolStatus::Error);
    assert!(promote_outcome
        .model_result
        .content
        .contains("unsupported for model actors"));

    let supersede_outcome = MemoryUpdateTool
        .handle(
            invocation(
                "call_supersede",
                "memory_update",
                json!({
                    "id": old.id.clone(),
                    "action": "supersede",
                    "kind": "fact",
                    "title": "Tool update new policy",
                    "content": "Tool update new content should be recalled.",
                    "files": ["src/tools/memory_update.rs"],
                    "concepts": ["tool update new"],
                    "pinned": true
                }),
            ),
            &ctx,
        )
        .await;
    assert_eq!(supersede_outcome.model_result.status, ToolStatus::Success);
    let new_id = supersede_outcome.model_result.meta.as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        supersede_outcome.model_result.meta.as_ref().unwrap()["supersedes_id"],
        old.id
    );
    assert_eq!(
        supersede_outcome.model_result.meta.as_ref().unwrap()["status"],
        "active"
    );

    let old_hits = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(fixture.project_id.clone()),
            thread_id: None,
            query: "toololdgone".into(),
            mode: exagent::state::memory::MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: true,
        })
        .await
        .unwrap();
    assert!(old_hits.is_empty());

    let new_hits = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(fixture.project_id.clone()),
            thread_id: None,
            query: "Tool update new policy".into(),
            mode: exagent::state::memory::MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: true,
        })
        .await
        .unwrap();
    assert_eq!(new_hits.len(), 1);
    assert_eq!(new_hits[0].source_id, new_id);

    let old_actions = db.memory_audit_actions_for_tests(&old.id).await.unwrap();
    assert!(old_actions.iter().any(|action| action == "pin"));
    assert!(old_actions.iter().any(|action| action == "unpin"));
    assert!(old_actions.iter().any(|action| action == "supersede_old"));
    let new_actions = db.memory_audit_actions_for_tests(&new_id).await.unwrap();
    assert!(new_actions.iter().any(|action| action == "supersede_new"));
}

#[tokio::test]
async fn memory_recall_respects_tool_context_budget() {
    let fixture = fixture().await;
    let mut ctx = fixture.context(Some(fixture.memory_api.clone()));
    ctx.config.memory_tool_context_max_chars = 180;
    let db = fixture.memory_api.runtime().db();
    db.save_memory_entry_for_scope(
        Some(&fixture.project_id),
        None,
        MemorySaveInput {
            scope: MemoryScope::Project,
            kind: MemoryEntryKind::Fact,
            title: "Budgeted memory recall".into(),
            content: "large ".repeat(200),
            files: vec!["src/tools/memory_recall.rs".into()],
            concepts: vec!["budget".into()],
            source_refs: vec![],
            pinned: false,
        },
        "test",
    )
    .await
    .unwrap();

    let outcome = MemoryRecallTool
        .handle(
            invocation(
                "call_recall_budget",
                "memory_recall",
                json!({ "query": "Budgeted memory recall", "scope": "project" }),
            ),
            &ctx,
        )
        .await;

    assert_eq!(outcome.model_result.status, ToolStatus::Success);
    assert!(outcome.model_result.content.len() <= ctx.config.memory_tool_context_max_chars);
    assert!(outcome.model_result.content.contains("[TRUNCATED]"));
}

#[tokio::test]
async fn memory_tools_error_when_memory_api_is_unavailable() {
    let fixture = fixture().await;
    let ctx = fixture.context(None);

    let outcome = MemoryRecallTool
        .handle(
            invocation(
                "call_recall",
                "memory_recall",
                json!({ "query": "anything" }),
            ),
            &ctx,
        )
        .await;

    assert_eq!(outcome.model_result.status, ToolStatus::Error);
    assert!(outcome
        .model_result
        .content
        .contains("memory API unavailable"));
}

async fn fixture() -> MemoryToolFixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("project");
    tokio::fs::create_dir_all(&workspace_root).await.unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    db.upsert_project(ProjectUpsert {
        name: "Project".into(),
        path: workspace_root.clone(),
    })
    .await
    .unwrap();
    let runtime = MemoryRuntime::new(db);
    let memory_api = Arc::new(MemoryToolApi::new(runtime));
    let project_id = memory_api
        .runtime()
        .resolve_project_id_cached(&workspace_root)
        .await
        .unwrap()
        .unwrap();
    MemoryToolFixture {
        _dir: dir,
        workspace_root,
        memory_api,
        project_id,
    }
}

struct MemoryToolFixture {
    _dir: tempfile::TempDir,
    workspace_root: std::path::PathBuf,
    memory_api: Arc<MemoryToolApi>,
    project_id: String,
}

impl MemoryToolFixture {
    fn context(&self, memory_api: Option<Arc<MemoryToolApi>>) -> ToolContext {
        ToolContext {
            config: AgentConfig {
                workspace_root: self.workspace_root.clone(),
                cwd: self.workspace_root.clone(),
                ..AgentConfig::default()
            },
            thread_id: Some(ThreadId::new("thread_memory_tools")),
            turn_id: Some(TurnId::new("turn_memory_tools")),
            tool_invocation_id: None,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            exec_output_sink: None,
            policy: Arc::new(PolicyManager::default()),
            agent_tool_policy: AgentToolPolicy::all(),
            inbox: None,
            goal_api: None,
            memory_api,
        }
    }
}

fn invocation(id: &str, name: &str, arguments: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        invocation_id: format!("inv_{id}"),
        call: ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            thought_signature: None,
        },
    }
}
