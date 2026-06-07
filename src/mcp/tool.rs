use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::mcp::manager::{McpDiscoveredTool, McpRuntimeManager};
use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};

pub struct McpToolHandler {
    server_id: String,
    remote_tool_name: String,
    spec: ToolSpec,
    manager: Arc<McpRuntimeManager>,
}

impl McpToolHandler {
    pub fn new(discovered: McpDiscoveredTool, manager: Arc<McpRuntimeManager>) -> Self {
        Self {
            server_id: discovered.server_id,
            remote_tool_name: discovered.tool_name,
            spec: discovered.spec,
            manager,
        }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        match self
            .manager
            .call_tool(
                &self.server_id,
                &self.remote_tool_name,
                call.arguments.clone(),
            )
            .await
        {
            Ok(output) => {
                let status = if output.is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                };
                ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status,
                    content: mcp_content_to_text(&output.content),
                    meta: output.meta,
                })
            }
            Err(err) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: format!("MCP tool call failed: {err:#}"),
                meta: None,
            }),
        }
    }
}

fn mcp_content_to_text(content: &[Value]) -> String {
    if content.is_empty() {
        return "[MCP tool returned no content]".to_string();
    }

    content
        .iter()
        .map(mcp_content_item_to_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn mcp_content_item_to_text(item: &Value) -> String {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => item
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| json_to_string(item)),
        Some("resource") => resource_content_to_text(item),
        Some("image") => {
            let mime = item
                .get("mimeType")
                .or_else(|| item.get("mime"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("[image: {mime}]")
        }
        _ => json_to_string(item),
    }
}

fn resource_content_to_text(item: &Value) -> String {
    if let Some(text) = item.pointer("/resource/text").and_then(Value::as_str) {
        return text.to_string();
    }

    if let Some(uri) = item.pointer("/resource/uri").and_then(Value::as_str) {
        return format!("[resource: {uri}]");
    }

    json_to_string(item)
}

fn json_to_string(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}
