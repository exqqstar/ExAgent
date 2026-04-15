use std::path::{Path, PathBuf};
use std::sync::Arc;

use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::events::{RuntimeEvent, RuntimeEventKind};
use exagent::exec_session::ExecSessionManager;
use exagent::llm::MockLlm;
use exagent::orchestration::collect_session;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::result_contract::{
    StructuredResultPayload, StructuredSessionResult, STRUCTURED_RESULT_SCHEMA_VERSION,
};
use exagent::session::{AgentRole, SessionSnapshot};
use exagent::types::{AssistantTurn, ConversationMessage, EventId, SessionId, ToolCall, TurnId};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn structured_result_event_round_trips() {
    let result = sample_spec_result(
        SessionId::new("session_spec"),
        Some(SessionId::new("session_parent")),
        Some(TurnId::new("turn_2")),
    );
    let event = RuntimeEvent {
        event_id: EventId::new("evt_4"),
        session_id: SessionId::new("session_spec"),
        turn_id: Some(TurnId::new("turn_2")),
        kind: RuntimeEventKind::StructuredResultRecorded {
            result: result.clone(),
        },
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["kind"]["type"], "structured_result_recorded");
    assert_eq!(value["kind"]["result"]["schema_version"], "phase3_p2/v1");
    assert_eq!(value["kind"]["result"]["agent_role"], "spec");
    assert_eq!(value["kind"]["result"]["payload"]["kind"], "spec");

    let decoded: RuntimeEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn transcript_replays_latest_structured_result() {
    let dir = tempdir().unwrap();
    let session_id = SessionId::new("session_test");

    exagent::transcript::record_structured_result(
        dir.path(),
        &session_id,
        Some(&TurnId::new("turn_1")),
        sample_test_result(
            session_id.clone(),
            Some(SessionId::new("session_parent")),
            Some(TurnId::new("turn_1")),
            "first matrix",
        ),
    )
    .unwrap();
    exagent::transcript::record_structured_result(
        dir.path(),
        &session_id,
        Some(&TurnId::new("turn_3")),
        sample_test_result(
            session_id.clone(),
            Some(SessionId::new("session_parent")),
            Some(TurnId::new("turn_3")),
            "revised matrix",
        ),
    )
    .unwrap();

    let replay = exagent::transcript::replay_session(dir.path(), &session_id).unwrap();
    let latest = exagent::transcript::latest_structured_result(dir.path(), &session_id).unwrap();

    assert_eq!(replay.len(), 2);
    assert!(matches!(
        replay[0].kind,
        RuntimeEventKind::StructuredResultRecorded { .. }
    ));
    assert_eq!(latest.unwrap().summary, "revised matrix");
}

#[tokio::test]
async fn tool_records_structured_result_for_matching_role() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("parent ready".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("recording spec result".into()),
            tool_calls: vec![ToolCall {
                id: "call_result".into(),
                name: "record_structured_result".into(),
                arguments: spec_tool_args("spec summary"),
            }],
        },
        AssistantTurn {
            text: Some("spec done".into()),
            tool_calls: vec![],
        },
    ]);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), exagent::default_tool_registry());
    let parent = agent.run_with_meta("lead").await.unwrap();
    let child = agent
        .fork_session(
            &parent.session_id,
            AgentRole::Spec,
            "draft the spec",
            Some(&TurnId::new("turn_1")),
        )
        .await
        .unwrap();

    let collected = agent.collect_session(&child.session_id).unwrap();
    let structured_result = collected.structured_result.expect("structured result");

    assert_eq!(
        structured_result.schema_version,
        STRUCTURED_RESULT_SCHEMA_VERSION
    );
    assert_eq!(structured_result.agent_role, AgentRole::Spec);
    assert_eq!(structured_result.session_id, child.session_id);
    assert_eq!(structured_result.parent_session_id, Some(parent.session_id));
    assert_eq!(
        structured_result.source_turn_id,
        Some(TurnId::new("turn_1"))
    );
    assert_eq!(structured_result.summary, "spec summary");
    assert!(matches!(
        structured_result.payload,
        StructuredResultPayload::Spec {
            ref goals,
            ref non_goals,
            ref acceptance_criteria,
            ref contract_boundaries,
        } if goals == &vec!["add structured contracts".to_string()]
            && non_goals == &vec!["no planner".to_string()]
            && acceptance_criteria == &vec!["collect returns typed result".to_string()]
            && contract_boundaries == &vec!["inspect stays topology-only".to_string()]
    ));
}

