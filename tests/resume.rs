use std::path::PathBuf;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::llm::{LlmClient, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::session::{
    AgentRole, ApprovalId, ApprovalStatus, CompactionSummary, ExecSessionId, ExecSessionRef,
    ExecSessionStatus, PendingApproval, SessionSnapshot,
};
use exagent::tools::write_file::WriteFileTool;
use exagent::types::{
    AssistantTurn, ConversationMessage, EventId, MessageRole, SessionId, ToolCall, ToolResult,
    ToolStatus, TurnId,
};
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[test]
fn session_snapshot_round_trips_to_json() {
    let snapshot = SessionSnapshot {
        session_id: SessionId::new("session_123"),
        parent_session_id: None,
        root_session_id: SessionId::new("session_123"),
        spawned_by_turn_id: Some(TurnId::new("turn_parent_1")),
        agent_role: AgentRole::Implementation,
        workspace_root: PathBuf::from("/tmp/exagent"),
        cwd: PathBuf::from("/tmp/exagent/src"),
        conversation: vec![
            ConversationMessage::user("resume the task"),
            ConversationMessage::assistant(Some("running tests".to_string()), vec![]),
        ],
        open_exec_sessions: vec![ExecSessionRef {
            exec_session_id: ExecSessionId::new("exec_1"),
            command: "cargo test".into(),
            cwd: PathBuf::from("/tmp/exagent"),
            status: ExecSessionStatus::Running,
        }],
        latest_compaction: Some(CompactionSummary {
            summary: "tests were running before interruption".into(),
            source_event_ids: vec![EventId::new("evt_1"), EventId::new("evt_2")],
        }),
        pending_approvals: vec![PendingApproval {
            approval_id: ApprovalId::new("approval_1"),
            requested_event_id: EventId::new("evt_3"),
            tool_name: "run_command".into(),
            reason: "command matched risky pattern".into(),
            status: ApprovalStatus::Pending,
        }],
    };

    let value = serde_json::to_value(&snapshot).unwrap();

    assert_eq!(value["session_id"], "session_123");
    assert_eq!(value["root_session_id"], "session_123");
    assert_eq!(value["spawned_by_turn_id"], "turn_parent_1");
    assert_eq!(value["agent_role"], "implementation");
    assert_eq!(value["workspace_root"], "/tmp/exagent");
    assert_eq!(value["cwd"], "/tmp/exagent/src");
    assert_eq!(value["open_exec_sessions"][0]["exec_session_id"], "exec_1");
    assert_eq!(value["pending_approvals"][0]["approval_id"], "approval_1");

    let decoded: SessionSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn runtime_event_round_trips_to_json_with_stable_ids() {
    let event = RuntimeEvent {
        event_id: EventId::new("evt_tool_1"),
        session_id: SessionId::new("session_123"),
        turn_id: Some(TurnId::new("turn_7")),
        kind: RuntimeEventKind::AssistantTurn {
            turn: AssistantTurn {
                text: Some("writing file".into()),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "write_file".into(),
                    arguments: json!({ "path": "src/lib.rs", "content": "pub mod agent;" }),
                }],
            },
        },
    };

    let value = serde_json::to_value(&event).unwrap();

    assert_eq!(value["event_id"], "evt_tool_1");
    assert_eq!(value["session_id"], "session_123");
    assert_eq!(value["turn_id"], "turn_7");
    assert_eq!(value["kind"]["type"], "assistant_turn");
    assert_eq!(value["kind"]["turn"]["tool_calls"][0]["id"], "call_1");

    let decoded: RuntimeEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn tool_result_event_preserves_result_payload() {
    let event = RuntimeEvent {
        event_id: EventId::new("evt_tool_result_1"),
        session_id: SessionId::new("session_123"),
        turn_id: Some(TurnId::new("turn_8")),
        kind: RuntimeEventKind::ToolResult {
            result: ToolResult {
                tool_call_id: "call_2".into(),
                tool_name: "run_command".into(),
                status: ToolStatus::Success,
                content: "stdout:\nok\n\nstderr:\n".into(),
                meta: Some(json!({
                    "exit_code": 0,
                    "cwd": "/tmp/exagent",
                })),
            },
        },
    };

    let value = serde_json::to_value(&event).unwrap();

    assert_eq!(value["kind"]["type"], "tool_result");
    assert_eq!(value["kind"]["result"]["tool_call_id"], "call_2");
    assert_eq!(value["kind"]["result"]["meta"]["exit_code"], 0);

    let decoded: RuntimeEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, event);
}

