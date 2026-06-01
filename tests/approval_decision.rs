use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionStatus, EventsReplayParams, ThreadReadParams,
    ThreadStartParams, ThreadStatus, TurnStartParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::MockLlm;
use exagent::policy::PolicyMode;
use exagent::registry::ToolRegistry;
use exagent::tools::run_command::RunCommandTool;
use exagent::types::{AssistantTurn, ToolCall};
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
                }],
            },
            AssistantTurn {
                text: Some("waiting for approval".into()),
                tool_calls: vec![],
            },
        ])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;

    let approval_id = loop {
        let replay = service
            .events_replay(events_replay_params(thread.id.clone()))
            .unwrap();
        if let Some(id) = replay.events.iter().find_map(|event| match &event.kind {
            RuntimeEventKind::ApprovalRequested { approval_id, .. } => Some(approval_id.clone()),
            _ => None,
        }) {
            break id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

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
