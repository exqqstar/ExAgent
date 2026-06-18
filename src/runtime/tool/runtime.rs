use std::sync::Arc;

use anyhow::Result;

use crate::config::AgentConfig;
use crate::exec_session::{ExecOutputEventSink, ExecSessionManager};
use crate::policy::PolicyManager;
use crate::registry::ToolContext;
use crate::runtime::agent_profile::AgentToolPolicy;
use crate::runtime::goal::GoalToolApi;
use crate::runtime::memory::MemoryToolApi;
use crate::runtime::thread_session::ThreadEventRecorder;
use crate::runtime::thread_session::ThreadInbox;
use crate::runtime::tool::effects::ToolExecutionOutcome;
use crate::runtime::tool::hooks::{NoopToolHooks, ToolHooks};
use crate::runtime::tool::orchestrator::ToolOrchestrator;
use crate::runtime::tool::selection::ToolSelection;
use crate::session::ThreadSnapshot;
use crate::tools::ToolSpec;
use crate::types::{ThreadId, ToolCall, TurnId};

pub(crate) struct ToolCallRuntime {
    config: AgentConfig,
    selection: ToolSelection,
    orchestrator: ToolOrchestrator,
    exec_sessions: Arc<ExecSessionManager>,
    exec_output_sink: Option<ExecOutputEventSink>,
    policy: Arc<PolicyManager>,
    agent_tool_policy: AgentToolPolicy,
    thread_id: ThreadId,
    turn_id: TurnId,
    inbox: Option<Arc<ThreadInbox>>,
    goal_api: Option<Arc<GoalToolApi>>,
    memory_api: Option<Arc<MemoryToolApi>>,
}

impl ToolCallRuntime {
    pub(crate) fn new(
        config: AgentConfig,
        selection: ToolSelection,
        exec_sessions: Arc<ExecSessionManager>,
        exec_output_sink: Option<ExecOutputEventSink>,
        policy: Arc<PolicyManager>,
        agent_tool_policy: AgentToolPolicy,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Self {
        let orchestrator =
            ToolOrchestrator::with_hooks(selection.resolver(), Arc::new(NoopToolHooks));
        Self {
            config,
            selection,
            orchestrator,
            exec_sessions,
            exec_output_sink,
            policy,
            agent_tool_policy,
            thread_id,
            turn_id,
            inbox: None,
            goal_api: None,
            memory_api: None,
        }
    }

    pub(crate) fn with_tool_hooks(mut self, tool_hooks: Arc<dyn ToolHooks>) -> Self {
        self.orchestrator = ToolOrchestrator::with_hooks(self.selection.resolver(), tool_hooks);
        self
    }

    pub(crate) fn with_inbox(mut self, inbox: Arc<ThreadInbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    pub(crate) fn with_goal_api(mut self, goal_api: Option<Arc<GoalToolApi>>) -> Self {
        self.goal_api = goal_api;
        self
    }

    pub(crate) fn with_memory_api(mut self, memory_api: Option<Arc<MemoryToolApi>>) -> Self {
        self.memory_api = memory_api;
        self
    }

    #[cfg(test)]
    pub(crate) fn schemas(&self) -> Vec<serde_json::Value> {
        self.visible_specs()
            .iter()
            .map(|spec| spec.to_internal_schema())
            .collect()
    }

    pub(crate) fn visible_specs(&self) -> &[ToolSpec] {
        self.selection.visible_specs()
    }

    pub(crate) async fn execute_with_lifecycle(
        &self,
        call: ToolCall,
        recorder: &mut ThreadEventRecorder,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
    ) -> Result<ToolExecutionOutcome> {
        let ctx = self.context();
        self.orchestrator
            .execute_with_lifecycle(call, &ctx, recorder, snapshot, turn_id)
            .await
    }

    fn context(&self) -> ToolContext {
        ToolContext {
            config: self.config.clone(),
            thread_id: Some(self.thread_id.clone()),
            turn_id: Some(self.turn_id.clone()),
            tool_invocation_id: None,
            exec_sessions: self.exec_sessions.clone(),
            exec_output_sink: self.exec_output_sink.clone(),
            policy: self.policy.clone(),
            agent_tool_policy: self.agent_tool_policy.clone(),
            inbox: self.inbox.clone(),
            goal_api: self.goal_api.clone(),
            memory_api: self.memory_api.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ToolRegistry;
    use crate::runtime::tool::resolver::ToolResolver;
    use crate::runtime::tool::selection::{select_visible_specs, ToolVisibilityContext};
    use crate::tools::read_file::ReadFileTool;

    fn selection_from_registry(
        base: ToolRegistry,
        config: &AgentConfig,
        agent_tool_policy: AgentToolPolicy,
    ) -> ToolSelection {
        let visible_specs = select_visible_specs(
            &base,
            &ToolVisibilityContext {
                permission_profile: config.permission_profile,
                provider_supports_tools: config.model.capabilities.supports_tools,
                agent_tool_policy: agent_tool_policy.clone(),
            },
        );
        ToolSelection::new(ToolResolver::new(base), visible_specs)
    }

    #[test]
    fn tool_call_runtime_schemas_respect_provider_tool_capability() {
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);

        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = false;
        let agent_tool_policy = AgentToolPolicy::all();
        let selection = selection_from_registry(registry, &config, agent_tool_policy.clone());

        let runtime = ToolCallRuntime::new(
            config,
            selection,
            Arc::new(ExecSessionManager::default()),
            None,
            Arc::new(PolicyManager::default()),
            agent_tool_policy,
            ThreadId::new("thread_tools_hidden"),
            TurnId::new("turn_tools_hidden"),
        );

        assert!(runtime.schemas().is_empty());
    }

    #[test]
    fn tool_call_runtime_visible_specs_respect_provider_tool_capability() {
        let mut registry = ToolRegistry::new();
        registry.register(ReadFileTool);

        let mut config = AgentConfig::default();
        config.model.capabilities.supports_tools = false;
        let agent_tool_policy = AgentToolPolicy::all();
        let selection = selection_from_registry(registry, &config, agent_tool_policy.clone());

        let runtime = ToolCallRuntime::new(
            config,
            selection,
            Arc::new(ExecSessionManager::default()),
            None,
            Arc::new(PolicyManager::default()),
            agent_tool_policy,
            ThreadId::new("thread_specs_hidden"),
            TurnId::new("turn_specs_hidden"),
        );

        assert!(runtime.visible_specs().is_empty());
    }
}
