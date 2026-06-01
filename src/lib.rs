pub mod app_server;
pub mod config;
pub mod entrypoints;
pub mod model;
pub mod runtime;
pub mod state;
pub mod tools;
pub mod workspace;

pub use entrypoints::api;
pub use entrypoints::cli;
pub use entrypoints::cli_adapter;
pub use model::llm;
pub use model::provider;
pub use model::resolved;
pub use model::resolver;
pub use model::types;
pub use runtime::agent;
pub use runtime::exec_session;
pub use runtime::policy;
pub use state::events;
pub use state::index_db;
pub use state::session;
pub use state::transcript;
pub use tools::registry;

pub fn default_tool_registry() -> tools::registry::ToolRegistry {
    let mut registry = tools::registry::ToolRegistry::new();
    registry.register(tools::read_file::ReadFileTool);
    registry.register(tools::write_file::WriteFileTool);
    registry.register(tools::run_command::RunCommandTool);
    registry
}
