use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::registry::ToolContext;
use crate::tools::run_command::{handle_run_command_args, RunCommandArgs};
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolCall, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteStdinArgs {
    pub exec_session_id: String,
    #[serde(default, alias = "stdin")]
    pub chars: String,
    pub terminate: Option<bool>,
}

pub struct WriteStdinTool;

#[async_trait]
impl ToolHandler for WriteStdinTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "write_stdin",
            "Write characters to an existing exec session, poll it with empty chars, or terminate it",
            serde_json::to_value(schemars::schema_for!(WriteStdinArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<WriteStdinArgs>(call.arguments.clone()) {
            Ok(args) => args,
            Err(err) => return argument_error(call, err.to_string()),
        };
        let run_args = RunCommandArgs {
            command: None,
            cwd: None,
            timeout_secs: None,
            persistent: None,
            exec_session_id: Some(args.exec_session_id),
            stdin: (!args.chars.is_empty()).then_some(args.chars),
            terminate: args.terminate,
            approval_id: None,
            decision: None,
        };

        handle_run_command_args(call, run_args, ctx, "write_stdin").await
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
