use async_trait::async_trait;
use exagent::app_server::desktop_facade::{DesktopFacade, NewProjectRequest};
use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionStatus, ApprovalsListParams, CheckpointRestoreParams,
    CheckpointRestoreStatus, EventsReplayParams, PendingApprovalKind, ThreadForkParams,
    TurnContextOverrides, TurnStartParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::index_db::{IndexDb, ThreadListFilter};
use exagent::llm::{LlmClient, LlmRequestOptions, MockLlm};
use exagent::model::factory::SharedLlmFactory;
use exagent::policy::PolicyMode;
use exagent::registry::ToolRegistry;
use exagent::resolved::{ModelRef, ResolvedCredential, ResolvedModelConfig};
use exagent::resolver::{EnvModelResolver, ModelResolver};
use exagent::state::fork_edges::fork_edges_path;
use exagent::tools::run_command::RunCommandTool;
use exagent::tools::ToolSpec;
use exagent::types::{AssistantTurn, ConversationMessage, LlmCompletion, ThreadId, ToolCall};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct RecordingToolsLlm {
    observed_tools: Arc<Mutex<Vec<Vec<String>>>>,
    observed_prompts: Arc<Mutex<Vec<Vec<String>>>>,
}

struct StaticModelResolver {
    resolved: ResolvedModelConfig,
}

#[async_trait]
impl ModelResolver for StaticModelResolver {
    async fn resolve(&self, _model_ref: &ModelRef) -> anyhow::Result<ResolvedModelConfig> {
        Ok(self.resolved.clone())
    }
}

#[async_trait]
impl LlmClient for RecordingToolsLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        self.observed_tools
            .lock()
            .unwrap()
            .push(tools.iter().map(|tool| tool.name.clone()).collect());
        self.observed_prompts.lock().unwrap().push(
            _messages
                .iter()
                .map(|message| message.content.clone())
                .collect(),
        );
        Ok(AssistantTurn {
            text: Some("recorded tools".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }
        .into_completion())
    }
}

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
async fn desktop_facade_list_threads_ignores_broken_fork_edge_store() {
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
            path: project,
        })
        .await
        .unwrap();
    let started = facade.start_thread(&project_record.id).await.unwrap();
    let fork_edges_path = fork_edges_path(&project_record.path);
    tokio::fs::create_dir_all(fork_edges_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&fork_edges_path, "{not valid json")
        .await
        .unwrap();

    let threads = facade
        .list_threads(ThreadListFilter {
            project_id: project_record.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap();

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, started.thread.id);
    assert_eq!(threads[0].fork_parent_thread_id, None);
    assert_eq!(threads[0].fork_point_turn_id, None);
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
            reasoning: vec![],
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
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
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

#[tokio::test]
async fn desktop_facade_root_turn_exposes_subagent_tools() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let observed_tools = Arc::new(Mutex::new(Vec::new()));
    let observed_prompts = Arc::new(Mutex::new(Vec::new()));
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(RecordingToolsLlm {
            observed_tools: observed_tools.clone(),
            observed_prompts: observed_prompts.clone(),
        }),
        exagent::default_tool_registry,
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
    facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "record visible tools".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await
        .unwrap();

    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;

    let tools = observed_tools.lock().unwrap();
    let names = tools.first().expect("observed tool names");
    assert!(names.contains(&"spawn_agent".to_string()));
    assert!(names.contains(&"list_agents".to_string()));
    assert!(names.contains(&"send_message".to_string()));
    assert!(names.contains(&"wait_agent".to_string()));

    // Subagent collaboration guidance now lives in the spawn_agent tool
    // description, not in a per-turn injected prompt message. Confirm the old
    // per-turn guidance is gone while the collaboration tools stay visible.
    let prompts = observed_prompts.lock().unwrap();
    let prompt = prompts
        .first()
        .expect("observed prompt")
        .join("\n--- message ---\n");
    assert!(!prompt.contains("Subagent collaboration tools are available"));
    assert!(
        prompts
            .first()
            .expect("observed prompt")
            .iter()
            .any(|message| message.contains("record visible tools")),
        "user prompt should be present",
    );
}