#[tokio::test]
async fn agent_persists_snapshot_and_event_log_for_new_session() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("writing".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "out.txt", "content": "hello"}),
            }],
        },
        AssistantTurn {
            text: Some("done".into()),
            tool_calls: vec![],
        },
    ]);

    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), registry);
    let output = agent.run_with_meta("create a file").await.unwrap();

    assert_eq!(output.final_turn.text.as_deref(), Some("done"));
    assert!(output.snapshot_path.exists());
    assert!(output.events_path.exists());

    let snapshot: SessionSnapshot = exagent::transcript::read_json(&output.snapshot_path).unwrap();
    assert_eq!(snapshot.session_id, output.session_id);
    assert_eq!(snapshot.conversation.len(), 4);
    assert!(matches!(snapshot.conversation[0].role, MessageRole::User));
    assert_eq!(snapshot.conversation[0].content, "create a file");
    assert!(matches!(
        snapshot.conversation.last().unwrap().role,
        MessageRole::Assistant
    ));

    let events = exagent::transcript::read_session_events(dir.path(), &output.session_id).unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(
        events[1].kind,
        RuntimeEventKind::ToolResult { .. }
    ));
    assert!(matches!(
        events[2].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
}

#[tokio::test]
async fn agent_can_resume_existing_session_with_prior_context() {
    let dir = tempdir().unwrap();
    let llm = ResumeInspectingLlm::default();

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let first = agent.run_with_meta("start phase2").await.unwrap();
    let resumed = agent
        .resume(&first.session_id, "continue phase2")
        .await
        .unwrap();

    assert_eq!(resumed.session_id, first.session_id);
    assert_eq!(resumed.final_turn.text.as_deref(), Some("second response"));

    let snapshot: SessionSnapshot = exagent::transcript::read_json(&resumed.snapshot_path).unwrap();
    assert_eq!(snapshot.conversation.len(), 4);
    assert_eq!(snapshot.conversation[0].content, "start phase2");
    assert_eq!(snapshot.conversation[1].content, "first response");
    assert_eq!(snapshot.conversation[2].content, "continue phase2");
    assert_eq!(snapshot.conversation[3].content, "second response");
}

#[tokio::test]
async fn replay_reads_persisted_events_without_rerunning_tools() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![AssistantTurn {
        text: Some("done".into()),
        tool_calls: vec![],
    }]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let output = agent.run_with_meta("record this").await.unwrap();
    exagent::transcript::record_session_spawn(
        dir.path(),
        &output.session_id,
        &SessionId::new("session_child"),
        AgentRole::Spec,
        Some(&TurnId::new("turn_1")),
    )
    .unwrap();
    let replay = exagent::transcript::replay_session(dir.path(), &output.session_id).unwrap();

    assert_eq!(replay.len(), 2);
    assert_eq!(replay[0].event_id.as_str(), "evt_1");
    assert_eq!(replay[1].event_id.as_str(), "evt_2");
    assert!(matches!(
        replay[0].kind,
        RuntimeEventKind::AssistantTurn { .. }
    ));
    assert!(matches!(
        &replay[1].kind,
        RuntimeEventKind::SessionSpawned {
            child_session_id,
            parent_session_id,
            agent_role: AgentRole::Spec,
            spawned_by_turn_id: Some(turn_id),
        } if child_session_id == &SessionId::new("session_child")
            && parent_session_id == &output.session_id
            && turn_id == &TurnId::new("turn_1")
    ));
}

