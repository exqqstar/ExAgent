use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, LlmRequestOptions, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::resolved::ModelRef;
use exagent::runtime::agent_profile::AgentType;
use exagent::runtime::subagent::InterAgentCommunication;
use exagent::runtime::thread_runtime::{
    AgentFactory, ThreadOpResult, ThreadRuntime, ThreadRuntimeOptions, ThreadRuntimeStatus,
    ThreadTurnContext,
};
use exagent::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
use exagent::runtime::turn_mode::TurnMode;
use exagent::session::TurnContextItem;
use exagent::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use exagent::tools::ToolSpec;
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, LlmCompletion, ReasoningBlock, ReasoningSignature,
    ThreadId, TurnId,
};
use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
    let rollout_paths = rollout_paths(&config.workspace_root, thread_id);
    RolloutStore::new(rollout_paths.rollout_path)
        .append_items_blocking(&[RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: config.workspace_root.clone(),
            initial_cwd: config.cwd.clone(),
            permission_profile: exagent::config::PermissionProfile::FullAccess,
            thread_source: Default::default(),
            lineage: None,
            created_at: "2026-05-20T00:00:00Z".to_string(),
        })])
        .expect("write rollout session meta");
}

#[test]
fn thread_session_can_be_constructed_as_runtime_state_owner() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_thread_session_construct");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("create thread session");

    assert_eq!(session.thread_id(), &thread_id);
}

#[test]
fn thread_without_rollout_meta_is_not_loaded_as_runtime_state() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_no_rollout_meta");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let result = ThreadSession::new(ThreadSessionOptions::new(
        thread_id,
        config,
        agent_factory(vec![]),
    ));

    assert!(result.is_err());
}

#[tokio::test]
async fn thread_runtime_starts_idle_and_accepts_shutdown_op() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_test");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    assert_eq!(runtime.thread_id(), &thread_id);
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Idle);

    runtime.shutdown().await.expect("submit shutdown");
    runtime.wait_until_terminated().await;
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
}

#[tokio::test]
async fn user_turn_persists_plan_mode_context() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_plan_mode_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        agent_factory(vec![AssistantTurn {
            text: Some("planned".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }]),
    ))
    .expect("spawn runtime");
    let result = runtime
        .submit_user_input_and_wait(
            "plan this change".into(),
            Some(ThreadTurnContext {
                cwd: None,
                resolved_model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
                turn_mode: TurnMode::Plan,
            }),
        )
        .await
        .expect("turn");
    let ThreadOpResult::UserInput { turn_id, .. } = result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_1"));

    let live_view = runtime.live_view();
    let context = live_view
        .snapshot
        .reference_turn_context
        .expect("turn context");

    assert_eq!(context.turn_mode, TurnMode::Plan);
    assert_eq!(context.agent_type, Some(AgentType::Planner));
    assert!(context
        .agent_profile_instructions
        .as_deref()
        .unwrap()
        .contains("planner agent"));
}

#[tokio::test]
async fn thread_session_loads_rollout_session_meta() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_start");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let _session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("create thread session");

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");

    assert!(matches!(items.first(), Some(RolloutItem::ThreadMeta(_))));
}

#[tokio::test]
async fn thread_resume_reconstructs_context_from_rollout_without_snapshot() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_resume");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let turn_context = TurnContextItem {
        turn_id: TurnId::new("turn_1"),
        workspace_root: config.workspace_root.clone(),
        cwd: config.cwd.clone(),
        model: ModelRef::new("openai", "mock"),
        policy_mode: exagent::policy::PolicyMode::Off,
        permission_profile: exagent::config::PermissionProfile::FullAccess,
        command_timeout_secs: 30,
        max_output_bytes: 1024,
        turn_mode: exagent::runtime::turn_mode::TurnMode::Default,
        agent_type: None,
        agent_profile_instructions: None,
        agent_response_guidance: None,
        thinking_mode: None,
        agent_role: None,
        current_utc_date: Some("2026-05-20".to_string()),
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: config.workspace_root.clone(),
                initial_cwd: config.cwd.clone(),
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            RolloutItem::TurnContext(turn_context),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("resume user"),
            ),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::assistant(Some("resume assistant".to_string()), vec![]),
            ),
        ])
        .await
        .expect("write rollout");

    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("resume thread runtime");

    let live_view = runtime.live_view();
    assert_eq!(live_view.snapshot.conversation.len(), 2);
    assert_eq!(live_view.snapshot.conversation[0].content, "resume user");
    assert_eq!(
        live_view.snapshot.conversation[1].content,
        "resume assistant"
    );
    assert!(live_view.snapshot.reference_turn_context.is_some());
}