#[tokio::test]
async fn desktop_facade_settings_constructor_exposes_subagent_tools() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let observed_tools = Arc::new(Mutex::new(Vec::new()));
    let observed_prompts = Arc::new(Mutex::new(Vec::new()));
    let llm = RecordingToolsLlm {
        observed_tools: observed_tools.clone(),
        observed_prompts: observed_prompts.clone(),
    };
    let service = AppServerService::with_config_llm_factory_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(SharedLlmFactory::new(Arc::new(llm))),
        Arc::new(EnvModelResolver),
        db.clone(),
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
    facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "record visible tools".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await
        .unwrap();

    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;

    let tools = observed_tools.lock().unwrap();
    let names = tools.first().expect("observed tool names");
    assert!(names.contains(&"spawn_agent".to_string()));
    assert!(names.contains(&"list_agents".to_string()));
    assert!(names.contains(&"send_message".to_string()));
    assert!(names.contains(&"wait_agent".to_string()));

    // Subagent collaboration guidance now lives in the spawn_agent tool
    // description, not in a per-turn injected prompt message. Confirm the old
    // per-turn guidance is gone while the collaboration tools stay visible.
    let prompts = observed_prompts.lock().unwrap();
    let prompt = prompts
        .first()
        .expect("observed prompt")
        .join("\n--- message ---\n");
    assert!(!prompt.contains("Subagent collaboration tools are available"));
    assert!(
        prompts
            .first()
            .expect("observed prompt")
            .iter()
            .any(|message| message.contains("record visible tools")),
        "user prompt should be present",
    );
}

#[tokio::test]
async fn desktop_facade_model_override_preserves_subagent_tools() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let observed_tools = Arc::new(Mutex::new(Vec::new()));
    let observed_prompts = Arc::new(Mutex::new(Vec::new()));
    let llm = RecordingToolsLlm {
        observed_tools: observed_tools.clone(),
        observed_prompts: observed_prompts.clone(),
    };
    let deepseek_model = ResolvedModelConfig::from_provider_profile(
        "deepseek",
        "deepseek-v4-pro",
        None,
        ResolvedCredential::None,
        Some(1_000_000),
    );
    let service = AppServerService::with_config_llm_factory_model_resolver_and_goal_store(
        AgentConfig::default(),
        Arc::new(SharedLlmFactory::new(Arc::new(llm))),
        Arc::new(StaticModelResolver {
            resolved: deepseek_model,
        }),
        db.clone(),
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
    facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "record visible tools with model override".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: Some(TurnContextOverrides {
                    cwd: None,
                    model: Some(ModelRef::new("deepseek", "deepseek-v4-pro")),
                    thinking_mode: None,
                    clear_thinking_mode: false,
                }),
            },
        )
        .await
        .unwrap();

    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;

    let tools = observed_tools.lock().unwrap();
    let names = tools.first().expect("observed tool names");
    assert!(names.contains(&"spawn_agent".to_string()));
    assert!(names.contains(&"list_agents".to_string()));
    assert!(names.contains(&"send_message".to_string()));
    assert!(names.contains(&"wait_agent".to_string()));
}

#[tokio::test]
async fn desktop_facade_forks_thread_and_reindexes_project() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("desktop fork source".into()),
            tool_calls: vec![],
            reasoning: vec![],
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
                prompt: "source prompt".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await
        .unwrap();
    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;

    let forked = facade
        .fork_thread(
            &project_record.id,
            ThreadForkParams {
                thread_id: started.thread.id.clone(),
                at_turn_id: turn.turn.id.clone(),
                workspace_root: None,
            },
        )
        .await
        .unwrap();
    let child = facade
        .read_thread(
            &project_record.id,
            exagent::app_server::protocol::ThreadReadParams {
                thread_id: forked.new_thread_id.clone(),
                workspace_root: None,
            },
        )
        .await
        .unwrap();
    let threads = facade
        .list_threads(ThreadListFilter {
            project_id: project_record.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap();
    let reindexed = facade.reindex_project(&project_record.id).await.unwrap();

    assert_eq!(forked.parent_thread_id, started.thread.id);
    assert_eq!(forked.fork_point_turn_id, turn.turn.id);
    assert_eq!(child.thread.turns.len(), 1);
    let parent_row = threads
        .iter()
        .find(|thread| thread.id == started.thread.id)
        .expect("parent thread row");
    let child_row = threads
        .iter()
        .find(|thread| thread.id == forked.new_thread_id)
        .expect("fork child thread row");
    assert_eq!(parent_row.fork_parent_thread_id, None);
    assert_eq!(parent_row.fork_point_turn_id, None);
    assert_eq!(
        child_row.fork_parent_thread_id,
        Some(started.thread.id.clone())
    );
    assert_eq!(child_row.fork_point_turn_id, Some(turn.turn.id.clone()));
    let reindexed_child = reindexed
        .iter()
        .find(|thread| thread.id == forked.new_thread_id)
        .expect("reindexed fork child thread row");
    assert_eq!(
        reindexed_child.fork_parent_thread_id,
        Some(started.thread.id.clone())
    );
    assert_eq!(
        reindexed_child.fork_point_turn_id,
        Some(turn.turn.id.clone())
    );
}

#[tokio::test]
async fn desktop_facade_lists_pending_approvals_for_project() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(project.join("scratch"))
        .await
        .unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: project.clone(),
            cwd: project.clone(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("request approval".into()),
            tool_calls: vec![ToolCall {
                id: "call_desktop_approval".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                thought_signature: None,
            }],
            reasoning: vec![],
        }])),
        run_command_registry,
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
    facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "trigger approval".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await
        .unwrap();

    let listed = wait_for_approvals_count(&facade, &project_record.id, 1).await;
    assert_eq!(listed.approvals[0].thread_id, started.thread.id);
    assert_eq!(listed.approvals[0].kind, PendingApprovalKind::Command);
    assert_eq!(listed.approvals[0].summary, "run_command: rm -rf scratch");
    assert_eq!(listed.approvals[0].detail, "rm -rf scratch");
}

