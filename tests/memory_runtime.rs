use exagent::index_db::{IndexDb, ProjectUpsert};
use exagent::runtime::memory::MemoryRuntime;

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