#[tokio::test]
async fn runtime_restore_uses_rollout_projection_for_compaction_metadata() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_compaction_restore");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: config.workspace_root.clone(),
                initial_cwd: config.cwd.clone(),
                permission_profile: exagent::config::PermissionProfile::FullAccess,
                thread_source: Default::default(),
                lineage: None,
                created_at: "2026-05-20T00:00:00Z".to_string(),
            }),
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("pre-compaction user"),
            ),
            RolloutItem::Compacted(exagent::state::rollout::CompactedItem {
                message: "compacted history".to_string(),
                replacement_history: Some(vec![ConversationMessage::assistant(
                    Some("summary history".to_string()),
                    vec![],
                )]),
            }),
        ])
        .await
        .expect("write rollout");

    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        agent_factory(vec![]),
    ))
    .expect("resume thread runtime");

    let live_view = runtime.live_view();
    assert_eq!(
        live_view
            .snapshot
            .latest_compaction
            .as_ref()
            .map(|compaction| compaction.summary.as_str()),
        Some("compacted history")
    );
    assert_eq!(live_view.snapshot.conversation.len(), 1);
    assert_eq!(
        live_view.snapshot.conversation[0].content,
        "summary history"
    );
}

#[tokio::test]
async fn thread_runtime_live_view_uses_loaded_session_state_not_disk_mutations() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_live_view");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let rollout_paths = exagent::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    exagent::transcript::append_json_line(
        &rollout_paths.rollout_path,
        &exagent::state::rollout::RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_disk_only"),
            thread_id: thread_id.clone(),
            turn_id: Some(TurnId::new("turn_disk_only")),
            kind: RuntimeEventKind::RuntimeError {
                message: "disk mutation after runtime load".into(),
            },
        }),
    )
    .expect("append disk-only event");

    let live_view = runtime.live_view();

    assert_eq!(live_view.thread_id, thread_id);
    assert!(live_view.events.is_empty());
    assert!(live_view.snapshot.conversation.is_empty());
}

#[tokio::test]
async fn thread_runtime_runs_user_input_through_agent_and_records_turn_lifecycle() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let final_turn = AssistantTurn {
        text: Some("runtime turn complete".into()),
        tool_calls: vec![],
        reasoning: vec![],
    };
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![final_turn]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait("continue".into(), None)
        .await
        .expect("run turn");

    let ThreadOpResult::UserInput {
        turn_id: actual_turn_id,
        final_turn,
    } = result
    else {
        panic!("expected user input result");
    };
    assert_eq!(actual_turn_id, turn_id);
    assert_eq!(final_turn.text.as_deref(), Some("runtime turn complete"));

    let live_view = runtime.live_view();
    assert!(matches!(
        live_view.events[0].kind,
        RuntimeEventKind::TurnStarted
    ));
    assert!(matches!(
        live_view.events[1].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(
        live_view.events[2].kind,
        RuntimeEventKind::TurnCompleted
    ));
    assert_eq!(live_view.events[0].turn_id.as_ref(), Some(&turn_id));
    assert_eq!(live_view.events[2].turn_id.as_ref(), Some(&turn_id));

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event) if matches!(event.kind, RuntimeEventKind::TurnStarted)
    )));
    assert!(rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::EventMsg(event) if matches!(event.kind, RuntimeEventKind::TurnCompleted)
    )));
}

