//! Single source of truth for assembling the tool registry.
//!
//! `register_base_tools` covers the always-on, dependency-free tools.
//! `register_dynamic_tools` covers the capability-gated tools that need runtime
//! dependencies. The runtime layer (`tool_selection`) builds [`DynamicToolDeps`]
//! from its per-turn input and calls into here, so tool registration lives in
//! one place instead of being split across `lib.rs` and `tool_selection.rs`.
//! See ADR-0042.

use std::sync::Arc;

use crate::app_server::protocol::ThreadGoalMode;
use crate::config::WebSearchConfig;
use crate::mcp::tool::McpToolHandler;
use crate::runtime::forge::open_questions::OpenQuestionStore;
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::goal::GoalToolApi;
use crate::runtime::memory::MemoryToolApi;
use crate::runtime::subagent::AgentControl;
use crate::tools::apply_patch::ApplyPatchTool;
use crate::tools::ask_user::AskUserTool;
use crate::tools::close_agent::CloseAgentTool;
use crate::tools::defer_question::DeferQuestionTool;
use crate::tools::exec_command::ExecCommandTool;
use crate::tools::followup_task::FollowupTaskTool;
use crate::tools::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
use crate::tools::list_agents::ListAgentsTool;
use crate::tools::list_dir::ListDirTool;
use crate::tools::memory_forget::MemoryForgetTool;
use crate::tools::memory_list::MemoryListTool;
use crate::tools::memory_recall::MemoryRecallTool;
use crate::tools::memory_save::MemorySaveTool;
use crate::tools::memory_update::MemoryUpdateTool;
use crate::tools::read_file::ReadFileTool;
use crate::tools::registry::ToolRegistry;
use crate::tools::run_command::RunCommandTool;
use crate::tools::search_files::SearchFilesTool;
use crate::tools::send_message::SendMessageTool;
use crate::tools::spawn_agent::SpawnAgentTool;
use crate::tools::submit_review::SubmitReviewTool;
use crate::tools::view_image::ViewImageTool;
use crate::tools::wait_agent::WaitAgentTool;
use crate::tools::web_fetch::WebFetchTool;
use crate::tools::web_search::{BraveSearchProvider, WebSearchTool};
use crate::tools::write_file::WriteFileTool;
use crate::tools::write_stdin::WriteStdinTool;

/// Register the always-on tools that need no runtime dependencies.
pub fn register_base_tools(registry: &mut ToolRegistry) {
    registry.register(ReadFileTool);
    registry.register(SearchFilesTool);
    registry.register(ListDirTool);
    registry.register(ViewImageTool);
    registry.register(WebFetchTool);
    registry.register(AskUserTool);
    registry.register(ApplyPatchTool);
    registry.register(WriteFileTool);
    registry.register(ExecCommandTool);
    registry.register(WriteStdinTool);
    registry.register(RunCommandTool);
}

/// Build a registry preloaded with the base tools.
pub fn base_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_base_tools(&mut registry);
    registry
}

/// Runtime dependencies for the capability-gated tools.
///
/// MCP handlers are passed in already-fetched because the runtime resolves them
/// asynchronously; this keeps `register_dynamic_tools` synchronous and free of
/// runtime IO.
pub(crate) struct DynamicToolDeps {
    pub(crate) subagent_control: Option<Arc<AgentControl>>,
    pub(crate) goal_api: Option<Arc<GoalToolApi>>,
    pub(crate) forge_review_gate_enabled: bool,
    pub(crate) memory_enabled: bool,
    pub(crate) memory_api: Option<Arc<MemoryToolApi>>,
    pub(crate) forge_review_store: Option<ReviewStore>,
    pub(crate) active_goal_mode: ThreadGoalMode,
    pub(crate) web_search: Option<WebSearchConfig>,
    pub(crate) mcp_handlers: Vec<McpToolHandler>,
}

/// Register the capability-gated tools onto an existing registry (usually the
/// base registry). Each group is added only when its dependency is present.
pub(crate) fn register_dynamic_tools(registry: &mut ToolRegistry, deps: DynamicToolDeps) {
    if let Some(control) = deps.subagent_control {
        registry.register_handler(SpawnAgentTool::new(control.clone()));
        registry.register_handler(ListAgentsTool::new(control.clone()));
        registry.register_handler(CloseAgentTool::new(control.clone()));
        registry.register_handler(SendMessageTool::new(control.clone()));
        registry.register_handler(FollowupTaskTool::new(control));
        registry.register_handler(WaitAgentTool);
    }

    if let Some(goal_api) = deps.goal_api {
        registry.register_handler(GetGoalTool::new(goal_api.clone()));
        registry.register_handler(CreateGoalTool::new_with_forge_modes(
            goal_api.clone(),
            deps.forge_review_gate_enabled,
        ));
        registry.register_handler(UpdateGoalTool::new(goal_api));
    }

    if deps.memory_enabled && deps.memory_api.is_some() {
        registry.register_handler(MemoryRecallTool);
        registry.register_handler(MemorySaveTool);
        registry.register_handler(MemoryUpdateTool);
        registry.register_handler(MemoryForgetTool);
        registry.register_handler(MemoryListTool);
    }

    if deps.forge_review_gate_enabled {
        if let Some(review_store) = deps.forge_review_store {
            registry.register_handler(SubmitReviewTool::new(review_store.clone()));
            if deps.active_goal_mode.is_review_gated() {
                registry.register_handler(DeferQuestionTool::new(OpenQuestionStore::new(
                    review_store.db(),
                )));
            }
        }
    }

    if let Some(web_search) = &deps.web_search {
        if web_search.provider == "brave" {
            registry.register_handler(WebSearchTool::new(Arc::new(BraveSearchProvider::new(
                web_search.api_key.clone(),
            ))));
        }
    }

    for handler in deps.mcp_handlers {
        registry.register_handler(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_registry_exposes_expected_catalog() {
        let registry = base_tool_registry();
        let mut names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
        names.sort();
        let mut expected = vec![
            "apply_patch",
            "ask_user",
            "exec_command",
            "list_dir",
            "read_file",
            "run_command",
            "search_files",
            "view_image",
            "web_fetch",
            "write_file",
            "write_stdin",
        ];
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn dynamic_tools_are_skipped_when_no_dependencies_present() {
        let mut registry = base_tool_registry();
        let before = registry.specs().len();
        register_dynamic_tools(
            &mut registry,
            DynamicToolDeps {
                subagent_control: None,
                goal_api: None,
                forge_review_gate_enabled: false,
                memory_enabled: false,
                memory_api: None,
                forge_review_store: None,
                active_goal_mode: ThreadGoalMode::default(),
                web_search: None,
                mcp_handlers: Vec::new(),
            },
        );
        assert_eq!(registry.specs().len(), before);
    }
}
