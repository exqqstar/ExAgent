use exagent::app_server::desktop_facade::{DesktopFacade, NewProjectRequest};
use exagent::app_server::protocol::{
    BoundaryCapability, BoundaryOp, BoundaryOpResponse, InitializeParams, MemoryUpdateAction,
    MemoryUpdateParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::index_db::IndexDb;
use exagent::resolver::EnvModelResolver;
use exagent::state::memory::{
    MemoryCodeRef, MemoryEntryKind, MemoryObservationKind, MemoryObservationUpsert,
    MemoryPrivacyFlags, MemorySaveInput, MemoryScope,
};
use exagent::types::ThreadId;
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
            source_observation_ids: vec![],
            pinned: false,
        },
        "test",
    )
    .await
    .unwrap();

    let response = facade
        .memory_search(&project.id, None, "Derived workspace memory", false, 10)
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
                source_observation_ids: vec![],
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
                source_observation_ids: None,
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
                source_observation_ids: None,
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
        .memory_search(&project.id, None, "appserveroldgone", false, 10)
        .await
        .unwrap();
    assert!(old_hits.hits.is_empty());

    let new_hits = facade
        .memory_search(&project.id, None, "appservernewkept", false, 10)
        .await
        .unwrap();
    assert_eq!(new_hits.hits.len(), 1);
    assert_eq!(new_hits.hits[0].id, superseded.entry.id);
}

#[tokio::test]
async fn desktop_memory_empty_search_lists_inspection_rows_with_trust_metadata() {
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
            name: "Memory Inspect".into(),
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
                title: "Pinned inspection entry".into(),
                content: "Empty desktop memory search should list active entries.".into(),
                files: vec!["src/app_server/request_processors/memory_processor.rs".into()],
                concepts: vec!["desktop memory inspection".into()],
                source_observation_ids: vec![],
                pinned: true,
            },
            "test",
        )
        .await
        .unwrap();
    db.upsert_memory_observations_incremental(vec![MemoryObservationUpsert {
        id: "obs_desktop_inspect".into(),
        scope: MemoryScope::Project,
        project_id: Some(derived_project_id),
        thread_id: ThreadId::new("thread_memory_inspect"),
        turn_id: None,
        event_id: None,
        source_tool_call_id: None,
        kind: MemoryObservationKind::FileRead,
        title: "Low trust observation".into(),
        narrative: "Empty desktop memory search should list observations separately.".into(),
        files: vec!["src/app_server/protocol.rs".into()],
        code_refs: vec![MemoryCodeRef {
            path: "src/app_server/protocol.rs".into(),
            line: None,
            symbol: None,
        }],
        concepts: vec!["desktop memory inspection".into()],
        importance: 2,
        confidence: 0.35,
        auto_inject_eligible: false,
        privacy_flags: MemoryPrivacyFlags::default(),
        created_at_ms: 1_700_000_000_000,
    }])
    .await
    .unwrap();

    let response = facade
        .memory_search(&project.id, None, "", true, 10)
        .await
        .unwrap();

    let active = response
        .hits
        .iter()
        .find(|hit| hit.id == entry.id)
        .expect("active entry listed");
    assert_eq!(active.source, "entry");
    assert!(active.pinned);
    assert_eq!(active.status.as_deref(), Some("active"));
    assert_eq!(active.use_count, 0);

    let observation = response
        .hits
        .iter()
        .find(|hit| hit.id == "obs_desktop_inspect")
        .expect("observation listed");
    assert_eq!(observation.source, "observation");
    assert_eq!(observation.confidence, 0.35);
    assert!(!observation.pinned);
}