#[tokio::test]
async fn thread_turn_records_rollout_items_and_context_history() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_turn");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![AssistantTurn {
            text: Some("rollout assistant".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait("rollout user".into(), None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput {
        turn_id: actual_turn_id,
        ..
    } = result
    else {
        panic!("expected user input result");
    };
    assert_eq!(actual_turn_id, turn_id);

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");

    assert!(items
        .iter()
        .any(|item| matches!(item, RolloutItem::TurnContext(_))));
    assert!(items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response_item)
            if response_item.turn_id == TurnId::new("turn_1")
                && response_item.message.content == "rollout user"
    )));
    assert!(items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response_item)
            if response_item.turn_id == TurnId::new("turn_1")
                && response_item.message.content == "rollout assistant"
    )));

    let live_view = runtime.live_view();
    assert_eq!(live_view.snapshot.conversation.len(), 4);
    assert!(live_view.snapshot.conversation[0].injected);
    assert!(live_view.snapshot.conversation[1].injected);
    assert_eq!(live_view.snapshot.conversation[2].content, "rollout user");
    assert_eq!(
        live_view.snapshot.conversation[3].content,
        "rollout assistant"
    );
}

#[tokio::test]
async fn thread_turn_injects_project_docs_prompt_context() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().join("packages/cli");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(dir.path().join("AGENTS.md"), "Root project rule").unwrap();
    fs::write(cwd.join("AGENTS.override.md"), "Child override rule").unwrap();
    let thread_id = ThreadId::new("thread_project_docs_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().join("packages/cli"),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![AssistantTurn {
                text: Some("project docs observed".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait("use local rules".into(), None)
        .await
        .expect("run turn");

    let prompt = prompts.lock().expect("prompts")[0].clone();
    assert!(prompt.contains("AGENTS.md instructions"));
    let root_index = prompt.find("Root project rule").expect("root docs");
    let child_index = prompt.find("Child override rule").expect("child docs");
    assert!(root_index < child_index);

    let live_view = runtime.live_view();
    assert!(!live_view
        .snapshot
        .conversation
        .iter()
        .any(|message| message.content.contains("Root project rule")));
}

#[tokio::test]
async fn thread_turn_injects_project_doc_warnings_prompt_context() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("AGENTS.md"), [0xff, 0xfe]).unwrap();
    let thread_id = ThreadId::new("thread_project_docs_warning_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![AssistantTurn {
                text: Some("project docs warning observed".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait("use local rules".into(), None)
        .await
        .expect("run turn");

    let prompt = prompts.lock().expect("prompts")[0].clone();
    assert!(prompt.contains("AGENTS.md instructions"));
    assert!(prompt.contains("Warnings"));
    assert!(prompt.contains("project doc is not valid UTF-8"));
}

#[tokio::test]
async fn thread_turn_injects_available_skills_and_explicit_skill_body() {
    let dir = tempdir().unwrap();
    let skill_dir = dir.path().join(".agents/skills/review");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review code changes carefully.\n---\n\nFull review playbook.\n",
    )
    .unwrap();
    let thread_id = ThreadId::new("thread_skill_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![AssistantTurn {
                text: Some("skill observed".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait("please use $review".into(), None)
        .await
        .expect("run turn");

    let prompt = prompts.lock().expect("prompts")[0].clone();
    assert!(prompt.contains("Available skills"));
    // Implicit-invocation guidance: the model is told to use a skill when the
    // task matches its description and to open the SKILL.md itself.
    assert!(prompt.contains("How to use skills"));
    assert!(prompt.contains("the task clearly matches a skill's description"));
    assert!(prompt.contains("$review [repo]: Review code changes carefully."));
    assert!(prompt.contains("# Skill: review"));
    assert!(prompt.contains("Full review playbook."));

    let live_view = runtime.live_view();
    assert!(!live_view
        .snapshot
        .conversation
        .iter()
        .any(|message| message.content.contains("Full review playbook.")));
}

#[tokio::test]
async fn thread_turn_injects_skill_warnings_prompt_context() {
    let dir = tempdir().unwrap();
    for dirname in ["first", "second"] {
        let skill_dir = dir.path().join(".agents/skills").join(dirname);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: duplicate\ndescription: Duplicate skill.\n---\n\nBody.\n",
        )
        .unwrap();
    }
    let thread_id = ThreadId::new("thread_skill_warning_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![AssistantTurn {
                text: Some("skill warning observed".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait("continue".into(), None)
        .await
        .expect("run turn");

    let prompt = prompts.lock().expect("prompts")[0].clone();
    assert!(prompt.contains("Skill warnings"));
    assert!(prompt.contains("duplicate_name [repo] duplicate"));
}

#[tokio::test]
async fn thread_turn_does_not_load_unmentioned_skill_body() {
    let dir = tempdir().unwrap();
    let skill_dir = dir.path().join(".agents/skills/debug");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: debug\ndescription: Debug failing behavior.\n---\n\nSecret full body.\n",
    )
    .unwrap();
    let thread_id = ThreadId::new("thread_unmentioned_skill_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![AssistantTurn {
                text: Some("metadata only".into()),
                tool_calls: vec![],
                reasoning: vec![],
            }],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    runtime
        .submit_user_input_and_wait("continue without a skill".into(), None)
        .await
        .expect("run turn");

    let prompt = prompts.lock().expect("prompts")[0].clone();
    assert!(prompt.contains("$debug [repo]: Debug failing behavior."));
    assert!(!prompt.contains("Secret full body."));
}

#[tokio::test]
async fn thread_turn_hides_explicit_only_skill_from_list_but_still_injects_on_mention() {
    let dir = tempdir().unwrap();
    let skill_dir = dir.path().join(".agents/skills/secret");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: secret\ndescription: Explicit only skill.\nallow_implicit_invocation: false\n---\n\nSecret playbook body.\n",
    )
    .unwrap();
    let thread_id = ThreadId::new("thread_explicit_only_skill_context");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id,
        config,
        recording_agent_factory(
            vec![
                AssistantTurn {
                    text: Some("no skill yet".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
                AssistantTurn {
                    text: Some("skill loaded".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
                },
            ],
            prompts.clone(),
        ),
    ))
    .expect("spawn runtime");

    // Turn 1: no explicit mention. The skill must not appear in the available
    // list (it is explicit-only) and its body must not be injected.
    runtime
        .submit_user_input_and_wait("just continue".into(), None)
        .await
        .expect("run turn 1");
    let first = prompts.lock().expect("prompts")[0].clone();
    assert!(!first.contains("$secret [repo]"));
    assert!(!first.contains("Secret playbook body."));

    // Turn 2: explicit `$secret` still loads the body.
    runtime
        .submit_user_input_and_wait("now use $secret".into(), None)
        .await
        .expect("run turn 2");
    let second = prompts.lock().expect("prompts")[1].clone();
    assert!(second.contains("# Skill: secret"));
    assert!(second.contains("Secret playbook body."));
}

#[tokio::test]
async fn rollout_thread_turn_does_not_write_snapshot_or_events_files() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("thread_rollout_no_legacy_files");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let store = RolloutStore::new(rollout_paths.rollout_path.clone());
    store
        .append_items(&[RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: thread_id.clone(),
            workspace_root: config.workspace_root.clone(),
            initial_cwd: config.cwd.clone(),
            permission_profile: exagent::config::PermissionProfile::FullAccess,
            thread_source: Default::default(),
            lineage: None,
            created_at: "2026-05-20T00:00:00Z".to_string(),
        })])
        .await
        .expect("write rollout meta");
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![AssistantTurn {
            text: Some("no legacy writes".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait("continue".into(), None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput {
        turn_id: actual_turn_id,
        ..
    } = result
    else {
        panic!("expected user input result");
    };
    assert_eq!(actual_turn_id, turn_id);

    assert!(rollout_paths.rollout_path.exists());
    let sessions_dir = config.workspace_root.join(".exagent").join("sessions");
    assert!(!sessions_dir.exists());
}

#[tokio::test]
async fn thread_runtime_live_view_tracks_snapshot_after_turn_without_disk_read() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_live_snapshot");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![AssistantTurn {
            text: Some("live snapshot complete".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait("continue".into(), None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput {
        turn_id: actual_turn_id,
        ..
    } = result
    else {
        panic!("expected user input result");
    };
    assert_eq!(actual_turn_id, turn_id);

    let live_view = runtime.live_view();

    assert_eq!(live_view.snapshot.conversation.len(), 4);
    assert!(live_view.snapshot.reference_turn_context.is_some());
    assert!(live_view.snapshot.conversation[0]
        .content
        .contains("Runtime context:"));
    assert!(live_view.snapshot.conversation[1]
        .content
        .contains("Environment context:"));
    assert_eq!(live_view.snapshot.conversation[2].content, "continue");
    assert_eq!(
        live_view.snapshot.conversation[3].content,
        "live snapshot complete"
    );
}

#[tokio::test]
async fn thread_runtime_allocates_monotonic_turn_ids_on_submit() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_next_turn_id");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![AssistantTurn {
            text: Some("first turn complete".into()),
            tool_calls: vec![],
            reasoning: vec![],
        }]),
    ))
    .expect("spawn runtime");

    let result = runtime
        .submit_user_input_and_wait("continue".into(), None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput { turn_id, .. } = result else {
        panic!("expected user input result");
    };
    assert_eq!(turn_id, TurnId::new("turn_1"));
}

