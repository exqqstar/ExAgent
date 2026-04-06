use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use crate::config::AgentConfig;
use crate::tools::Tool;
use crate::types::{ToolCall, ToolResult, ToolStatus};

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub config: AgentConfig,
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }

    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "input_schema": tool.input_schema(),
                })
            })
            .collect()
    }

    pub async fn execute(&self, call: ToolCall, ctx: Option<&ToolContext>) -> ToolResult {
        match (self.tools.get(&call.name), ctx) {
            (Some(tool), Some(ctx)) => tool.execute(call, ctx).await,
            (Some(_), None) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: "Tool context missing".into(),
                meta: None,
            },
            (None, _) => ToolResult {
                tool_call_id: call.id,
                tool_name: call.name.clone(),
                status: ToolStatus::Error,
                content: format!("Unknown tool: {}", call.name),
                meta: None,
            },
        }
    }
}
