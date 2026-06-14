use std::sync::Arc;

use anyhow::Result;

use crate::app_server::protocol::ThreadGoalMode;
use crate::config::{AgentConfig, PermissionProfile};
use crate::mcp::manager::McpRuntimeManager;
use crate::registry::ToolRegistry;
use crate::runtime::agent_profile::AgentToolPolicy;
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::goal::GoalToolApi;
use crate::runtime::memory::MemoryToolApi;
use crate::runtime::subagent::AgentControl;
use crate::runtime::tool_resolver::ToolResolver;
use crate::tools::close_agent::CloseAgentTool;
use crate::tools::defer_question::DeferQuestionTool;
use crate::tools::followup_task::FollowupTaskTool;
use crate::tools::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
use crate::tools::list_agents::ListAgentsTool;
use crate::tools::memory_forget::MemoryForgetTool;
use crate::tools::memory_list::MemoryListTool;
use crate::tools::memory_recall::MemoryRecallTool;
use crate::tools::memory_save::MemorySaveTool;
use crate::tools::memory_update::MemoryUpdateTool;
use crate::tools::send_message::SendMessageTool;
use crate::tools::spawn_agent::SpawnAgentTool;
use crate::tools::submit_review::SubmitReviewTool;
use crate::tools::wait_agent::WaitAgentTool;
use crate::tools::web_search::{BraveSearchProvider, WebSearchTool};
use crate::tools::ToolSpec;

pub(crate) struct ToolSelection {
    resolver: ToolResolver,
    visible_specs: Vec<ToolSpec>,
}

impl ToolSelection {
    pub(crate) fn new(resolver: ToolResolver, visible_specs: Vec<ToolSpec>) -> Self {
        Self {
            resolver,
            visible_specs,
        }
    }

    pub(crate) fn resolver(&self) -> ToolResolver {
        self.resolver.clone()
    }

    pub(crate) fn visible_specs(&self) -> &[ToolSpec] {
        &self.visible_specs
    }
}

pub(crate) struct ToolSelectionInput<'a> {
    pub(crate) base_registry: ToolRegistry,
    pub(crate) config: &'a AgentConfig,
    pub(crate) mcp_runtime: Arc<McpRuntimeManager>,
    pub(crate) subagent_control: Option<Arc<AgentControl>>,
    pub(crate) goal_api: Option<Arc<GoalToolApi>>,
    pub(crate) memory_api: Option<Arc<MemoryToolApi>>,
    pub(crate) forge_review_store: Option<ReviewStore>,
    pub(crate) active_goal_mode: ThreadGoalMode,
    pub(crate) agent_tool_policy: AgentToolPolicy,
}

pub(crate) struct ToolVisibilityContext {
    pub(crate) permission_profile: PermissionProfile,
    pub(crate) provider_supports_tools: bool,
    pub(crate) agent_tool_policy: AgentToolPolicy,
}

pub(crate) async fn build_tool_selection(input: ToolSelectionInput<'_>) -> Result<ToolSelection> {
    let registry = assemble_tool_registry(&input).await?;
    let visible_specs = select_visible_specs(
        &registry,
        &ToolVisibilityContext {
            permission_profile: input.config.permission_profile,
            provider_supports_tools: input.config.model.capabilities.supports_tools,
            agent_tool_policy: input.agent_tool_policy,
        },
    );
    Ok(ToolSelection::new(
        ToolResolver::new(registry),
        visible_specs,
    ))
}

async fn assemble_tool_registry(input: &ToolSelectionInput<'_>) -> Result<ToolRegistry> {
    let mut registry = input.base_registry.clone();

    if let Some(control) = input.subagent_control.clone() {
        registry.register_handler(SpawnAgentTool::new(control.clone()));
        registry.register_handler(ListAgentsTool::new(control.clone()));
        registry.register_handler(CloseAgentTool::new(control.clone()));
        registry.register_handler(SendMessageTool::new(control.clone()));
        registry.register_handler(FollowupTaskTool::new(control));
        registry.register_handler(WaitAgentTool);
    }

    if let Some(goal_api) = input.goal_api.clone() {
        registry.register_handler(GetGoalTool::new(goal_api.clone()));
        registry.register_handler(CreateGoalTool::new_with_forge_modes(
            goal_api.clone(),
            input.config.forge_review_gate_enabled,
        ));
        registry.register_handler(UpdateGoalTool::new(goal_api));
    }

    if input.config.memory_enabled && input.memory_api.is_some() {
        registry.register_handler(MemoryRecallTool);
        registry.register_handler(MemorySaveTool);
        registry.register_handler(MemoryUpdateTool);
        registry.register_handler(MemoryForgetTool);
        registry.register_handler(MemoryListTool);
    }

    if input.config.forge_review_gate_enabled {
        if let Some(review_store) = input.forge_review_store.clone() {
            registry.register_handler(SubmitReviewTool::new(review_store.clone()));
            if input.active_goal_mode.is_review_gated() {
                registry.register_handler(DeferQuestionTool::new(
                    crate::runtime::forge::open_questions::OpenQuestionStore::new(
                        review_store.db(),
                    ),
                ));
            }
        }
    }

    if let Some(web_search) = &input.config.web_search {
        if web_search.provider == "brave" {
            registry.register_handler(WebSearchTool::new(Arc::new(BraveSearchProvider::new(
                web_search.api_key.clone(),
            ))));
        }
    }

    if input.config.model.capabilities.supports_tools {
        for handler in input.mcp_runtime.handlers().await? {
            registry.register_handler(handler);
        }
    }

    Ok(registry)
}

