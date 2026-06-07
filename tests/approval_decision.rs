use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionStatus, EventsReplayParams, ThreadReadParams,
    ThreadResumeParams, ThreadStartParams, ThreadStatus, TurnContextOverrides, TurnStartParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::MockLlm;
use exagent::policy::PolicyMode;
use exagent::registry::ToolRegistry;
use exagent::session::ApprovalId;
use exagent::state::rollout::{RolloutItem, RolloutStore};
use exagent::tools::run_command::RunCommandTool;
use exagent::types::{AssistantTurn, MessageRole, ThreadId, ToolCall};
use tempfile::tempdir;

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);
    registry
}

fn events_replay_params(thread_id: exagent::types::ThreadId) -> EventsReplayParams {
    EventsReplayParams {
        thread_id,
        workspace_root: None,
        after_event_id: None,
        limit: None,
        include_snapshot: false,
        event_kinds: vec![],
    }
}

async fn wait_for_approval_id(service: &AppServerService, thread_id: &ThreadId) -> ApprovalId {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let replay = service
                .events_replay(events_replay_params(thread_id.clone()))
                .unwrap();
            if let Some(id) = replay.events.iter().find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested { approval_id, .. } => {
                    Some(approval_id.clone())
                }
                _ => None,
            }) {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("approval request must be recorded")
}

async fn wait_for_new_approval_id(
    service: &AppServerService,
    thread_id: &ThreadId,
    previous: &ApprovalId,
) -> ApprovalId {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let replay = service
                .events_replay(events_replay_params(thread_id.clone()))
                .unwrap();
            if let Some(id) = replay
                .events
                .iter()
                .rev()
                .find_map(|event| match &event.kind {
                    RuntimeEventKind::ApprovalRequested { approval_id, .. }
                        if approval_id != previous =>
                    {
                        Some(approval_id.clone())
                    }
                    _ => None,
                })
            {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("new approval request must be recorded")
}

fn read_rollout_items(
    workspace_root: &std::path::Path,
    thread_id: &exagent::types::ThreadId,
) -> Vec<RolloutItem> {
    let workspace_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let rollout_paths = exagent::state::rollout::rollout_paths(&workspace_root, thread_id);
    RolloutStore::read_items_blocking(&rollout_paths.rollout_path).unwrap()
}

#[tokio::test]
async fn approval_decision_clears_waiting_approval() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("request approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;

    let approval_id = wait_for_approval_id(&service, &thread.id).await;

    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    let read = service
        .thread_read(ThreadReadParams {
            thread_id: thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    let replay = service
        .events_replay(events_replay_params(thread.id))
        .unwrap();

    assert_ne!(read.thread.status, ThreadStatus::WaitingApproval);
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ApprovalDecision { note, .. }
            if note.as_deref() == Some("desktop denied")
    )));
}

#[tokio::test]
async fn approval_decision_denies_cold_pending_approval_after_restart() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("request approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky_cold".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;
    let approval_id = wait_for_approval_id(&service, &thread.id).await;

    let restarted = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        registry,
    );
    let response = restarted
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id: approval_id.clone(),
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied after restart".into()),
            workspace_root: None,
        })
        .await
        .expect("cold approval decision should be accepted");

    assert_eq!(response.approval_id, approval_id);

    let read = restarted
        .thread_read(ThreadReadParams {
            thread_id: thread.id.clone(),
            workspace_root: None,
        })
        .unwrap();
    assert_ne!(read.thread.status, ThreadStatus::WaitingApproval);
    let replay = restarted
        .events_replay(events_replay_params(thread.id))
        .unwrap();
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ApprovalDecision { note, .. }
            if note.as_deref() == Some("desktop denied after restart")
    )));
}

#[tokio::test]
async fn approval_decision_denies_loaded_cold_pending_approval_after_resume() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("request approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_risky_loaded_cold".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;
    let approval_id = wait_for_approval_id(&service, &thread.id).await;

    let restarted = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        registry,
    );
    restarted
        .thread_resume(ThreadResumeParams {
            thread_id: thread.id.clone(),
            workspace_root: None,
            cwd: None,
        })
        .unwrap();

    let response = restarted
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id: approval_id.clone(),
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied after resume".into()),
            workspace_root: None,
        })
        .await
        .expect("loaded cold approval decision should be accepted");

    assert_eq!(response.approval_id, approval_id);

    let replay = restarted
        .events_replay(events_replay_params(thread.id))
        .unwrap();
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ApprovalDecision { note, .. }
            if note.as_deref() == Some("desktop denied after resume")
    )));
}

#[tokio::test]
async fn cold_approval_restore_uses_latest_matching_tool_call_id() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("first approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_reused".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "rm -rf scratch; printf 'first-wrong\\n'",
                        "persistent": true
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("first waiting".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("second approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_reused".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "rm -rf scratch; printf 'second-approved\\n'",
                        "persistent": true
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("second waiting".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let first_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "first risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;
    let first_approval_id = wait_for_approval_id(&service, &thread.id).await;
    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(first_turn.id),
            approval_id: first_approval_id.clone(),
            decision: ApprovalDecisionStatus::Denied,
            note: Some("deny first reused call".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let second_turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "second risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;
    let second_approval_id =
        wait_for_new_approval_id(&service, &thread.id, &first_approval_id).await;

    let restarted = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        registry,
    );
    restarted
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(second_turn.id),
            approval_id: second_approval_id,
            decision: ApprovalDecisionStatus::Approved,
            note: Some("approve second reused call".into()),
            workspace_root: None,
        })
        .await
        .expect("second reused call approval should be accepted");

    for _ in 0..200 {
        let replay = restarted
            .events_replay(events_replay_params(thread.id.clone()))
            .unwrap();
        let saw_second = replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ExecOutput { chunk, .. } if chunk.contains("second-approved")
            )
        });
        let saw_first = replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ExecOutput { chunk, .. } if chunk.contains("first-wrong")
            )
        });
        assert!(
            !saw_first,
            "cold restore executed the older reused tool call"
        );
        if saw_second {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for second reused command output");
}

