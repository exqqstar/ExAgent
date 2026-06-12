use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::{Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::{resolve_workspace_path, ResolvedWorkspacePath};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteFileTool;

#[async_trait]
impl ToolHandler for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "write_file",
            "Write a UTF-8 text file in the workspace",
            serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: WriteFileArgs = match serde_json::from_value(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                    parts: Vec::new(),
                });
            }
        };

        match write_file(&ctx.config.workspace_root, &args) {
            Ok(resolved) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: format!("Wrote {}", resolved.canonical_path.display()),
                meta: Some(workspace_path_meta(&resolved)),
                parts: Vec::new(),
            }),
            Err(err) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err.to_string(),
                meta: None,
                parts: Vec::new(),
            }),
        }
    }
}

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
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.handle(invocation, ctx).await.model_result
    }
}

fn write_file(
    workspace_root: &std::path::Path,
    args: &WriteFileArgs,
) -> Result<ResolvedWorkspacePath, String> {
    let resolved =
        resolve_workspace_path(workspace_root, &args.path).map_err(|err| err.to_string())?;
    if let Some(parent) = resolved.canonical_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&resolved.canonical_path, args.content.as_bytes())
        .map_err(|err| err.to_string())?;
    Ok(resolved)
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