#[tokio::test]
async fn thread_runtime_delivers_mailbox_messages_on_next_turn() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_mailbox_delivery");
    let turn_id = TurnId::new("turn_1");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let prompts_for_llm = prompts.clone();
    let factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingPromptLlm {
                turns: Mutex::new(VecDeque::from([AssistantTurn {
                    text: Some("mail received".to_string()),
                    tool_calls: vec![],
                    reasoning: vec![],
                }])),
                prompts: prompts_for_llm.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        factory,
    ))
    .expect("spawn runtime");

    runtime
        .submit_inter_agent_communication(InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_author"),
            author_path: "/root/research".to_string(),
            recipient_thread_id: thread_id.clone(),
            recipient_path: "/root".to_string(),
            other_recipients: Vec::new(),
            content: "research result: use approach B".to_string(),
            trigger_turn: true,
            source_turn_id: Some(TurnId::new("turn_author_1")),
            created_at: "2026-06-04T00:00:00Z".to_string(),
        })
        .await
        .expect("submit mailbox message");

    let result = runtime
        .submit_user_input_and_wait("continue after mail".into(), None)
        .await
        .expect("run turn");
    let ThreadOpResult::UserInput {
        turn_id: actual_turn_id,
        ..
    } = result
    else {
        panic!("expected user input result");
    };
    assert_eq!(actual_turn_id, turn_id);

    let prompt = prompts
        .lock()
        .expect("prompts")
        .first()
        .cloned()
        .expect("recorded prompt");
    assert!(prompt.contains("inter_agent_communication"));
    assert!(prompt.contains("/root/research"));
    assert!(prompt.contains("research result: use approach B"));

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(rollout_items.iter().any(|item| match item {
        RolloutItem::ResponseItem(response_item)
            if response_item.turn_id == turn_id && response_item.message.injected =>
        {
            InterAgentCommunication::from_conversation_message(&response_item.message).is_some_and(
                |mail| {
                    mail.content == "research result: use approach B"
                        && mail.trigger_turn
                        && mail.source_turn_id.as_ref() == Some(&TurnId::new("turn_author_1"))
                },
            )
        }
        _ => false,
    }));
}