#[tokio::test]
async fn desktop_facade_restores_checkpoint_for_project() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    init_git_repo(&project);
    std::fs::write(project.join("tracked.txt"), "base\n").unwrap();
    std::fs::create_dir_all(project.join("scratch")).unwrap();
    std::fs::write(project.join("scratch").join("note.txt"), "before\n").unwrap();
    git(&project, ["add", "tracked.txt", "scratch/note.txt"]);
    git(&project, ["commit", "-m", "initial"]);

    let db = IndexDb::open(dir.path().join("exagent.sqlite"))
        .await
        .unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: project.clone(),
            cwd: project.clone(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("request approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_desktop_restore_checkpoint".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("approval handled".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        run_command_registry,
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
    let turn = facade
        .start_turn(
            &project_record.id,
            TurnStartParams {
                thread_id: started.thread.id.clone(),
                prompt: "trigger approval".into(),
                input: vec![],
                workspace_root: None,
                turn_mode: Default::default(),
                turn_context: None,
            },
        )
        .await
        .unwrap();
    let listed = wait_for_approvals_count(&facade, &project_record.id, 1).await;
    let approval = &listed.approvals[0];
    let checkpoint_id = approval
        .checkpoint_id
        .clone()
        .expect("approval-derived checkpoint id");

    facade
        .approval_decision(
            &project_record.id,
            ApprovalDecisionParams {
                thread_id: started.thread.id.clone(),
                turn_id: Some(turn.turn.id.clone()),
                approval_id: approval.approval_id.clone(),
                decision: ApprovalDecisionStatus::Approved,
                note: Some("allow mutation".into()),
                workspace_root: None,
            },
        )
        .await
        .unwrap();
    wait_for_turn_completed(&facade, &project_record.id, started.thread.id.clone()).await;
    assert!(
        !project.join("scratch").exists(),
        "approved command should mutate workspace before restore"
    );

    let restored = facade
        .checkpoint_restore(
            &project_record.id,
            CheckpointRestoreParams {
                workspace_root: "/ignored/by/facade".to_string(),
                checkpoint_id: checkpoint_id.clone(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        restored.workspace_root,
        project.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(restored.checkpoint_id, checkpoint_id);
    assert_eq!(restored.status, CheckpointRestoreStatus::Restored);
    assert_eq!(
        std::fs::read_to_string(project.join("scratch").join("note.txt")).unwrap(),
        "before\n"
    );
}

fn run_command_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);
    registry
}

fn init_git_repo(path: &std::path::Path) {
    git(path, ["init"]);
    git(path, ["config", "user.name", "ExAgent Test"]);
    git(
        path,
        ["config", "user.email", "exagent-test@example.invalid"],
    );
}

fn git<const N: usize>(cwd: &std::path::Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

async fn wait_for_approvals_count(
    facade: &DesktopFacade,
    project_id: &str,
    expected: usize,
) -> exagent::app_server::protocol::ApprovalsListResponse {
    for _ in 0..200 {
        let listed = facade
            .approvals_list(
                project_id,
                ApprovalsListParams {
                    workspace_root: None,
                },
            )
            .await
            .unwrap();
        if listed.approvals.len() == expected {
            return listed;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for {expected} pending approvals");
}
