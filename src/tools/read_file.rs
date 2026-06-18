use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};
use crate::workspace::{resolve_readable_path, ResolvedWorkspacePath};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// Workspace-relative (or workspace-scoped absolute) path to the UTF-8 text file to read.
    pub path: String,
    /// Optional 1-based line number to start from, inclusive. Defaults to the first line.
    pub start_line: Option<usize>,
    /// Optional 1-based line number to stop at, inclusive. Defaults to the last line.
    pub end_line: Option<usize>,
}

pub struct ReadFileTool;

#[async_trait]
impl ToolHandler for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "read_file",
            "Read a UTF-8 text file from the workspace or a configured skill root",
            serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap(),
        )
        // Internal contract: describes the structured `meta` side-channel this
        // tool emits (model-facing content is the file text). See ADR-0042.
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Canonical resolved path of the file that was read." },
                "requested_path": { "type": "string", "description": "Path exactly as requested by the caller." },
                "normalized_path": { "type": "string", "description": "Normalized form of the requested path." },
                "canonical_path": { "type": "string", "description": "Fully canonicalized filesystem path." },
                "was_absolute": { "type": "boolean", "description": "Whether the requested path was absolute." }
            },
            "required": ["path", "requested_path", "normalized_path", "canonical_path", "was_absolute"],
            "additionalProperties": false
        }))
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args: ReadFileArgs = match serde_json::from_value(call.arguments) {
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

        match read_file(
            &ctx.config.workspace_root,
            &ctx.config.skills_user_roots,
            &args,
        ) {
            Ok((resolved, content)) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content,
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

fn read_file(
    workspace_root: &std::path::Path,
    extra_read_roots: &[std::path::PathBuf],
    args: &ReadFileArgs,
) -> Result<(ResolvedWorkspacePath, String), String> {
    let resolved = resolve_readable_path(workspace_root, extra_read_roots, &args.path)
        .map_err(|err| err.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_schema_carries_per_field_descriptions() {
        let spec = ReadFileTool.spec();
        let crate::tools::ToolSpecKind::Function { input_schema } = &spec.kind;
        let props = input_schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("schema has properties");
        for field in ["path", "start_line", "end_line"] {
            let desc = props
                .get(field)
                .and_then(|f| f.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !desc.is_empty(),
                "field `{field}` should carry a description in the derived schema"
            );
        }
    }

    #[test]
    fn output_schema_required_matches_emitted_meta_keys() {
        let spec = ReadFileTool.spec();
        let output_schema = spec
            .output_schema
            .expect("read_file declares output_schema");
        let required: Vec<&str> = output_schema["required"]
            .as_array()
            .expect("required is an array")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // Keys the handler actually emits in `meta` (workspace_path_meta).
        let mut expected = vec![
            "path",
            "requested_path",
            "normalized_path",
            "canonical_path",
            "was_absolute",
        ];
        let mut got = required.clone();
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got, expected);
    }
}
