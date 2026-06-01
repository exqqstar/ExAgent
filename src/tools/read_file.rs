use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::{resolve_workspace_path, ResolvedWorkspacePath};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 text file from the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let args: ReadFileArgs = match serde_json::from_value(call.arguments) {
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

        match read_file(&ctx.config.workspace_root, &args) {
            Ok((resolved, content)) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content,
                meta: Some(workspace_path_meta(&resolved)),
            },
            Err(err) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
            },
        }
    }
}

fn read_file(
    workspace_root: &std::path::Path,
    args: &ReadFileArgs,
) -> Result<(ResolvedWorkspacePath, String), String> {
    let resolved =
        resolve_workspace_path(workspace_root, &args.path).map_err(|err| err.to_string())?;
    let body = std::fs::read_to_string(&resolved.canonical_path).map_err(|err| err.to_string())?;
    let start_line = args.start_line.unwrap_or(1);
    let end_line = args.end_line.unwrap_or(usize::MAX);
    let content = body
        .lines()
        .enumerate()
        .filter(|(index, _)| {
            let line_no = index + 1;
            line_no >= start_line && line_no <= end_line
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n");

    Ok((resolved, content))
}

fn workspace_path_meta(resolved: &ResolvedWorkspacePath) -> Value {
    json!({
        "path": resolved.canonical_path,
        "requested_path": resolved.requested_path,
        "normalized_path": resolved.normalized_path,
        "canonical_path": resolved.canonical_path,
        "was_absolute": resolved.was_absolute,
    })
}
