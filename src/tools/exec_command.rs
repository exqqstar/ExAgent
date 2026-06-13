use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::registry::ToolContext;
use crate::tools::run_command::{handle_run_command_args, RunCommandArgs};
use crate::tools::{Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolCall, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecCommandArgs {
    pub cmd: String,
    #[serde(alias = "workdir")]
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
    pub persistent: Option<bool>,
}

pub struct ExecCommandTool;

#[async_trait]
impl ToolHandler for ExecCommandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "exec_command",
            "Run a shell command inside the workspace, returning output or an exec_session_id for persistent commands",
            serde_json::to_value(schemars::schema_for!(ExecCommandArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<ExecCommandArgs>(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => return argument_error(call, err.to_string()),
        };
        let run_args = RunCommandArgs {
            command: Some(args.cmd),
            cwd: args.cwd,
            timeout_secs: args.timeout_secs,
            persistent: args.persistent,
            exec_session_id: None,
            stdin: None,
            terminate: None,
            approval_id: None,
            decision: None,
        };

        handle_run_command_args(call, run_args, ctx, "exec_command").await
    }
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn name(&self) -> &'static str {
        "exec_command"
    }

    fn description(&self) -> &'static str {
        "Run a shell command inside the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(ExecCommandArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.handle(invocation, ctx).await.model_result
    }
}

fn argument_error(call: ToolCall, message: String) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id: call.id,
        tool_name: call.name,
        status: crate::types::ToolStatus::Error,
        content: message,
        meta: None,
        parts: Vec::new(),
    })
}
