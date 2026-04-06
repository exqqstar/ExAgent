use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunCommandArgs {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Run a shell command inside the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(RunCommandArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let args: RunCommandArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                };
            }
        };

        match run_command(&args, ctx).await {
            Ok(result) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: result.status,
                content: result.content,
                meta: Some(result.meta),
            },
            Err(err) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err,
                meta: None,
            },
        }
    }
}

struct CommandOutcome {
    status: ToolStatus,
    content: String,
    meta: Value,
}

async fn run_command(args: &RunCommandArgs, ctx: &ToolContext) -> Result<CommandOutcome, String> {
    let cwd = resolve_cwd(args, ctx)?;
    let timeout_secs = args.timeout_secs.unwrap_or(ctx.config.command_timeout_secs);

    let mut command = Command::new("sh");
    command.arg("-lc").arg(&args.command);
    command.current_dir(&cwd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);

    let child = command.spawn().map_err(|err| err.to_string())?;
    let wait = timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

    match wait {
        Ok(Ok(output)) => {
            let stdout = truncate_utf8(
                &String::from_utf8_lossy(&output.stdout),
                ctx.config.max_output_bytes,
            );
            let stderr = truncate_utf8(
                &String::from_utf8_lossy(&output.stderr),
                ctx.config.max_output_bytes,
            );
            let status = if output.status.success() {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };

            Ok(CommandOutcome {
                status,
                content: format!("stdout:\n{}\n\nstderr:\n{}", stdout, stderr),
                meta: json!({
                    "exit_code": output.status.code(),
                    "stdout": stdout,
                    "stderr": stderr,
                    "timed_out": false,
                    "cwd": cwd,
                }),
            })
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Ok(CommandOutcome {
            status: ToolStatus::Error,
            content: "Command timed out".into(),
            meta: json!({
                "exit_code": Value::Null,
                "stdout": "",
                "stderr": "",
                "timed_out": true,
                "cwd": cwd,
            }),
        }),
    }
}

fn resolve_cwd(args: &RunCommandArgs, ctx: &ToolContext) -> Result<PathBuf, String> {
    match &args.cwd {
        Some(raw) => {
            resolve_workspace_path(&ctx.config.workspace_root, raw).map_err(|err| err.to_string())
        }
        None => Ok(ctx.config.cwd.clone()),
    }
}

fn truncate_utf8(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_string();
    }

    let mut end = 0;
    for (idx, ch) in output.char_indices() {
        if idx + ch.len_utf8() > max_bytes {
            break;
        }
        end = idx + ch.len_utf8();
    }
    output[..end].to_string()
}
