use async_trait::async_trait;
use serde_json::Value;

use crate::types::{ToolCall, ToolResult};

pub mod read_file;
pub mod registry;
pub mod run_command;
pub mod write_file;

use registry::ToolContext;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult;
}
