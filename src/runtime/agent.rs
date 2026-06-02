use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::{LlmClient, LlmRequestOptions};
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::runtime::tool_call_runtime::ToolCallRuntime;
use crate::types::{ConversationMessage, LlmCompletion, ThreadId, TurnId};

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

    pub(crate) fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub(crate) async fn sample_assistant_turn(
        &self,
        prompt: &[ConversationMessage],
        tool_schemas: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(prompt, tool_schemas, options).await
    }

    pub(crate) fn tool_runtime(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        workspace_root: PathBuf,
        cwd: PathBuf,
    ) -> ToolCallRuntime {
        let mut turn_config = self.config.clone();
        turn_config.workspace_root = workspace_root;
        turn_config.cwd = cwd.clone();
        ToolCallRuntime::new(
            turn_config,
            self.registry.clone(),
            self.exec_sessions.clone(),
            self.policy.clone(),
            thread_id,
            turn_id,
            cwd,
        )
    }
}