#[tokio::test]
async fn thread_runtime_rejects_mail_for_a_different_recipient() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_mailbox_wrong_recipient");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        agent_factory(vec![]),
    ))
    .expect("spawn runtime");

    let err = runtime
        .submit_inter_agent_communication(InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_author"),
            author_path: "/root/research".to_string(),
            recipient_thread_id: ThreadId::new("thread_other"),
            recipient_path: "/root".to_string(),
            other_recipients: Vec::new(),
            content: "wrong recipient".to_string(),
            trigger_turn: false,
            source_turn_id: None,
            created_at: "2026-06-04T00:00:00Z".to_string(),
        })
        .await
        .expect_err("wrong recipient should be rejected");

    assert!(err.to_string().contains("does not match thread"));
}

fn agent_factory(turns: Vec<AssistantTurn>) -> AgentFactory {
    Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(MockLlm::new(turns.clone())),
            ToolRegistry::new(),
        ))
    })
}

struct RecordingPromptLlm {
    turns: Mutex<VecDeque<AssistantTurn>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

fn recording_agent_factory(
    turns: Vec<AssistantTurn>,
    prompts: Arc<Mutex<Vec<String>>>,
) -> AgentFactory {
    Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingPromptLlm {
                turns: Mutex::new(turns.clone().into()),
                prompts: prompts.clone(),
            }),
            ToolRegistry::new(),
        ))
    })
}

