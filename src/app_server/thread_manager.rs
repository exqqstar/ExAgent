use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::agent::{Agent, AgentRunOutput};
use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    AgentRunResponse, CollectParams, CollectResponse, EventsReplayParams, EventsReplayResponse,
    ForkParams, InspectParams, InspectResponse, RunParams, ThreadSpawnChildParams,
    ThreadSpawnChildResponse, ThreadStartParams, ThreadStartResponse, TurnStartParams,
    TurnStartResponse,
};
use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::{LlmClient, OpenAiCompatibleLlm};
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
use crate::session::SessionSnapshot;
use crate::types::{SessionId, TurnId};

type RegistryFactory = Arc<dyn Fn() -> ToolRegistry + Send + Sync>;

trait LlmFactory: Send + Sync {
    fn build(&self, config: &AgentConfig) -> Result<Box<dyn LlmClient>>;
}

struct EnvLlmFactory;

impl LlmFactory for EnvLlmFactory {
    fn build(&self, _config: &AgentConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(OpenAiCompatibleLlm::from_env()?))
    }
}

struct SharedLlmFactory {
    llm: Arc<dyn LlmClient>,
}

impl LlmFactory for SharedLlmFactory {
    fn build(&self, _config: &AgentConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(SharedLlmClient {
            llm: self.llm.clone(),
        }))
    }
}

struct SharedLlmClient {
    llm: Arc<dyn LlmClient>,
}

#[async_trait]
impl LlmClient for SharedLlmClient {
    async fn complete(
        &self,
        messages: &[crate::types::ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<crate::types::AssistantTurn> {
        self.llm.complete(messages, tools).await
    }
}

pub struct ThreadManager {
    base_config: AgentConfig,
    llm_factory: Arc<dyn LlmFactory>,
    registry_factory: RegistryFactory,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
}

impl Default for ThreadManager {
    fn default() -> Self {
        Self::from_env(AgentConfig::default())
    }
}

impl ThreadManager {
    pub fn from_env(base_config: AgentConfig) -> Self {
        Self {
            base_config,
            llm_factory: Arc::new(EnvLlmFactory),
            registry_factory: Arc::new(crate::default_tool_registry),
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
        }
    }

    pub fn with_llm<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        Self {
            base_config,
            llm_factory: Arc::new(SharedLlmFactory {
                llm: Arc::from(llm),
            }),
            registry_factory: Arc::new(registry_factory),
            exec_sessions: Arc::new(ExecSessionManager::default()),
            policy: Arc::new(PolicyManager::default()),
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<AgentRunResponse> {
        let config = OverridePolicy::apply(
            &self.base_config,
            RuntimeOverrides {
                workspace_root: params.workspace_root,
                cwd: params.cwd,
            },
        )?;
        let agent = self.agent_for(config)?;
        let output = match params.session_id.as_ref() {
            Some(session_id) => agent.resume(session_id, &params.prompt).await?,
            None => agent.run_with_meta(&params.prompt).await?,
        };

        Ok(agent_run_response(output))
    }

    pub async fn fork(&self, params: ForkParams) -> Result<AgentRunResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        let agent = self.agent_for(config)?;
        let output = agent
            .fork_session(
                &params.parent_session_id,
                params.agent_role,
                &params.prompt,
                params.spawned_by_turn_id.as_ref(),
            )
            .await?;

        Ok(agent_run_response(output))
    }

    pub fn inspect(&self, params: InspectParams) -> Result<InspectResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        Ok(InspectResponse {
            children: crate::orchestration::inspect_children(
                &config.workspace_root,
                &params.parent_session_id,
            )?,
        })
    }

    pub fn collect(&self, params: CollectParams) -> Result<CollectResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        Ok(CollectResponse {
            session: crate::orchestration::collect_session(
                &config.workspace_root,
                &params.session_id,
            )?,
        })
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
        let config = OverridePolicy::apply(
            &self.base_config,
            RuntimeOverrides {
                workspace_root: params.workspace_root,
                cwd: params.cwd,
            },
        )?;
        let thread_id = crate::transcript::new_session_id();
        let snapshot =
            SessionSnapshot::new_thread(thread_id.clone(), config.workspace_root, config.cwd);
        let paths = crate::transcript::session_paths(&snapshot.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot)?;

        Ok(ThreadStartResponse {
            thread_id,
            snapshot_path: paths.snapshot_path,
            events_path: paths.events_path,
        })
    }

    pub async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStartResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        let thread_id = params.thread_id;
        let agent = self.agent_for(config.clone())?;
        let output = agent.resume(&thread_id, &params.prompt).await?;
        let turn_id = latest_turn_id(&config.workspace_root, &thread_id)?
            .ok_or_else(|| anyhow!("turn_start completed without recording a turn event"))?;

        Ok(TurnStartResponse {
            thread_id,
            turn_id,
            output: agent_run_response(output),
        })
    }

    pub async fn thread_spawn_child(
        &self,
        params: ThreadSpawnChildParams,
    ) -> Result<ThreadSpawnChildResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        let agent = self.agent_for(config)?;
        let output = agent
            .fork_session(
                &params.parent_thread_id,
                params.agent_role.clone(),
                &params.prompt,
                params.spawned_by_turn_id.as_ref(),
            )
            .await?;
        let child_thread_id = output.session_id.clone();

        Ok(ThreadSpawnChildResponse {
            parent_thread_id: params.parent_thread_id,
            child_thread_id,
            agent_role: params.agent_role,
            output: agent_run_response(output),
        })
    }

    pub fn events_replay(&self, params: EventsReplayParams) -> Result<EventsReplayResponse> {
        let config =
            OverridePolicy::apply_workspace_only(&self.base_config, params.workspace_root)?;
        let events = crate::transcript::replay_session(&config.workspace_root, &params.thread_id)?;

        Ok(EventsReplayResponse {
            thread_id: params.thread_id,
            events,
        })
    }

    fn agent_for(&self, config: AgentConfig) -> Result<Agent> {
        let llm = self.llm_factory.build(&config)?;
        Ok(Agent::with_runtime(
            config,
            llm,
            (self.registry_factory)(),
            self.exec_sessions.clone(),
            self.policy.clone(),
        ))
    }
}

fn latest_turn_id(
    workspace_root: &std::path::Path,
    thread_id: &SessionId,
) -> Result<Option<TurnId>> {
    Ok(
        crate::transcript::read_session_events(workspace_root, thread_id)?
            .into_iter()
            .rev()
            .find_map(|event| event.turn_id),
    )
}

fn agent_run_response(output: AgentRunOutput) -> AgentRunResponse {
    AgentRunResponse {
        text: output.final_turn.text,
        tool_calls: output.final_turn.tool_calls,
        session_id: output.session_id,
        snapshot_path: output.snapshot_path,
        events_path: output.events_path,
    }
}
