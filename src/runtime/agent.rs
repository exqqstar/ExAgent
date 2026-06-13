use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::app_server::protocol::ThreadGoalMode;
use crate::config::AgentConfig;
use crate::exec_session::{ExecOutputEventSink, ExecSessionManager};
use crate::llm::{LlmClient, LlmRequestOptions, LlmStreamSink};
#[cfg(test)]
use crate::mcp::client::McpClientFactory;
use crate::mcp::manager::McpRuntimeManager;
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::runtime::agent_profile::AgentToolPolicy;
use crate::runtime::forge::gate::ForgeGateHooks;
use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
use crate::runtime::forge::review::ReviewStore;
use crate::runtime::goal::GoalToolApi;
use crate::runtime::subagent::AgentControl;
use crate::runtime::thread_session::ThreadInbox;
use crate::runtime::tool_call_runtime::ToolCallRuntime;
use crate::runtime::tool_hooks::{NoopToolHooks, ToolHooks};
use crate::runtime::tool_selection::{build_tool_selection, ToolSelectionInput};
use crate::tools::ToolSpec;
use crate::types::{ConversationMessage, LlmCompletion, ThreadId, TurnId};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
    mcp_runtime: Arc<McpRuntimeManager>,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    tool_hooks: Arc<dyn ToolHooks>,
    subagent_control: Option<Arc<AgentControl>>,
    goal_api: Option<Arc<GoalToolApi>>,
    forge_review_store: Option<ReviewStore>,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Box<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self::with_runtime(
            config,
            llm,
            registry,
            Arc::new(ExecSessionManager::default()),
            Arc::new(PolicyManager::default()),
        )
    }

    pub fn with_exec_sessions(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
    ) -> Self {
        Self::with_runtime(
            config,
            llm,
            registry,
            exec_sessions,
            Arc::new(PolicyManager::default()),
        )
    }

    pub fn with_runtime(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
        policy: Arc<PolicyManager>,
    ) -> Self {
        let mcp_runtime = Arc::new(McpRuntimeManager::new(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
        ));
        Self::with_runtime_parts(config, llm, registry, exec_sessions, policy, mcp_runtime)
    }

    #[cfg(test)]
    pub(crate) fn with_runtime_and_mcp_client_factory_for_tests(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
        policy: Arc<PolicyManager>,
        mcp_client_factory: Arc<dyn McpClientFactory>,
    ) -> Self {
        let mcp_runtime = Arc::new(McpRuntimeManager::with_factory(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
            mcp_client_factory,
        ));
        Self::with_runtime_parts(config, llm, registry, exec_sessions, policy, mcp_runtime)
    }

    fn with_runtime_parts(
        config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry: ToolRegistry,
        exec_sessions: Arc<ExecSessionManager>,
        policy: Arc<PolicyManager>,
        mcp_runtime: Arc<McpRuntimeManager>,
    ) -> Self {
        Self {
            config,
            llm,
            registry,
            mcp_runtime,
            exec_sessions,
            policy,
            tool_hooks: Arc::new(NoopToolHooks),
            subagent_control: None,
            goal_api: None,
            forge_review_store: None,
        }
    }

    pub(crate) fn with_subagent_control(
        mut self,
        subagent_control: Option<Arc<AgentControl>>,
    ) -> Self {
        self.subagent_control = subagent_control;
        self
    }

    pub(crate) fn with_goal_api(mut self, goal_api: Option<Arc<GoalToolApi>>) -> Self {
        self.goal_api = goal_api;
        self
    }

    pub(crate) fn with_forge_review_store(mut self, store: Option<ReviewStore>) -> Self {
        self.forge_review_store = store;
        self
    }

    pub(crate) fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub(crate) async fn sample_assistant_turn(
        &self,
        prompt: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(prompt, tools, options).await
    }

    pub(crate) async fn stream_assistant_turn(
        &self,
        prompt: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmCompletion> {
        self.llm.stream(prompt, tools, options, sink).await
    }

    pub async fn shutdown(&self) {
        self.mcp_runtime.shutdown().await;
    }

    pub(crate) async fn tool_runtime(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        workspace_root: PathBuf,
        cwd: PathBuf,
        exec_output_sink: Option<ExecOutputEventSink>,
        agent_tool_policy: AgentToolPolicy,
        inbox: Option<Arc<ThreadInbox>>,
    ) -> Result<ToolCallRuntime> {
        let mut turn_config = self.config.clone();
        turn_config.workspace_root = workspace_root;
        turn_config.cwd = cwd.clone();
        let forge_gate_enabled = turn_config.forge_review_gate_enabled;
        let active_goal_mode = self.active_goal_mode(&thread_id).await?;
        // This turn_config is local to Agent::tool_runtime after applying
        // workspace_root/cwd. It is separate from the LLM turn_config built in
        // run_session_turn for model/thinking/profile defaults.
        let selection = build_tool_selection(ToolSelectionInput {
            base_registry: self.registry.clone(),
            config: &turn_config,
            mcp_runtime: self.mcp_runtime.clone(),
            subagent_control: self.subagent_control.clone(),
            goal_api: self.goal_api.clone(),
            forge_review_store: self.forge_review_store.clone(),
            active_goal_mode,
            agent_tool_policy: agent_tool_policy.clone(),
        })
        .await?;

        let mut runtime = ToolCallRuntime::new(
            turn_config,
            selection,
            self.exec_sessions.clone(),
            exec_output_sink,
            self.policy.clone(),
            agent_tool_policy,
            thread_id,
            turn_id,
        )
        .with_tool_hooks(self.tool_hooks.clone());
        if forge_gate_enabled {
            if let Some(review_store) = self.forge_review_store.clone() {
                runtime = runtime.with_tool_hooks(Arc::new(ForgeGateHooks::new(
                    review_store.clone(),
                    crate::runtime::forge::open_questions::OpenQuestionStore::new(
                        review_store.db(),
                    ),
                )));
            }
        }
        if let Some(inbox) = inbox {
            runtime = runtime.with_inbox(inbox);
        }
        runtime = runtime.with_goal_api(self.goal_api.clone());
        Ok(runtime)
    }

    async fn active_goal_mode(&self, thread_id: &ThreadId) -> Result<ThreadGoalMode> {
        let (Some(goal_api), Some(review_store)) =
            (self.goal_api.as_ref(), self.forge_review_store.as_ref())
        else {
            return Ok(ThreadGoalMode::Standard);
        };
        let Some(goal) = goal_api.get_goal(thread_id).await? else {
            return Ok(ThreadGoalMode::Standard);
        };
        ForgeGoalModeStore::new(review_store.db())
            .mode_for_goal(thread_id, &goal.goal_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::json;

    use crate::llm::LlmRequestOptions;
    use crate::mcp::client::{McpCallOutput, McpClient, McpClientFactory, McpToolDefinition};
    use crate::mcp::config::McpServerConfig;
    use crate::policy::PolicyManager;
    use crate::runtime::agent_profile::AgentToolPolicy;
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::run_command::RunCommandTool;
    use crate::tools::search_files::SearchFilesTool;
    use crate::tools::write_file::WriteFileTool;
    use crate::types::{ConversationMessage, LlmCompletion};

    struct PanicLlm;

    #[async_trait]
    impl LlmClient for PanicLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            panic!("agent runtime visibility tests do not sample")
        }
    }

    #[derive(Clone, Default)]
    struct CountingFactory {
        connected: Arc<AtomicUsize>,
        listed: Arc<AtomicUsize>,
        shutdown: Arc<AtomicUsize>,
    }

    struct CountingClient {
        listed: Arc<AtomicUsize>,
        shutdown: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl McpClientFactory for CountingFactory {
        async fn connect(
            &self,
            _server: &McpServerConfig,
            _default_cwd: &Path,
        ) -> Result<Arc<dyn McpClient>> {
            self.connected.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(CountingClient {
                listed: self.listed.clone(),
                shutdown: self.shutdown.clone(),
            }))
        }
    }

    #[async_trait]
    impl McpClient for CountingClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDefinition>> {
            self.listed.fetch_add(1, Ordering::SeqCst);
            Ok(vec![McpToolDefinition {
                name: "lookup".into(),
                description: "Lookup a record.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
            }])
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<McpCallOutput> {
            Ok(McpCallOutput {
                content: Vec::new(),
                is_error: false,
                meta: None,
            })
        }

        async fn shutdown(&self) -> Result<()> {
            self.shutdown.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn mcp_server(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            display_name: id.into(),
            command: "fake".into(),
            args: Vec::new(),
            env: HashMap::new(),
            working_directory: None,
        }
    }

    fn agent_with_mcp_support(supports_tools: bool, factory: CountingFactory) -> Agent {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            mcp_servers: vec![mcp_server("records")],
            ..AgentConfig::default()
        };
        config.model.capabilities.supports_tools = supports_tools;
        let mcp_runtime = Arc::new(McpRuntimeManager::with_factory(
            config.mcp_servers.clone(),
            config.workspace_root.clone(),
            Arc::new(factory),
        ));

        Agent {
            config,
            llm: Box::new(PanicLlm),
            registry: ToolRegistry::new(),
            mcp_runtime,
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
            tool_hooks: Arc::new(NoopToolHooks),
            subagent_control: None,
            goal_api: None,
            forge_review_store: None,
        }
    }

    #[tokio::test]
    async fn tool_runtime_overlays_mcp_specs_and_respects_provider_tool_support() {
        let visible_agent = agent_with_mcp_support(true, CountingFactory::default());
        let visible_runtime = visible_agent
            .tool_runtime(
                ThreadId::new("thread_mcp_visible"),
                TurnId::new("turn_mcp_visible"),
                visible_agent.config.workspace_root.clone(),
                visible_agent.config.cwd.clone(),
                None,
                AgentToolPolicy::all(),
                None,
            )
            .await
            .unwrap();

        assert!(visible_runtime
            .visible_specs()
            .iter()
            .any(|spec| spec.name == "mcp__records__lookup"));

        let hidden_factory = CountingFactory::default();
        let hidden_agent = agent_with_mcp_support(false, hidden_factory.clone());
        let hidden_runtime = hidden_agent
            .tool_runtime(
                ThreadId::new("thread_mcp_hidden"),
                TurnId::new("turn_mcp_hidden"),
                hidden_agent.config.workspace_root.clone(),
                hidden_agent.config.cwd.clone(),
                None,
                AgentToolPolicy::all(),
                None,
            )
            .await
            .unwrap();

        assert!(hidden_runtime.visible_specs().is_empty());
        assert_eq!(hidden_factory.connected.load(Ordering::SeqCst), 0);
        assert_eq!(hidden_factory.listed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tool_runtime_filters_visible_specs_by_agent_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.supports_tools = true;
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);
        registry.register(SearchFilesTool);
        registry.register(WriteFileTool);
        registry.register(RunCommandTool);
        let agent = Agent::new(config, Box::new(PanicLlm), registry);

        let runtime = agent
            .tool_runtime(
                ThreadId::new("thread_explorer_tools"),
                TurnId::new("turn_explorer_tools"),
                agent.config.workspace_root.clone(),
                agent.config.cwd.clone(),
                None,
                AgentToolPolicy::read_only_basic_collaboration(),
                None,
            )
            .await
            .unwrap();
        let specs = runtime.visible_specs();

        assert!(specs.iter().any(|spec| spec.name == "read_file"));
        assert!(specs.iter().any(|spec| spec.name == "search_files"));
        assert!(!specs.iter().any(|spec| spec.name == "write_file"));
        assert!(!specs.iter().any(|spec| spec.name == "run_command"));
    }

    #[tokio::test]
    async fn shutdown_closes_mcp_runtime_clients() {
        let factory = CountingFactory::default();
        let agent = agent_with_mcp_support(true, factory.clone());

        let runtime = agent
            .tool_runtime(
                ThreadId::new("thread_mcp_shutdown"),
                TurnId::new("turn_mcp_shutdown"),
                agent.config.workspace_root.clone(),
                agent.config.cwd.clone(),
                None,
                AgentToolPolicy::all(),
                None,
            )
            .await
            .unwrap();
        assert!(runtime
            .visible_specs()
            .iter()
            .any(|spec| spec.name == "mcp__records__lookup"));

        agent.shutdown().await;

        assert_eq!(factory.connected.load(Ordering::SeqCst), 1);
        assert_eq!(factory.listed.load(Ordering::SeqCst), 1);
        assert_eq!(factory.shutdown.load(Ordering::SeqCst), 1);
    }
}
