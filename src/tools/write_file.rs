use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::resolve_workspace_path;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write a UTF-8 text file in the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let args: WriteFileArgs = match serde_json::from_value(call.arguments) {
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

        match write_file(&ctx.config.workspace_root, &args) {
            Ok(path) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: format!("Wrote {}", path.display()),
                meta: Some(json!({ "path": path })),
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

fn write_file(
    workspace_root: &std::path::Path,
    args: &WriteFileArgs,
) -> Result<std::path::PathBuf, String> {
    let path = resolve_workspace_path(workspace_root, &args.path).map_err(|err| err.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&path, args.content.as_bytes()).map_err(|err| err.to_string())?;
    Ok(path)
}
