pub mod agent;
pub mod api;
pub mod app_server;
pub mod cli;
pub mod cli_adapter;
pub mod config;
pub mod events;
pub mod exec_session;
pub mod llm;
pub mod orchestration;
pub mod policy;
pub mod registry;
pub mod result_contract;
pub mod runtime;
pub mod session;
pub mod tools;
pub mod transcript;
pub mod types;
pub mod workspace;

pub fn default_tool_registry() -> registry::ToolRegistry {
    let mut registry = registry::ToolRegistry::new();
    registry.register(tools::read_file::ReadFileTool);
    registry.register(tools::record_structured_result::RecordStructuredResultTool);
    registry.register(tools::write_file::WriteFileTool);
    registry.register(tools::run_command::RunCommandTool);
    registry
}