#[async_trait]
impl LlmClient for RecordingPromptLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        self.prompts
            .lock()
            .expect("prompts")
            .push(serde_json::to_string(messages).expect("serialize prompt"));
        self.turns
            .lock()
            .expect("turns")
            .pop_front()
            .map(AssistantTurn::into_completion)
            .ok_or_else(|| anyhow::anyhow!("RecordingPromptLlm is out of scripted turns"))
    }
}

#[tokio::test]
async fn reasoning_only_assistant_turn_records_event_without_poisoning_history() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_reasoning_only_not_history");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let prompts_for_llm = prompts.clone();
    let factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(RecordingPromptLlm {
                turns: Mutex::new(VecDeque::from([
                    AssistantTurn {
                        text: None,
                        tool_calls: vec![],
                        reasoning: vec![ReasoningBlock {
                            text: "hidden reasoning only".to_string(),
                            signature: Some(ReasoningSignature::AnthropicRedactedData(
                                "hidden-signature".to_string(),
                            )),
                            redacted: true,
                        }],
                    },
                    AssistantTurn {
                        text: Some("visible answer".to_string()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                ])),
                prompts: prompts_for_llm.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config.clone(),
        factory,
    ))
    .expect("spawn runtime");

    let first_result = runtime
        .submit_user_input_and_wait("first prompt".into(), None)
        .await
        .expect("first turn");
    let ThreadOpResult::UserInput {
        turn_id: first_turn_id,
        ..
    } = first_result
    else {
        panic!("expected user input result");
    };
    assert_eq!(first_turn_id, TurnId::new("turn_1"));

    let second_result = runtime
        .submit_user_input_and_wait("second prompt".into(), None)
        .await
        .expect("second turn");
    let ThreadOpResult::UserInput {
        turn_id: second_turn_id,
        ..
    } = second_result
    else {
        panic!("expected user input result");
    };
    assert_eq!(second_turn_id, TurnId::new("turn_2"));

    let live_view = runtime.live_view();
    assert!(live_view.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::AssistantTurn { turn }
            if turn.reasoning.iter().any(|block| block.text == "hidden reasoning only")
    )));
    assert!(
        !live_view.snapshot.conversation.iter().any(|message| message
            .reasoning
            .iter()
            .any(|block| block.text == "hidden reasoning only"))
    );

    let rollout_paths = rollout_paths(&config.workspace_root, &thread_id);
    let rollout_items = RolloutStore::read_items(&rollout_paths.rollout_path)
        .await
        .expect("read rollout");
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response_item)
            if response_item.message.reasoning.iter().any(|block| block.text == "hidden reasoning only")
    )));

    let prompts = prompts.lock().expect("prompts");
    assert_eq!(prompts.len(), 2);
    assert!(!prompts[1].contains("hidden reasoning only"));
    assert!(!prompts[1].contains("hidden-signature"));
}

struct PanicLlm;

#[async_trait]
impl LlmClient for PanicLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        panic!("simulated llm panic to verify StoppedGuard");
    }
}

#[tokio::test]
async fn thread_runtime_marks_stopped_when_loop_handler_panics() {
    let dir = tempdir().unwrap();
    let thread_id = ThreadId::new("session_runtime_panic_guard");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);

    let panicking_factory: AgentFactory =
        Arc::new(move |config| Ok(Agent::new(config, Box::new(PanicLlm), ToolRegistry::new())));
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(
        thread_id.clone(),
        config,
        panicking_factory,
    ))
    .expect("spawn runtime");

    // Submitting user input triggers the panic inside the loop. We expect the
    // completion oneshot to be dropped (sender lost during unwinding), so the
    // await returns Err -- but the important guarantee is below.
    let _ = runtime
        .submit_user_input_and_wait("trigger panic".into(), None)
        .await;

    tokio::time::timeout(Duration::from_secs(2), runtime.wait_until_terminated())
        .await
        .expect("StoppedGuard must report termination even when a handler panics");
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
}
