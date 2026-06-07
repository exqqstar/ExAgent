use async_trait::async_trait;
use exagent::config::AgentConfig;
use exagent::events::{ExecOutputStream, RuntimeEventKind};
use exagent::exec_session::ExecSessionManager;
use exagent::policy::PolicyManager;
use exagent::registry::{ToolContext, ToolRegistry};
use exagent::session::ExecSessionId;
use exagent::tools::read_file::ReadFileTool;
use exagent::tools::{
    ToolCapabilities, ToolHandler, ToolInvocation, ToolModelOutput, ToolOutcome, ToolRuntimeEffect,
    ToolSpec,
};
use exagent::types::{ToolCall, ToolStatus};
use std::sync::Arc;
use std::sync::Mutex;

fn tool_test_context() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        config: AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        thread_id: None,
        turn_id: None,
        tool_invocation_id: None,
        exec_sessions: Arc::new(ExecSessionManager::default()),
        exec_output_sink: None,
        policy: Arc::new(PolicyManager::default()),
        agent_tool_policy: exagent::runtime::agent_profile::AgentToolPolicy::all(),
        mailbox_rx: None,
        goal_api: None,
    };
    (dir, ctx)
}

#[test]
fn function_tool_spec_projects_to_internal_schema() {
    let spec = ToolSpec::function(
        "read_file",
        "Read a UTF-8 file inside the workspace.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }),
    );

    let schema = spec.to_internal_schema();
    assert_eq!(schema["name"], "read_file");
    assert_eq!(schema["input_schema"]["required"][0], "path");
}

#[test]
fn tool_outcome_keeps_model_output_separate_from_effects() {
    let outcome = ToolOutcome::success(
        "call_1",
        "run_command",
        ToolModelOutput::text("exit_code: 0\nstdout:\nok"),
    )
    .with_effect(ToolRuntimeEffect::ExecSessionNotRunning {
        exec_session_id: ExecSessionId::new("exec_1"),
    });

    assert_eq!(outcome.model_result.content, "exit_code: 0\nstdout:\nok");
    assert_eq!(outcome.effects.len(), 1);
}

#[test]
fn tool_invocation_started_serializes_as_snake_case() {
    let event = RuntimeEventKind::ToolInvocationStarted {
        invocation_id: "inv_call_1".into(),
        tool_call_id: "call_1".into(),
        tool_name: "run_command".into(),
        mutating: true,
    };

    let value = serde_json::to_value(event).expect("serialize lifecycle event");
    assert_eq!(value["type"], "tool_invocation_started");
    assert_eq!(value["tool_call_id"], "call_1");
    assert_eq!(value["tool_name"], "run_command");
    assert_eq!(value["mutating"], true);
}

#[test]
fn tool_invocation_completed_serializes_status() {
    let event = RuntimeEventKind::ToolInvocationCompleted {
        invocation_id: "inv_call_1".into(),
        tool_call_id: "call_1".into(),
        tool_name: "run_command".into(),
        status: ToolStatus::Success,
    };

    let value = serde_json::to_value(event).expect("serialize lifecycle event");
    assert_eq!(value["type"], "tool_invocation_completed");
    assert_eq!(value["status"], "success");
}

#[test]
fn tool_invocation_lifecycle_variants_serialize_as_snake_case() {
    let waiting = RuntimeEventKind::ToolInvocationWaitingApproval {
        invocation_id: "inv_call_1".into(),
        approval_id: exagent::session::ApprovalId::new("approval_1"),
        reason: "review required".into(),
    };
    let output = RuntimeEventKind::ToolInvocationOutputDelta {
        invocation_id: "inv_call_1".into(),
        stream: ExecOutputStream::Stdout,
        chunk: "hello".into(),
        sequence: 7,
    };
    let failed = RuntimeEventKind::ToolInvocationFailed {
        invocation_id: "inv_call_1".into(),
        tool_call_id: "call_1".into(),
        tool_name: "run_command".into(),
        message: "failed".into(),
    };
    let cancelled = RuntimeEventKind::ToolInvocationCancelled {
        invocation_id: "inv_call_1".into(),
        tool_call_id: "call_1".into(),
        tool_name: "run_command".into(),
        reason: "interrupted".into(),
    };

    assert_eq!(
        serde_json::to_value(waiting).unwrap()["type"],
        "tool_invocation_waiting_approval"
    );
    assert_eq!(
        serde_json::to_value(output).unwrap()["type"],
        "tool_invocation_output_delta"
    );
    assert_eq!(
        serde_json::to_value(failed).unwrap()["type"],
        "tool_invocation_failed"
    );
    assert_eq!(
        serde_json::to_value(cancelled).unwrap()["type"],
        "tool_invocation_cancelled"
    );
}

#[tokio::test]
async fn registry_exposes_specs_and_returns_typed_outcomes() {
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool);

    let specs = registry.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "read_file");

    let (dir, ctx) = tool_test_context();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_1".to_string(),
                call: ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({ "path": "a.txt" }),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    assert_eq!(outcome.model_result.content, "hello");
    assert!(outcome.effects.is_empty());
}

struct ContextCaptureTool {
    seen_invocation_id: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl ToolHandler for ContextCaptureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "capture_context",
            "Capture tool context",
            serde_json::json!({}),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        *self.seen_invocation_id.lock().unwrap() = ctx.tool_invocation_id.clone();
        let call = invocation.call;
        ToolOutcome::success(call.id, call.name, ToolModelOutput::text("ok"))
    }
}

#[tokio::test]
async fn execute_outcome_exposes_invocation_id_to_handlers() {
    let seen_invocation_id = Arc::new(Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(ContextCaptureTool {
        seen_invocation_id: seen_invocation_id.clone(),
    });
    let (_dir, ctx) = tool_test_context();

    let outcome = registry
        .execute_outcome(
            ToolInvocation {
                invocation_id: "inv_visible".to_string(),
                call: ToolCall {
                    id: "call_capture".into(),
                    name: "capture_context".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                },
            },
            &ctx,
        )
        .await;

    assert_eq!(outcome.model_result.content, "ok");
    assert_eq!(
        seen_invocation_id.lock().unwrap().as_deref(),
        Some("inv_visible")
    );
}
