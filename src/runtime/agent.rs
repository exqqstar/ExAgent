use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::LlmClient;
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::runtime::tool_call_runtime::ToolCallRuntime;
use crate::types::{AssistantTurn, ConversationMessage, SessionId, TurnId};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
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
        Self {
            config,
            llm,
            registry,
            exec_sessions,
            policy,
        }
    }

    pub(crate) fn max_turns(&self) -> usize {
        self.config.max_turns
    }

    pub(crate) async fn sample_assistant_turn(
        &self,
        prompt: &[ConversationMessage],
        tool_schemas: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        self.llm.complete(prompt, tool_schemas).await
    }

    pub(crate) fn tool_runtime(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        workspace_root: PathBuf,
        cwd: PathBuf,
    ) -> ToolCallRuntime {
        let mut session_config = self.config.clone();
        session_config.workspace_root = workspace_root;
        session_config.cwd = cwd.clone();
        ToolCallRuntime::new(
            session_config,
            self.registry.clone(),
            self.exec_sessions.clone(),
            self.policy.clone(),
            session_id,
            turn_id,
            cwd,
        )
    }
}