#[tokio::test]
async fn new_root_session_defaults_to_primary_role_and_own_root_id() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![AssistantTurn {
        text: Some("done".into()),
        tool_calls: vec![],
    }]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let output = agent.run_with_meta("start phase3").await.unwrap();
    let snapshot: SessionSnapshot = exagent::transcript::read_json(&output.snapshot_path).unwrap();

    assert_eq!(snapshot.parent_session_id, None);
    assert_eq!(snapshot.spawned_by_turn_id, None);
    assert_eq!(snapshot.root_session_id, snapshot.session_id);
    assert_eq!(snapshot.agent_role, AgentRole::Primary);
}

#[tokio::test]
async fn collect_returns_latest_resumed_child_output() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("child draft".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("child revised".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let parent = agent.run_with_meta("lead phase3").await.unwrap();
    let child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Spec,
            "draft goals",
            Some(&TurnId::new("turn_1")),
        )
        .await
        .unwrap();
    let resumed = agent
        .resume(&child.session_id, "revise the draft")
        .await
        .unwrap();

    let collected = agent.collect_session(&child.session_id).unwrap();

    assert_eq!(resumed.session_id, child.session_id);
    assert_eq!(
        collected
            .latest_useful_output
            .as_ref()
            .map(|output| output.content.as_str()),
        Some("child revised")
    );
}

#[tokio::test]
async fn collect_returns_latest_structured_result_after_resume() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("record first spec".into()),
            tool_calls: vec![structured_result_tool_call("first spec summary")],
        },
        AssistantTurn {
            text: Some("child draft".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("record revised spec".into()),
            tool_calls: vec![structured_result_tool_call("revised spec summary")],
        },
        AssistantTurn {
            text: Some("child revised".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), exagent::default_tool_registry());
    let parent = agent.run_with_meta("lead phase3").await.unwrap();
    let child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Spec,
            "draft goals",
            Some(&TurnId::new("turn_1")),
        )
        .await
        .unwrap();
    agent
        .resume(&child.session_id, "revise the draft")
        .await
        .unwrap();

    let collected = agent.collect_session(&child.session_id).unwrap();

    assert_eq!(
        collected
            .structured_result
            .as_ref()
            .map(|result| result.summary.as_str()),
        Some("revised spec summary")
    );
    assert_eq!(
        collected
            .structured_result
            .as_ref()
            .and_then(|result| result.source_turn_id.as_ref())
            .map(TurnId::as_str),
        Some("turn_3")
    );
    assert_eq!(
        collected
            .latest_useful_output
            .as_ref()
            .map(|output| output.content.as_str()),
        Some("child revised")
    );
}

#[derive(Default)]
struct ResumeInspectingLlm {
    calls: Mutex<usize>,
}

#[async_trait]
impl LlmClient for ResumeInspectingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        let mut calls = self.calls.lock().await;
        *calls += 1;

        match *calls {
            1 => {
                assert_eq!(messages.len(), 1);
                assert!(matches!(messages[0].role, MessageRole::User));
                assert_eq!(messages[0].content, "start phase2");

                Ok(AssistantTurn {
                    text: Some("first response".into()),
                    tool_calls: vec![],
                })
            }
            2 => {
                assert_eq!(messages.len(), 3);
                assert_eq!(messages[0].content, "start phase2");
                assert!(matches!(messages[0].role, MessageRole::User));
                assert_eq!(messages[1].content, "first response");
                assert!(matches!(messages[1].role, MessageRole::Assistant));
                assert_eq!(messages[2].content, "continue phase2");
                assert!(matches!(messages[2].role, MessageRole::User));

                Ok(AssistantTurn {
                    text: Some("second response".into()),
                    tool_calls: vec![],
                })
            }
            _ => Err(anyhow!("unexpected extra llm call")),
        }
    }
}

fn structured_result_tool_call(summary: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{}", summary.replace(' ', "_")),
        name: "record_structured_result".into(),
        arguments: json!({
            "summary": summary,
            "assumptions": ["P1 is stable"],
            "risks": ["scope creep"],
            "open_questions": ["none"],
            "payload": {
                "kind": "spec",
                "goals": ["add structured contracts"],
                "non_goals": ["no planner"],
                "acceptance_criteria": ["collect returns typed result"],
                "contract_boundaries": ["inspect stays topology-only"]
            }
        }),
    }
}
