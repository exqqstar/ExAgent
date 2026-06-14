use async_trait::async_trait;
use serde_json::Value;

use crate::types::{ToolCall, ToolResult};

pub mod apply_patch;
pub mod ask_user;
pub mod close_agent;
pub(crate) mod defer_question;
pub mod exec_command;
pub mod followup_task;
pub mod goal;
pub mod handler;
pub mod list_agents;
pub mod list_dir;
pub(crate) mod memory_common;
pub mod memory_forget;
pub mod memory_list;
pub mod memory_recall;
pub mod memory_save;
pub mod memory_update;
pub mod outcome;
pub(crate) mod output_projection;
pub mod read_file;
pub mod registry;
pub mod run_command;
pub mod search_files;
pub mod send_message;
pub mod spawn_agent;
pub mod spec;
pub(crate) mod submit_review;
pub mod view_image;
pub mod wait_agent;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;
pub mod write_stdin;

use registry::ToolContext;

pub use handler::{ToolCapabilities, ToolHandler, ToolInvocation};
pub use outcome::{ToolModelOutput, ToolOutcome, ToolRuntimeEffect};
pub use spec::{ToolSpec, ToolSpecKind};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult;
}
