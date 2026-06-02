use exagent::app_server::desktop_facade::{DesktopFacade, NewProjectRequest};
use exagent::app_server::protocol::{EventsReplayParams, TurnStartParams};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::index_db::{IndexDb, ThreadListFilter};
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use exagent::types::{AssistantTurn, ThreadId};
use tempfile::tempdir;

#[tokio::test]
async fn desktop_facade_adds_project_and_starts_thread() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );
    let facade = DesktopFacade::new(service, db);

    let project_record = facade
        .add_project(NewProjectRequest {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();
    let started = facade.start_thread(&project_record.id).await.unwrap();
    let threads = facade
        .list_threads(ThreadListFilter {
            project_id: project_record.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap();

    assert_eq!(project_record.path, project.canonicalize().unwrap());
    assert_eq!(started.thread.turns.len(), 0);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, started.thread.id);
}

#[tokio::test]
async fn desktop_facade_runs_turn_replays_events_and_updates_index() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("desktop turn complete".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );
    let facade = DesktopFacade::new(service, db);

    let project_record = facade
        .add_project(NewProjectRequest {
            name: "Project".into(),
            path: project,
        })
        .await
        .unwrap();
    let started = facade.start_thread(&project_record.id).await.unwrap();
    let turn = facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "run the desktop chain".into(),
                workspace_root: None,
                turn_context: None,
            },
        )
        .await
        .unwrap();

    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;

    let replay = facade
        .events_replay(
            &project_record.id,
            EventsReplayParams {
                thread_id: started.thread.id.clone(),
                workspace_root: None,
                after_event_id: None,
                limit: None,
                include_snapshot: true,
                event_kinds: vec![],
            },
        )
        .await
        .unwrap();
    let threads = facade
        .list_threads(ThreadListFilter {
            project_id: project_record.id,
            include_archived: false,
            search: Some("run the desktop chain".into()),
        })
        .await
        .unwrap();

    assert_eq!(turn.thread_id, started.thread.id);
    assert!(replay
        .events
        .iter()
        .any(|event| matches!(&event.kind, RuntimeEventKind::AssistantTurn { turn } if turn.text.as_deref() == Some("desktop turn complete"))));
    assert!(replay
        .events
        .iter()
        .any(|event| matches!(event.kind, RuntimeEventKind::TurnCompleted)));
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, started.thread.id);
}

async fn wait_for_turn_completed(facade: &DesktopFacade, project_id: &str, thread_id: ThreadId) {
    for _ in 0..200 {
        let replay = facade
            .events_replay(
                project_id,
                EventsReplayParams {
                    thread_id: thread_id.clone(),
                    workspace_root: None,
                    after_event_id: None,
                    limit: None,
                    include_snapshot: false,
                    event_kinds: vec![],
                },
            )
            .await
            .unwrap();
        if replay
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::TurnCompleted))
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for desktop facade turn completion");
}
