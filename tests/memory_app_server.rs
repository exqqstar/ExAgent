use exagent::app_server::desktop_facade::{DesktopFacade, NewProjectRequest};
use exagent::app_server::protocol::{
    BoundaryCapability, BoundaryOp, BoundaryOpResponse, InitializeParams, MemoryListArchivedParams,
    MemoryUpdateAction, MemoryUpdateParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::index_db::IndexDb;
use exagent::resolver::EnvModelResolver;
use exagent::state::memory::{MemoryEntryKind, MemorySaveInput, MemoryScope};
use std::sync::Arc;

#[tokio::test]
async fn initialize_exposes_memory_capability() {
    let service = AppServerService::with_config(AgentConfig::default());

    let response = service
        .submit_boundary_op(BoundaryOp::Initialize(InitializeParams {}))
        .await
        .unwrap();

    let BoundaryOpResponse::Initialized(initialized) = response else {
        panic!("initialize returned unexpected response");
    };
    assert!(initialized
        .supported_ops
        .contains(&BoundaryCapability::MemorySearch));
    assert!(initialized
        .supported_ops
        .contains(&BoundaryCapability::MemoryPromote));
    assert!(initialized
        .supported_ops
        .contains(&BoundaryCapability::MemoryListArchived));
}

#[tokio::test]
async fn desktop_memory_search_derives_scope_from_project_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_config_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(EnvModelResolver),
        db.clone(),
    );
    let facade = DesktopFacade::new(service, db.clone());
    let project = facade
        .add_project(NewProjectRequest {
            name: "Memory App Server".into(),
            path: workspace.clone(),
        })
        .await
        .unwrap();
    let derived_project_id = db
        .project_id_for_existing_path(&workspace)
        .await
        .unwrap()
        .unwrap();

    db.save_memory_entry_for_scope(
        Some(&derived_project_id),
        None,
        MemorySaveInput {
            scope: MemoryScope::Project,
            kind: MemoryEntryKind::Fact,
            title: "Derived workspace memory".into(),
            content: "Desktop memory search must derive scope from workspace root.".into(),
            files: vec!["src/app_server/desktop_facade.rs".into()],
            concepts: vec!["app-server-memory".into()],
            source_refs: vec![],
            pinned: false,
        },
        "test",
    )
    .await
    .unwrap();

    let response = facade
        .memory_search(&project.id, None, "Derived workspace memory", 10)
        .await
        .unwrap();

    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].title, "Derived workspace memory");
    assert_eq!(response.hits[0].scope, "project");
}

#[tokio::test]
async fn desktop_memory_update_is_scoped_and_supersedes_active_entries() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_config_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(EnvModelResolver),
        db.clone(),
    );
    let facade = DesktopFacade::new(service, db.clone());
    let project = facade
        .add_project(NewProjectRequest {
            name: "Memory Update".into(),
            path: workspace.clone(),
        })
        .await
        .unwrap();
    let derived_project_id = db
        .project_id_for_existing_path(&workspace)
        .await
        .unwrap()
        .unwrap();
    let old = db
        .save_memory_entry_for_scope(
            Some(&derived_project_id),
            None,
            MemorySaveInput {
                scope: MemoryScope::Project,
                kind: MemoryEntryKind::Fact,
                title: "Desktop old memory".into(),
                content: "appserveroldgone should disappear after supersession.".into(),
                files: vec!["src/app_server/request_processors/memory_processor.rs".into()],
                concepts: vec!["desktop memory update".into()],
                source_refs: vec![],
                pinned: false,
            },
            "test",
        )
        .await
        .unwrap();

    let pinned = facade
        .memory_update(
            &project.id,
            MemoryUpdateParams {
                workspace_root: None,
                entry_id: old.id.clone(),
                action: MemoryUpdateAction::Pin,
                scope: None,
                kind: None,
                title: None,
                content: None,
                files: None,
                concepts: None,
                pinned: None,
            },
        )
        .await
        .unwrap();
    assert!(pinned.entry.pinned);

    let superseded = facade
        .memory_update(
            &project.id,
            MemoryUpdateParams {
                workspace_root: None,
                entry_id: old.id.clone(),
                action: MemoryUpdateAction::Supersede,
                scope: None,
                kind: Some("fact".into()),
                title: Some("Desktop new memory".into()),
                content: Some("appservernewkept should be recalled after supersession.".into()),
                files: Some(vec![
                    "src/app_server/request_processors/memory_processor.rs".into(),
                ]),
                concepts: Some(vec!["desktop memory update".into()]),
                pinned: Some(true),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        superseded.entry.supersedes_id.as_deref(),
        Some(old.id.as_str())
    );
    assert!(superseded.entry.pinned);

    let old_hits = facade
        .memory_search(&project.id, None, "appserveroldgone", 10)
        .await
        .unwrap();
    assert!(old_hits.hits.is_empty());

    let new_hits = facade
        .memory_search(&project.id, None, "appservernewkept", 10)
        .await
        .unwrap();
    assert_eq!(new_hits.hits.len(), 1);
    assert_eq!(new_hits.hits[0].id, superseded.entry.id);
}