#[tokio::test]
async fn tool_rejects_role_mismatch_without_persisting_result() {
    let dir = tempdir().unwrap();
    let session_id = SessionId::new("session_judge");
    write_snapshot(
        dir.path(),
        &SessionSnapshot {
            session_id: session_id.clone(),
            parent_session_id: Some(SessionId::new("session_parent")),
            root_session_id: SessionId::new("session_parent"),
            spawned_by_turn_id: Some(TurnId::new("turn_1")),
            agent_role: AgentRole::Judge,
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            conversation: vec![ConversationMessage::user("review the plan")],
            open_exec_sessions: vec![],
            latest_compaction: None,
            pending_approvals: vec![],
        },
    );

    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        session_id: Some(session_id.clone()),
        turn_id: Some(TurnId::new("turn_2")),
        exec_sessions: Arc::new(ExecSessionManager::default()),
        policy: Arc::new(PolicyManager::default()),
    };

    let mut registry = ToolRegistry::new();
    registry.register(exagent::tools::record_structured_result::RecordStructuredResultTool);

    let result = registry
        .execute(
            ToolCall {
                id: "call_result".into(),
                name: "record_structured_result".into(),
                arguments: spec_tool_args("mismatched summary"),
            },
            Some(&ctx),
        )
        .await;

    assert_eq!(result.status.as_str(), "error");
    assert!(result.content.contains("does not match session role"));
    assert!(
        exagent::transcript::latest_structured_result(dir.path(), &session_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn collect_returns_structured_result_when_present() {
    let dir = tempdir().unwrap();
    let workspace_root = dir.path().to_path_buf();
    let parent = root_snapshot("session_parent", &workspace_root, "lead phase3");
    let mut child = child_snapshot(
        "session_spec_child",
        &parent.session_id,
        &parent.root_session_id,
        AgentRole::Spec,
        "draft goals",
        Some(TurnId::new("turn_1")),
    );
    child.conversation.push(ConversationMessage::assistant(
        Some("free-form summary".into()),
        vec![],
    ));

    write_snapshot(dir.path(), &parent);
    write_snapshot(dir.path(), &child);
    exagent::transcript::record_session_spawn(
        dir.path(),
        &parent.session_id,
        &child.session_id,
        AgentRole::Spec,
        child.spawned_by_turn_id.as_ref(),
    )
    .unwrap();
    exagent::transcript::record_structured_result(
        dir.path(),
        &child.session_id,
        Some(&TurnId::new("turn_1")),
        sample_spec_result(
            child.session_id.clone(),
            Some(parent.session_id.clone()),
            Some(TurnId::new("turn_1")),
        ),
    )
    .unwrap();

    let collected = collect_session(dir.path(), &child.session_id).unwrap();

    assert_eq!(
        collected
            .structured_result
            .as_ref()
            .map(|result| result.summary.as_str()),
        Some("spec summary")
    );
    assert_eq!(
        collected
            .latest_useful_output
            .as_ref()
            .map(|output| output.content.as_str()),
        Some("free-form summary")
    );
}

fn sample_spec_result(
    session_id: SessionId,
    parent_session_id: Option<SessionId>,
    source_turn_id: Option<TurnId>,
) -> StructuredSessionResult {
    StructuredSessionResult {
        schema_version: STRUCTURED_RESULT_SCHEMA_VERSION.into(),
        agent_role: AgentRole::Spec,
        session_id,
        parent_session_id,
        source_turn_id,
        summary: "spec summary".into(),
        assumptions: vec!["P1 is stable".into()],
        risks: vec!["scope creep".into()],
        open_questions: vec!["none".into()],
        payload: StructuredResultPayload::Spec {
            goals: vec!["add structured contracts".into()],
            non_goals: vec!["no planner".into()],
            acceptance_criteria: vec!["collect returns typed result".into()],
            contract_boundaries: vec!["inspect stays topology-only".into()],
        },
    }
}

fn sample_test_result(
    session_id: SessionId,
    parent_session_id: Option<SessionId>,
    source_turn_id: Option<TurnId>,
    summary: &str,
) -> StructuredSessionResult {
    StructuredSessionResult {
        schema_version: STRUCTURED_RESULT_SCHEMA_VERSION.into(),
        agent_role: AgentRole::Test,
        session_id,
        parent_session_id,
        source_turn_id,
        summary: summary.into(),
        assumptions: vec!["P1 collect exists".into()],
        risks: vec!["regression drift".into()],
        open_questions: vec![],
        payload: StructuredResultPayload::Test {
            regression_risks: vec!["collect precedence".into()],
            test_matrix: vec!["replay survives restart".into()],
            coverage_gaps: vec!["no long-running eval yet".into()],
        },
    }
}

fn spec_tool_args(summary: &str) -> serde_json::Value {
    json!({
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
    })
}

fn root_snapshot(session_id: &str, workspace_root: &Path, prompt: &str) -> SessionSnapshot {
    SessionSnapshot::new_root(
        SessionId::new(session_id),
        workspace_root.to_path_buf(),
        workspace_root.to_path_buf(),
        prompt,
    )
}

fn child_snapshot(
    session_id: &str,
    parent_session_id: &SessionId,
    root_session_id: &SessionId,
    agent_role: AgentRole,
    prompt: &str,
    spawned_by_turn_id: Option<TurnId>,
) -> SessionSnapshot {
    SessionSnapshot {
        session_id: SessionId::new(session_id),
        parent_session_id: Some(parent_session_id.clone()),
        root_session_id: root_session_id.clone(),
        spawned_by_turn_id,
        agent_role,
        workspace_root: PathBuf::new(),
        cwd: PathBuf::new(),
        conversation: vec![ConversationMessage::user(prompt)],
        open_exec_sessions: vec![],
        latest_compaction: None,
        pending_approvals: vec![],
    }
}

fn write_snapshot(workspace_root: &Path, snapshot: &SessionSnapshot) {
    let mut snapshot = snapshot.clone();
    snapshot.workspace_root = workspace_root.to_path_buf();
    snapshot.cwd = workspace_root.to_path_buf();
    let paths = exagent::transcript::session_paths(workspace_root, &snapshot.session_id);
    exagent::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
}