#[tokio::test]
async fn cold_approval_restore_preserves_turn_context_cwd() {
    let dir = tempdir().unwrap();
    let initial_cwd = dir.path().join("initial");
    let turn_cwd = dir.path().join("turn-cwd");
    std::fs::create_dir_all(initial_cwd.join("scratch")).unwrap();
    std::fs::create_dir_all(turn_cwd.join("scratch")).unwrap();
    let turn_cwd = std::fs::canonicalize(&turn_cwd).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("approval with turn cwd".into()),
                tool_calls: vec![ToolCall {
                    id: "call_turn_cwd".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "rm -rf scratch; pwd; printf 'cwd-approved\\n'",
                        "persistent": true
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for cwd approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: Some("initial".into()),
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try turn cwd command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: Some(TurnContextOverrides {
                cwd: Some("turn-cwd".into()),
                model: None,
                thinking_mode: None,
                clear_thinking_mode: false,
            }),
        })
        .await
        .unwrap()
        .turn;
    let approval_id = wait_for_approval_id(&service, &thread.id).await;

    let restarted = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![])),
        registry,
    );
    restarted
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Approved,
            note: Some("approve turn cwd".into()),
            workspace_root: None,
        })
        .await
        .expect("turn cwd approval should be accepted after restart");

    for _ in 0..200 {
        let replay = restarted
            .events_replay(events_replay_params(thread.id.clone()))
            .unwrap();
        let output = replay
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                RuntimeEventKind::ExecOutput { chunk, .. } => Some(chunk.as_str()),
                _ => None,
            })
            .collect::<String>();
        if output.contains("cwd-approved") {
            assert!(
                output.contains(&turn_cwd.to_string_lossy().to_string()),
                "approved command did not run in turn cwd: {output}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for turn cwd approved output");
}

#[tokio::test]
async fn approval_decision_does_not_write_synthetic_tool_message_to_rollout_context() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("request approval".into()),
            tool_calls: vec![ToolCall {
                id: "call_risky_orphan_guard".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "rm -rf scratch" }),
                thought_signature: None,
            }],
            reasoning: vec![],
        }])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;

    let approval_id = wait_for_approval_id(&service, &thread.id).await;
    let approval_call_id = format!("approval_decision_{}", approval_id.as_str());

    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    let replay = service
        .events_replay(events_replay_params(thread.id.clone()))
        .unwrap();
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ApprovalDecision { note, .. }
            if note.as_deref() == Some("desktop denied")
    )));
    assert!(replay.events.iter().any(|event| matches!(
        &event.kind,
        RuntimeEventKind::ToolResult { result } if result.tool_call_id == approval_call_id
    )));

    let rollout_items = read_rollout_items(dir.path(), &thread.id);
    assert!(!rollout_items.iter().any(|item| matches!(
        item,
        RolloutItem::ResponseItem(response_item)
            if response_item.message.role == MessageRole::Tool
                && response_item.message.tool_call_id.as_deref() == Some(approval_call_id.as_str())
    )));
    let snapshot =
        exagent::state::rollout::snapshot_from_rollout_items(&thread.id, &rollout_items).unwrap();
    assert!(!snapshot.conversation.iter().any(|message| {
        message.role == MessageRole::Tool
            && message.tool_call_id.as_deref() == Some(approval_call_id.as_str())
    }));
}

#[tokio::test]
async fn approved_persistent_command_emits_lifecycle_and_live_output() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scratch")).unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![
            AssistantTurn {
                text: Some("request persistent approval".into()),
                tool_calls: vec![ToolCall {
                    id: "call_persistent_risky".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": "rm -rf scratch; sleep 0.1; printf 'approved-live\\n'",
                        "persistent": true
                    }),
                    thought_signature: None,
                }],
                reasoning: vec![],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
                reasoning: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
            permission_profile: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try persistent risky command".into(),
            workspace_root: None,
            turn_mode: Default::default(),
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;

    let approval_id = wait_for_approval_id(&service, &thread.id).await;
    let approval_call_id = format!("approval_decision_{}", approval_id.as_str());

    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Approved,
            note: Some("desktop approved".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    for _ in 0..200 {
        let replay = service
            .events_replay(events_replay_params(thread.id.clone()))
            .unwrap();
        let saw_started = replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ToolInvocationStarted { tool_call_id, .. }
                    if tool_call_id == &approval_call_id
            )
        });
        let saw_completed = replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ToolInvocationCompleted { tool_call_id, .. }
                    if tool_call_id == &approval_call_id
            )
        });
        let saw_output = replay.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ExecOutput { chunk, .. } if chunk.contains("approved-live")
            )
        });
        if saw_started && saw_completed && saw_output {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("timed out waiting for approved persistent command lifecycle and live output");
}

#[test]
fn approval_decision_params_deserialize_snake_case_status() {
    let value = serde_json::json!({
        "thread_id": "thread_1",
        "approval_id": "approval_1",
        "decision": "denied",
        "workspace_root": "."
    });
    let params: ApprovalDecisionParams = serde_json::from_value(value).unwrap();
    assert!(matches!(params.decision, ApprovalDecisionStatus::Denied));
}