#[tokio::test]
async fn desktop_memory_archive_is_reversible_and_excluded_from_active_search() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let db = IndexDb::open(dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_config_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(EnvModelResolver),
        db.clone(),
    );
    let facade = DesktopFacade::new(service, db.clone());
    let project = facade
        .add_project(NewProjectRequest {
            name: "Memory Archive".into(),
            path: workspace.clone(),
        })
        .await
        .unwrap();
    let derived_project_id = db
        .project_id_for_existing_path(&workspace)
        .await
        .unwrap()
        .unwrap();
    let entry = db
        .save_memory_entry_for_scope(
            Some(&derived_project_id),
            None,
            MemorySaveInput {
                scope: MemoryScope::Project,
                kind: MemoryEntryKind::Fact,
                title: "Archive candidate entry".into(),
                content: "archivereversiblemarker should disappear while archived.".into(),
                files: vec!["src/app_server/request_processors/memory_processor.rs".into()],
                concepts: vec!["desktop memory archive".into()],
                source_refs: vec![],
                pinned: false,
            },
            "test",
        )
        .await
        .unwrap();

    let archived = facade
        .memory_update(
            &project.id,
            MemoryUpdateParams {
                workspace_root: None,
                entry_id: entry.id.clone(),
                action: MemoryUpdateAction::Archive,
                scope: None,
                kind: None,
                title: None,
                content: None,
                files: None,
                concepts: None,
                pinned: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(archived.entry.status, "archived");

    let archived_list = facade
        .memory_list_archived(
            &project.id,
            MemoryListArchivedParams {
                workspace_root: None,
                scope: None,
                query: Some("archivereversiblemarker".into()),
                limit: Some(10),
            },
        )
        .await
        .unwrap();
    assert_eq!(archived_list.archived.len(), 1);
    assert_eq!(archived_list.archived[0].id, entry.id);

    let archived_hits = facade
        .memory_search(&project.id, None, "archivereversiblemarker", 10)
        .await
        .unwrap();
    assert!(archived_hits.hits.is_empty());

    let restored = facade
        .memory_update(
            &project.id,
            MemoryUpdateParams {
                workspace_root: None,
                entry_id: entry.id.clone(),
                action: MemoryUpdateAction::Unarchive,
                scope: None,
                kind: None,
                title: None,
                content: None,
                files: None,
                concepts: None,
                pinned: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(restored.entry.status, "active");

    let archived_after_restore = facade
        .memory_list_archived(
            &project.id,
            MemoryListArchivedParams {
                workspace_root: None,
                scope: None,
                query: Some("archivereversiblemarker".into()),
                limit: Some(10),
            },
        )
        .await
        .unwrap();
    assert!(archived_after_restore.archived.is_empty());

    let restored_hits = facade
        .memory_search(&project.id, None, "archivereversiblemarker", 10)
        .await
        .unwrap();
    assert_eq!(restored_hits.hits.len(), 1);
    assert_eq!(restored_hits.hits[0].id, entry.id);
}
