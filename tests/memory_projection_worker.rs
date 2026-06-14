use std::time::{Duration, Instant};

use exagent::index_db::{IndexDb, ProjectUpsert};
use exagent::runtime::memory::{MemoryProjectionRequest, MemoryRuntime};
use exagent::session::ThreadSource;
use exagent::state::memory::{MemoryRecallMode, MemoryScope, MemorySearchQuery, MemorySourceKind};
use exagent::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use exagent::types::{ConversationMessage, ThreadId, TurnId};

#[tokio::test]
async fn enqueue_projection_returns_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let runtime = MemoryRuntime::new(db);
    let thread_id = ThreadId::new("enqueue_projection_returns_immediately");
    let rollout_path = dir.path().join("rollout.jsonl");

    let started = Instant::now();
    runtime.enqueue_projection(MemoryProjectionRequest {
        workspace_root: dir.path().to_path_buf(),
        project_id: None,
        thread_id,
        rollout_path,
    });

    assert!(started.elapsed() < Duration::from_millis(20));
}

#[tokio::test]
async fn project_id_cache_does_not_pin_missing_projects() {
    let dir = tempfile::tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let runtime = MemoryRuntime::new(db.clone());
    let workspace_root = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace_root).await.unwrap();

    assert_eq!(
        runtime
            .resolve_project_id_cached(&workspace_root)
            .await
            .unwrap(),
        None
    );

    let project = db
        .upsert_project(ProjectUpsert {
            name: "Workspace".into(),
            path: workspace_root.clone(),
        })
        .await
        .unwrap();

    assert_eq!(
        runtime
            .resolve_project_id_cached(&workspace_root)
            .await
            .unwrap()
            .as_deref(),
        Some(project.id.as_str())
    );
}

#[tokio::test]
async fn incremental_projection_uses_cursor_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let runtime = MemoryRuntime::new(db.clone());
    let workspace_root = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace_root).await.unwrap();
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Workspace".into(),
            path: workspace_root.clone(),
        })
        .await
        .unwrap();
    let thread_id = ThreadId::new("incremental_projection_uses_cursor_and_is_idempotent");
    let paths = rollout_paths(&workspace_root, &thread_id);
    let store = RolloutStore::new(paths.rollout_path.clone());

    store
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: workspace_root.clone(),
                initial_cwd: workspace_root,
                permission_profile: Default::default(),
                thread_source: ThreadSource::User,
                lineage: None,
                created_at: "2026-06-14T00:00:00Z".into(),
            }),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("Always keep public docs concise and sanitized."),
            ),
        ])
        .await
        .unwrap();

    runtime
        .project_thread_incremental(Some(project.id.as_str()), &thread_id, store.path())
        .await
        .unwrap();
    let first_hits = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(project.id.clone()),
            thread_id: Some(thread_id.clone()),
            query: "public docs concise sanitized".into(),
            mode: MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: false,
            include_observations: true,
        })
        .await
        .unwrap();
    assert_eq!(first_hits.len(), 1);
    assert_eq!(first_hits[0].source, MemorySourceKind::Observation);

    runtime
        .project_thread_incremental(Some(project.id.as_str()), &thread_id, store.path())
        .await
        .unwrap();
    let second_hits = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(project.id),
            thread_id: Some(thread_id.clone()),
            query: "public docs concise sanitized".into(),
            mode: MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: false,
            include_observations: true,
        })
        .await
        .unwrap();

    assert_eq!(
        runtime
            .db()
            .memory_projection_start_index(&thread_id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(second_hits.len(), 1);
    assert_eq!(second_hits[0].source_id, first_hits[0].source_id);
}

#[tokio::test]
async fn incremental_projection_does_not_reparse_items_before_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let runtime = MemoryRuntime::new(db.clone());
    let workspace_root = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace_root).await.unwrap();
    let project = db
        .upsert_project(ProjectUpsert {
            name: "Workspace".into(),
            path: workspace_root.clone(),
        })
        .await
        .unwrap();
    let thread_id = ThreadId::new("incremental_projection_does_not_reparse_items_before_cursor");
    let paths = rollout_paths(&workspace_root, &thread_id);
    tokio::fs::create_dir_all(paths.thread_dir).await.unwrap();
    let new_item = serde_json::to_string(&RolloutItem::response_item_for_turn(
        TurnId::new("turn_after_cursor"),
        ConversationMessage::user("Always skip already projected rollout items."),
    ))
    .unwrap();
    tokio::fs::write(
        &paths.rollout_path,
        format!("not-json-before-cursor\n{new_item}\n"),
    )
    .await
    .unwrap();
    db.set_memory_projection_cursor(&thread_id, &paths.rollout_path, 1)
        .await
        .unwrap();

    runtime
        .project_thread_incremental(Some(project.id.as_str()), &thread_id, &paths.rollout_path)
        .await
        .unwrap();

    let hits = db
        .search_memory(MemorySearchQuery {
            scope: MemoryScope::Project,
            project_id: Some(project.id),
            thread_id: Some(thread_id.clone()),
            query: "skip already projected rollout items".into(),
            mode: MemoryRecallMode::ToolPull,
            limit: 10,
            include_entries: false,
            include_observations: true,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        runtime
            .db()
            .memory_projection_start_index(&thread_id)
            .await
            .unwrap(),
        2
    );
}
