use exagent::config::PermissionProfile;
use exagent::index_db::{IndexDb, ProjectUpsert, ThreadListFilter};
use exagent::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use exagent::types::{ConversationMessage, ThreadId, TurnId};
use tempfile::tempdir;

#[tokio::test]
async fn index_db_open_creates_schema() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("exagent.sqlite");

    let db = IndexDb::open(&db_path).await.unwrap();
    let version = db.schema_version().await.unwrap();

    assert_eq!(version, 2);
    assert!(db_path.exists());
}

#[tokio::test]
async fn index_db_open_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("exagent.sqlite");

    IndexDb::open(&db_path).await.unwrap();
    let db = IndexDb::open(&db_path).await.unwrap();

    assert_eq!(db.schema_version().await.unwrap(), 2);
}

#[tokio::test]
async fn project_registry_upserts_and_lists_projects_by_last_opened() {
    let dir = tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let alpha = dir.path().join("alpha");
    let beta = dir.path().join("beta");
    tokio::fs::create_dir_all(&alpha).await.unwrap();
    tokio::fs::create_dir_all(&beta).await.unwrap();

    let first = db
        .upsert_project(ProjectUpsert {
            name: "Alpha".into(),
            path: alpha.clone(),
        })
        .await
        .unwrap();
    let second = db
        .upsert_project(ProjectUpsert {
            name: "Beta".into(),
            path: beta.clone(),
        })
        .await
        .unwrap();

    db.touch_project(&first.id).await.unwrap();
    let projects = db.list_projects().await.unwrap();

    assert_eq!(projects[0].id, first.id);
    assert_eq!(projects[1].id, second.id);
    assert_eq!(projects[0].path, alpha.canonicalize().unwrap());
}

#[tokio::test]
async fn reindex_project_discovers_rollout_threads() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let project_row = db
        .upsert_project(ProjectUpsert {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();

    let thread_id = ThreadId::new("thread_reindex_1");
    let paths = rollout_paths(&project, &thread_id);
    RolloutStore::new(paths.rollout_path.clone())
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: project.clone(),
                initial_cwd: project.clone(),
                permission_profile: PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-06-01T00:00:00Z".into(),
            }),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("Design the desktop GUI"),
            ),
        ])
        .await
        .unwrap();

    let report = db.reindex_project(&project_row.id, &project).await.unwrap();
    let threads = db
        .list_threads(ThreadListFilter {
            project_id: project_row.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap();

    assert_eq!(report.indexed_threads, 1);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, thread_id);
    assert_eq!(threads[0].fallback_title, "Design the desktop GUI");
}

#[tokio::test]
async fn thread_metadata_rename_pin_archive_and_search_do_not_touch_rollout() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let project_row = db
        .upsert_project(ProjectUpsert {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();
    let thread_id = ThreadId::new("thread_metadata_1");
    let paths = rollout_paths(&project, &thread_id);
    RolloutStore::new(paths.rollout_path.clone())
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: project.clone(),
                initial_cwd: project.clone(),
                permission_profile: PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-06-01T00:00:00Z".into(),
            }),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("Searchable session title"),
            ),
        ])
        .await
        .unwrap();
    let before = tokio::fs::read_to_string(&paths.rollout_path)
        .await
        .unwrap();
    db.reindex_project(&project_row.id, &project).await.unwrap();

    db.rename_thread(&thread_id, "Custom Title").await.unwrap();
    db.set_thread_pinned(&thread_id, true).await.unwrap();
    db.archive_thread(&thread_id).await.unwrap();
    assert!(db
        .list_threads(ThreadListFilter {
            project_id: project_row.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap()
        .is_empty());

    db.unarchive_thread(&thread_id).await.unwrap();
    let search = db
        .list_threads(ThreadListFilter {
            project_id: project_row.id,
            include_archived: false,
            search: Some("Custom".into()),
        })
        .await
        .unwrap();
    let after = tokio::fs::read_to_string(&paths.rollout_path)
        .await
        .unwrap();

    assert_eq!(search.len(), 1);
    assert_eq!(search[0].user_title.as_deref(), Some("Custom Title"));
    assert!(search[0].pinned);
    assert_eq!(before, after);
}