pub(crate) fn select_visible_specs(
    registry: &ToolRegistry,
    ctx: &ToolVisibilityContext,
) -> Vec<ToolSpec> {
    if !ctx.provider_supports_tools || !ctx.permission_profile.is_supported() {
        return Vec::new();
    }

    registry
        .specs()
        .into_iter()
        .filter(|spec| authorize_tool(&spec.name, &ctx.agent_tool_policy))
        .collect()
}

pub(crate) fn authorize_tool(tool_name: &str, agent_tool_policy: &AgentToolPolicy) -> bool {
    agent_tool_policy.allows(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebSearchConfig;
    use crate::registry::ToolContext;
    use crate::runtime::agent_profile::{profile_for_type, AgentType};
    use crate::runtime::subagent::{
        CloseAgentResponse, CloseAgentsRequest, DeliverInterAgentMessageRequest,
        SendMessageResponse, SpawnAgentResponse, SpawnCleanChildRequest, SubagentLifecycle,
    };
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::run_command::RunCommandTool;
    use crate::tools::search_files::SearchFilesTool;
    use crate::tools::write_file::WriteFileTool;
    use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome};
    use crate::types::ThreadId;
    use crate::types::ToolCall;
    use async_trait::async_trait;

    struct NoopSubagentLifecycle;

    #[async_trait]
    impl SubagentLifecycle for NoopSubagentLifecycle {
        async fn spawn_clean_child(
            &self,
            _request: SpawnCleanChildRequest,
            _control: Arc<AgentControl>,
        ) -> Result<SpawnAgentResponse> {
            unreachable!("tool selection tests do not spawn subagents")
        }

        async fn close_agents(&self, _request: CloseAgentsRequest) -> Result<CloseAgentResponse> {
            unreachable!("tool selection tests do not close subagents")
        }

        async fn deliver_inter_agent_message(
            &self,
            _request: DeliverInterAgentMessageRequest,
        ) -> Result<SendMessageResponse> {
            unreachable!("tool selection tests do not send messages")
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);
        registry.register(SearchFilesTool);
        registry.register(WriteFileTool);
        registry.register(RunCommandTool);
        registry
    }

    struct SubmitReviewSpecTool;
    struct DeferQuestionSpecTool;

    #[async_trait]
    impl ToolHandler for SubmitReviewSpecTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "submit_review",
                "Submit a reviewer verdict.",
                serde_json::json!({"type": "object"}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::read_only()
        }

        async fn handle(&self, _invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            unreachable!("tool selection tests do not execute submit_review")
        }
    }

    #[async_trait]
    impl ToolHandler for DeferQuestionSpecTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "defer_question",
                "Record a deferred user question.",
                serde_json::json!({"type": "object"}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::read_only()
        }

        async fn handle(&self, _invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            unreachable!("tool selection tests do not execute defer_question")
        }
    }

    fn registry_with_submit_review() -> ToolRegistry {
        let mut registry = registry();
        registry.register(SubmitReviewSpecTool);
        registry
    }

    fn registry_with_defer_question() -> ToolRegistry {
        let mut registry = registry();
        registry.register(DeferQuestionSpecTool);
        registry
    }

    fn subagent_control() -> Arc<AgentControl> {
        let lifecycle = Arc::new(NoopSubagentLifecycle);
        let lifecycle: Arc<dyn SubagentLifecycle> = lifecycle;
        AgentControl::new_root(
            ThreadId::new("thread_tool_selection_root"),
            Arc::downgrade(&lifecycle),
        )
    }

    #[test]
    fn select_visible_specs_returns_empty_when_provider_does_not_support_tools() {
        let visible = select_visible_specs(
            &registry(),
            &ToolVisibilityContext {
                permission_profile: PermissionProfile::FullAccess,
                provider_supports_tools: false,
                agent_tool_policy: AgentToolPolicy::all(),
            },
        );

        assert!(visible.is_empty());
    }

    #[test]
    fn select_visible_specs_uses_gate_then_agent_policy() {
        let visible = select_visible_specs(
            &registry(),
            &ToolVisibilityContext {
                permission_profile: PermissionProfile::FullAccess,
                provider_supports_tools: true,
                agent_tool_policy: AgentToolPolicy::read_only_basic_collaboration(),
            },
        );

        let names = visible
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"search_files"));
        assert!(!names.contains(&"write_file"));
        assert!(!names.contains(&"run_command"));
    }

    #[test]
    fn authorize_tool_is_the_shared_agent_policy_predicate() {
        let policy = AgentToolPolicy::read_only_basic_collaboration();

        assert!(authorize_tool("read_file", &policy));
        assert!(authorize_tool("send_message", &policy));
        assert!(authorize_tool("wait_agent", &policy));
        assert!(!authorize_tool("run_command", &policy));
        assert!(!authorize_tool("spawn_agent", &policy));
    }

    #[test]
    fn submit_review_is_visible_only_to_reviewer_profile() {
        let reviewer = profile_for_type(Some(AgentType::Reviewer));
        let reviewer_visible = select_visible_specs(
            &registry_with_submit_review(),
            &ToolVisibilityContext {
                permission_profile: PermissionProfile::FullAccess,
                provider_supports_tools: true,
                agent_tool_policy: reviewer.tool_policy,
            },
        );
        assert!(reviewer_visible
            .iter()
            .any(|spec| spec.name == "submit_review"));

        let worker = profile_for_type(Some(AgentType::Worker));
        let worker_visible = select_visible_specs(
            &registry_with_submit_review(),
            &ToolVisibilityContext {
                permission_profile: PermissionProfile::FullAccess,
                provider_supports_tools: true,
                agent_tool_policy: worker.tool_policy,
            },
        );
        assert!(!worker_visible
            .iter()
            .any(|spec| spec.name == "submit_review"));
    }

    #[test]
    fn defer_question_is_visible_only_to_worker_profile() {
        let worker = profile_for_type(Some(AgentType::Worker));
        let worker_visible = select_visible_specs(
            &registry_with_defer_question(),
            &ToolVisibilityContext {
                permission_profile: PermissionProfile::FullAccess,
                provider_supports_tools: true,
                agent_tool_policy: worker.tool_policy,
            },
        );
        assert!(worker_visible
            .iter()
            .any(|spec| spec.name == "defer_question"));

        for agent_type in [AgentType::Explorer, AgentType::Planner, AgentType::Reviewer] {
            let profile = profile_for_type(Some(agent_type));
            let visible = select_visible_specs(
                &registry_with_defer_question(),
                &ToolVisibilityContext {
                    permission_profile: PermissionProfile::FullAccess,
                    provider_supports_tools: true,
                    agent_tool_policy: profile.tool_policy,
                },
            );
            assert!(
                !visible.iter().any(|spec| spec.name == "defer_question"),
                "{agent_type:?} must not see defer_question"
            );
        }
    }

    #[tokio::test]
    async fn build_selection_keeps_registry_executable_when_visible_specs_are_empty() {
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = false;

        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let selection = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection");

        assert!(selection.visible_specs().is_empty());
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "Cargo.toml" }),
            thought_signature: None,
        };
        assert!(selection.resolver().resolve(&call).is_some());
    }

    #[tokio::test]
    async fn build_selection_registers_subagent_tools_and_filters_them_by_policy() {
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = true;
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let all_selection = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: Some(subagent_control()),
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("all selection");
        let all_names = all_selection
            .visible_specs()
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        for tool_name in [
            "spawn_agent",
            "list_agents",
            "close_agent",
            "send_message",
            "followup_task",
            "wait_agent",
        ] {
            assert!(
                all_names.contains(&tool_name),
                "expected {tool_name} in visible specs"
            );
        }

        let read_only_selection = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: Some(subagent_control()),
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::read_only_basic_collaboration(),
        })
        .await
        .expect("read-only selection");
        let read_only_names = read_only_selection
            .visible_specs()
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert!(read_only_names.contains(&"read_file"));
        assert!(read_only_names.contains(&"search_files"));
        assert!(read_only_names.contains(&"list_agents"));
        assert!(read_only_names.contains(&"send_message"));
        assert!(read_only_names.contains(&"wait_agent"));
        for tool_name in ["spawn_agent", "close_agent", "followup_task"] {
            assert!(
                !read_only_names.contains(&tool_name),
                "expected {tool_name} to be hidden by agent policy"
            );
        }
    }

    #[tokio::test]
    async fn build_selection_registers_forge_tools_when_review_store_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = true;
        config.forge_review_gate_enabled = true;
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let review_store = ReviewStore::new(db);
        let reviewer = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: Some(review_store.clone()),
            active_goal_mode: ThreadGoalMode::Reviewed,
            agent_tool_policy: profile_for_type(Some(AgentType::Reviewer)).tool_policy,
        })
        .await
        .expect("reviewer selection");

        assert!(visible_tool_names(&reviewer).contains(&"submit_review"));
        assert!(!visible_tool_names(&reviewer).contains(&"defer_question"));

        let standard_worker = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: Some(review_store.clone()),
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: profile_for_type(Some(AgentType::Worker)).tool_policy,
        })
        .await
        .expect("standard worker selection");

        assert!(!visible_tool_names(&standard_worker).contains(&"submit_review"));
        assert!(!visible_tool_names(&standard_worker).contains(&"defer_question"));

        let worker = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: Some(review_store),
            active_goal_mode: ThreadGoalMode::Reviewed,
            agent_tool_policy: profile_for_type(Some(AgentType::Worker)).tool_policy,
        })
        .await
        .expect("worker selection");

        assert!(!visible_tool_names(&worker).contains(&"submit_review"));
        assert!(visible_tool_names(&worker).contains(&"defer_question"));
    }

    #[tokio::test]
    async fn build_selection_hides_forge_tools_when_forge_gate_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = true;
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let review_store = ReviewStore::new(db);
        let reviewer = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: Some(review_store.clone()),
            active_goal_mode: ThreadGoalMode::Reviewed,
            agent_tool_policy: profile_for_type(Some(AgentType::Reviewer)).tool_policy,
        })
        .await
        .expect("reviewer selection");

        assert!(!visible_tool_names(&reviewer).contains(&"submit_review"));

        let worker = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: Some(review_store),
            active_goal_mode: ThreadGoalMode::Reviewed,
            agent_tool_policy: profile_for_type(Some(AgentType::Worker)).tool_policy,
        })
        .await
        .expect("worker selection");

        assert!(!visible_tool_names(&worker).contains(&"defer_question"));
    }

    #[tokio::test]
    async fn build_selection_registers_memory_tools_only_when_enabled_with_api() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let memory_api = Arc::new(crate::runtime::memory::MemoryToolApi::new(
            crate::runtime::memory::MemoryRuntime::new(db),
        ));
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = true;
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let without_api = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection without memory api");
        assert!(!visible_tool_names(&without_api).contains(&"memory_save"));

        let with_api = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: Some(memory_api.clone()),
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection with memory api");
        for tool_name in [
            "memory_recall",
            "memory_save",
            "memory_update",
            "memory_forget",
            "memory_list",
        ] {
            assert!(visible_tool_names(&with_api).contains(&tool_name));
        }

        config.memory_enabled = false;
        let disabled = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: None,
            goal_api: None,
            memory_api: Some(memory_api),
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection with memory disabled");
        assert!(!visible_tool_names(&disabled).contains(&"memory_save"));
    }

    #[tokio::test]
    async fn build_selection_registers_web_search_only_when_configured() {
        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = true;
        config.web_search = None;
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));

        let without_search = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection without search");
        assert!(!visible_tool_names(&without_search).contains(&"web_search"));

        config.web_search = Some(WebSearchConfig {
            provider: "brave".to_string(),
            api_key: "search-key".to_string(),
        });
        let with_search = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime: mcp_runtime.clone(),
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::read_only_basic_collaboration(),
        })
        .await
        .expect("selection with search");
        assert!(visible_tool_names(&with_search).contains(&"web_search"));

        config.web_search = Some(WebSearchConfig {
            provider: "unsupported".to_string(),
            api_key: "search-key".to_string(),
        });
        let unsupported_provider = build_tool_selection(ToolSelectionInput {
            base_registry: registry(),
            config: &config,
            mcp_runtime,
            subagent_control: None,
            goal_api: None,
            memory_api: None,
            forge_review_store: None,
            active_goal_mode: ThreadGoalMode::Standard,
            agent_tool_policy: AgentToolPolicy::all(),
        })
        .await
        .expect("selection with unsupported provider");
        assert!(!visible_tool_names(&unsupported_provider).contains(&"web_search"));
    }

    fn visible_tool_names(selection: &ToolSelection) -> Vec<&str> {
        selection
            .visible_specs()
            .iter()
            .map(|spec| spec.name.as_str())
            .collect()
    }
}
