use std::path::Path;
use std::sync::{Arc, Weak};

use anyhow::Result;
use async_trait::async_trait;

use crate::agent::Agent;
use crate::app_server::request_processors::workflow_processor::{
    new_workflow_run_registry, WorkflowRunRegistry,
};
use crate::app_server::runtime_loader::{RuntimeLoader, RuntimeSpawner};
use crate::app_server::thread_store::read_thread_state_from_storage;
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::llm::LlmClient;
#[cfg(test)]
use crate::mcp::client::McpClientFactory;
use crate::model::factory::{DefaultLlmClientFactory, LlmClientFactory, SharedLlmFactory};
use crate::policy::PolicyManager;
use crate::registry::ToolRegistry;
#[cfg(test)]
use crate::resolver::EnvModelResolver;
use crate::resolver::ModelResolver;
use crate::runtime::memory::MemoryRuntime;
use crate::runtime::subagent::{
    message_preview, AgentControl, CloseAgentResponse, CloseAgentsRequest,
    DeliverInterAgentMessageRequest, SendMessageResponse, SpawnAgentResponse,
    SpawnCleanChildRequest, SubagentLifecycle,
};
use crate::runtime::thread_runtime::{AgentFactory, WorkspaceRuntimeOpGate};
use crate::session::{ThreadSnapshot, ThreadSource};
use crate::state::fork_history::{build_fork_history, ForkTurns};
use crate::state::index_db::GoalAccountingMode;
use crate::state::rollout::{rollout_paths, thread_meta_from_snapshot, RolloutItem, RolloutStore};
use crate::state::spawn_edges::{SpawnEdgeStatus, ThreadSpawnEdge, ThreadSpawnEdgeStore};
use crate::types::ThreadId;

pub(in crate::app_server) type RegistryFactory = Arc<dyn Fn() -> ToolRegistry + Send + Sync>;

#[derive(Clone)]
pub(in crate::app_server) struct AppServerServices {
    pub(in crate::app_server) base_config: AgentConfig,
    pub(in crate::app_server) llm_factory: Arc<dyn LlmClientFactory>,
    pub(in crate::app_server) model_resolver: Arc<dyn ModelResolver>,
    pub(in crate::app_server) registry_factory: RegistryFactory,
    pub(in crate::app_server) exec_sessions: Arc<ExecSessionManager>,
    pub(in crate::app_server) policy: Arc<PolicyManager>,
    pub(in crate::app_server) runtime_loader: RuntimeLoader,
    pub(in crate::app_server) subagent_lifecycle: Arc<dyn SubagentLifecycle>,
    pub(in crate::app_server) workflow_runs: Arc<WorkflowRunRegistry>,
    pub(in crate::app_server) goal_store: Option<crate::index_db::IndexDb>,
    pub(in crate::app_server) memory_runtime: Option<Arc<MemoryRuntime>>,
    #[cfg(test)]
    pub(in crate::app_server) mcp_client_factory: Option<Arc<dyn McpClientFactory>>,
}

impl AppServerServices {
    pub(in crate::app_server) fn with_model_resolver(
        base_config: AgentConfig,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self {
        let llm_factory: Arc<dyn LlmClientFactory> = Arc::new(DefaultLlmClientFactory::default());
        Self::with_llm_factory_and_model_resolver(base_config, llm_factory, model_resolver)
    }

    pub(in crate::app_server) fn with_llm_factory_and_model_resolver(
        base_config: AgentConfig,
        llm_factory: Arc<dyn LlmClientFactory>,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self {
        let registry_factory: RegistryFactory = Arc::new(crate::default_tool_registry);
        let exec_sessions = Arc::new(ExecSessionManager::default());
        let policy = Arc::new(PolicyManager::default());
        let runtime_loader = RuntimeLoader::new();
        let agent_factory = runtime_agent_factory_from_parts(
            llm_factory.clone(),
            registry_factory.clone(),
            exec_sessions.clone(),
            policy.clone(),
            #[cfg(test)]
            None,
        );
        let subagent_lifecycle = new_subagent_lifecycle(
            runtime_loader.clone(),
            agent_factory,
            policy.clone(),
            model_resolver.clone(),
            None,
            None,
        );
        let workflow_runs = new_workflow_run_registry();
        Self {
            base_config,
            llm_factory,
            model_resolver,
            registry_factory,
            exec_sessions,
            policy,
            runtime_loader,
            subagent_lifecycle,
            workflow_runs,
            goal_store: None,
            memory_runtime: None,
            #[cfg(test)]
            mcp_client_factory: None,
        }
    }

    pub(in crate::app_server) fn with_llm_and_model_resolver<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
        model_resolver: Arc<dyn ModelResolver>,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        let llm_factory: Arc<dyn LlmClientFactory> =
            Arc::new(SharedLlmFactory::new(Arc::from(llm)));
        let registry_factory: RegistryFactory = Arc::new(registry_factory);
        let exec_sessions = Arc::new(ExecSessionManager::default());
        let policy = Arc::new(PolicyManager::default());
        let runtime_loader = RuntimeLoader::new();
        let agent_factory = runtime_agent_factory_from_parts(
            llm_factory.clone(),
            registry_factory.clone(),
            exec_sessions.clone(),
            policy.clone(),
            #[cfg(test)]
            None,
        );
        let subagent_lifecycle = new_subagent_lifecycle(
            runtime_loader.clone(),
            agent_factory,
            policy.clone(),
            model_resolver.clone(),
            None,
            None,
        );
        let workflow_runs = new_workflow_run_registry();
        Self {
            base_config,
            llm_factory,
            model_resolver,
            registry_factory,
            exec_sessions,
            policy,
            runtime_loader,
            subagent_lifecycle,
            workflow_runs,
            goal_store: None,
            memory_runtime: None,
            #[cfg(test)]
            mcp_client_factory: None,
        }
    }

    #[cfg(test)]
    pub(in crate::app_server) fn with_llm_and_mcp_client_factory_for_tests<F>(
        base_config: AgentConfig,
        llm: Box<dyn LlmClient>,
        registry_factory: F,
        mcp_client_factory: Arc<dyn McpClientFactory>,
    ) -> Self
    where
        F: Fn() -> ToolRegistry + Send + Sync + 'static,
    {
        let llm_factory: Arc<dyn LlmClientFactory> =
            Arc::new(SharedLlmFactory::new(Arc::from(llm)));
        let registry_factory: RegistryFactory = Arc::new(registry_factory);
        let exec_sessions = Arc::new(ExecSessionManager::default());
        let policy = Arc::new(PolicyManager::default());
        let runtime_loader = RuntimeLoader::new();
        let agent_factory = runtime_agent_factory_from_parts(
            llm_factory.clone(),
            registry_factory.clone(),
            exec_sessions.clone(),
            policy.clone(),
            Some(mcp_client_factory.clone()),
        );
        let model_resolver: Arc<dyn ModelResolver> = Arc::new(EnvModelResolver);
        let subagent_lifecycle = new_subagent_lifecycle(
            runtime_loader.clone(),
            agent_factory,
            policy.clone(),
            model_resolver.clone(),
            None,
            None,
        );
        let workflow_runs = new_workflow_run_registry();
        Self {
            base_config,
            llm_factory,
            model_resolver,
            registry_factory,
            exec_sessions,
            policy,
            runtime_loader,
            subagent_lifecycle,
            workflow_runs,
            goal_store: None,
            memory_runtime: None,
            mcp_client_factory: Some(mcp_client_factory),
        }
    }

    pub(in crate::app_server) fn runtime_agent_factory(&self) -> AgentFactory {
        runtime_agent_factory_from_parts(
            self.llm_factory.clone(),
            self.registry_factory.clone(),
            self.exec_sessions.clone(),
            self.policy.clone(),
            #[cfg(test)]
            self.mcp_client_factory.clone(),
        )
    }

    pub(in crate::app_server) fn with_goal_store(
        mut self,
        goal_store: crate::index_db::IndexDb,
    ) -> Self {
        let memory_runtime = MemoryRuntime::new(goal_store.clone());
        self.goal_store = Some(goal_store);
        self.memory_runtime = Some(memory_runtime);
        let agent_factory = self.runtime_agent_factory();
        self.subagent_lifecycle = new_subagent_lifecycle(
            self.runtime_loader.clone(),
            agent_factory,
            self.policy.clone(),
            self.model_resolver.clone(),
            self.goal_store.clone(),
            self.memory_runtime.clone(),
        );
        self
    }
}

fn new_subagent_lifecycle(
    runtime_loader: RuntimeLoader,
    agent_factory: AgentFactory,
    policy: Arc<PolicyManager>,
    model_resolver: Arc<dyn ModelResolver>,
    goal_store: Option<crate::index_db::IndexDb>,
    memory_runtime: Option<Arc<MemoryRuntime>>,
) -> Arc<dyn SubagentLifecycle> {
    let lifecycle: Arc<AppServerSubagentLifecycle> =
        Arc::<AppServerSubagentLifecycle>::new_cyclic(move |self_ref| {
            let self_lifecycle: Weak<dyn SubagentLifecycle> = self_ref.clone();
            AppServerSubagentLifecycle {
                runtime_loader: runtime_loader.clone(),
                agent_factory: agent_factory.clone(),
                policy: policy.clone(),
                model_resolver: model_resolver.clone(),
                goal_store: goal_store.clone(),
                memory_runtime: memory_runtime.clone(),
                self_lifecycle,
            }
        });
    lifecycle
}

fn rehydrated_subagent_control(
    lifecycle: Weak<dyn SubagentLifecycle>,
    workspace_root: &Path,
    requested_thread_id: &ThreadId,
) -> Result<Arc<AgentControl>> {
    let requested = read_thread_state_from_storage(workspace_root, requested_thread_id)?
        .ok_or_else(|| AppServerError::ThreadNotFound(requested_thread_id.clone()))?;
    let root_thread_id = requested
        .snapshot
        .lineage
        .as_ref()
        .map(|lineage| lineage.root_thread_id.clone())
        .unwrap_or_else(|| requested_thread_id.clone());
    let control = AgentControl::new_root(root_thread_id.clone(), lifecycle);

    if root_thread_id != *requested_thread_id {
        if let Some(root) = read_thread_state_from_storage(workspace_root, &root_thread_id)? {
            control.register_thread_from_snapshot(&root.snapshot);
        }
    }
    control.register_thread_from_snapshot(&requested.snapshot);

    let edge_store = ThreadSpawnEdgeStore::for_workspace(workspace_root);
    for edge in edge_store.list_by_root_blocking(&root_thread_id, Some(SpawnEdgeStatus::Open))? {
        if let Some(child) = read_thread_state_from_storage(workspace_root, &edge.child_thread_id)?
        {
            control.register_thread_from_snapshot(&child.snapshot);
        }
    }

    Ok(control)
}

impl RuntimeSpawner for AppServerServices {
    fn runtime_agent_factory(&self) -> AgentFactory {
        AppServerServices::runtime_agent_factory(self)
    }

    fn policy(&self) -> Arc<PolicyManager> {
        self.policy.clone()
    }

    fn workspace_runtime_op_gate(&self) -> Option<Arc<dyn WorkspaceRuntimeOpGate>> {
        Some(Arc::new(self.runtime_loader.clone()))
    }

    fn goal_store(&self) -> Option<crate::index_db::IndexDb> {
        self.goal_store.clone()
    }

    fn memory_runtime(&self) -> Option<Arc<MemoryRuntime>> {
        self.memory_runtime.clone()
    }

    fn forge_review_store(&self) -> Option<crate::runtime::forge::review::ReviewStore> {
        self.goal_store
            .clone()
            .map(crate::runtime::forge::review::ReviewStore::new)
    }

    fn subagent_control_for_cold_load(
        &self,
        workspace_root: &Path,
        thread_id: &ThreadId,
    ) -> Result<Arc<AgentControl>> {
        rehydrated_subagent_control(
            Arc::downgrade(&self.subagent_lifecycle),
            workspace_root,
            thread_id,
        )
    }
}

struct AppServerSubagentLifecycle {
    runtime_loader: RuntimeLoader,
    agent_factory: AgentFactory,
    policy: Arc<PolicyManager>,
    model_resolver: Arc<dyn ModelResolver>,
    goal_store: Option<crate::index_db::IndexDb>,
    memory_runtime: Option<Arc<MemoryRuntime>>,
    self_lifecycle: Weak<dyn SubagentLifecycle>,
}

impl RuntimeSpawner for AppServerSubagentLifecycle {
    fn runtime_agent_factory(&self) -> AgentFactory {
        self.agent_factory.clone()
    }

    fn policy(&self) -> Arc<PolicyManager> {
        self.policy.clone()
    }

    fn workspace_runtime_op_gate(&self) -> Option<Arc<dyn WorkspaceRuntimeOpGate>> {
        Some(Arc::new(self.runtime_loader.clone()))
    }

    fn goal_store(&self) -> Option<crate::index_db::IndexDb> {
        self.goal_store.clone()
    }

    fn memory_runtime(&self) -> Option<Arc<MemoryRuntime>> {
        self.memory_runtime.clone()
    }

    fn forge_review_store(&self) -> Option<crate::runtime::forge::review::ReviewStore> {
        self.goal_store
            .clone()
            .map(crate::runtime::forge::review::ReviewStore::new)
    }

    fn subagent_control_for_cold_load(
        &self,
        workspace_root: &Path,
        thread_id: &ThreadId,
    ) -> Result<Arc<AgentControl>> {
        rehydrated_subagent_control(self.self_lifecycle.clone(), workspace_root, thread_id)
    }
}

#[async_trait]
impl SubagentLifecycle for AppServerSubagentLifecycle {
    async fn spawn_clean_child(
        &self,
        request: SpawnCleanChildRequest,
        control: Arc<AgentControl>,
    ) -> Result<SpawnAgentResponse> {
        let thread_id = crate::transcript::new_thread_id();
        let mut child_config = request.config;
        if let Some(model_ref) = request.model.as_ref() {
            child_config.model = self.model_resolver.resolve(model_ref).await?;
        }
        if let Some(thinking_mode) = request.thinking_mode {
            child_config.thinking_mode = Some(thinking_mode);
        }
        let snapshot = ThreadSnapshot::new_thread_with_options(
            thread_id.clone(),
            child_config.workspace_root.clone(),
            child_config.cwd.clone(),
            child_config.permission_profile,
            ThreadSource::Subagent,
            Some(request.lineage.clone()),
        );
        let child_paths = rollout_paths(&snapshot.workspace_root, &thread_id);
        let mut child_rollout_items = vec![RolloutItem::ThreadMeta(thread_meta_from_snapshot(
            &snapshot,
        ))];
        if request.fork_turns != ForkTurns::None {
            let parent_paths =
                rollout_paths(&snapshot.workspace_root, &request.lineage.parent_thread_id);
            let parent_items = RolloutStore::read_items_blocking(&parent_paths.rollout_path)?;
            child_rollout_items.extend(build_fork_history(&parent_items, request.fork_turns));
        }
        RolloutStore::new(child_paths.rollout_path).append_items_blocking(&child_rollout_items)?;
        let runtime = self.runtime_loader.ensure_runtime_loaded_with_control(
            &thread_id,
            child_config,
            false,
            self,
            Some(control),
        )?;
        let edge_store = ThreadSpawnEdgeStore::for_workspace(&snapshot.workspace_root);
        edge_store.upsert_edge_blocking(ThreadSpawnEdge::open(
            request.lineage.parent_thread_id.clone(),
            thread_id.clone(),
            request.lineage.root_thread_id.clone(),
            request.lineage.agent_path.clone(),
        ))?;
        let turn_id = match runtime
            .submit_user_input(request.message.clone(), None)
            .await
        {
            Ok(turn_id) => turn_id,
            Err(err) => {
                let _ = edge_store.mark_closed_blocking(&thread_id);
                return Err(AppServerError::TurnRejected {
                    thread_id: thread_id.clone(),
                    reason: err.to_string(),
                }
                .into());
            }
        };
        Ok(SpawnAgentResponse {
            thread_id,
            parent_thread_id: request.lineage.parent_thread_id,
            root_thread_id: request.lineage.root_thread_id,
            turn_id,
            task_name: request.lineage.agent_path,
            message_preview: message_preview(&request.message),
            depth: request.lineage.depth,
        })
    }

    async fn close_agents(&self, request: CloseAgentsRequest) -> Result<CloseAgentResponse> {
        let edge_store = ThreadSpawnEdgeStore::for_workspace(&request.config.workspace_root);
        for target in &request.targets {
            let token_total = child_token_total_before_close(
                &self.runtime_loader,
                &request.config.workspace_root,
                &target.thread_id,
            )?;
            account_child_token_rollup(
                self.goal_store.as_ref(),
                &edge_store,
                &target.thread_id,
                token_total,
            )
            .await?;
            self.runtime_loader
                .shutdown_and_remove(&target.thread_id)
                .await?;
            edge_store.mark_closed_blocking(&target.thread_id)?;
        }
        Ok(CloseAgentResponse {
            parent_thread_id: request.parent_thread_id,
            root_thread_id: request.root_thread_id,
            closed_agents: request.targets,
        })
    }

    async fn deliver_inter_agent_message(
        &self,
        request: DeliverInterAgentMessageRequest,
    ) -> Result<SendMessageResponse> {
        let runtime = match self
            .runtime_loader
            .runtime_for(&request.mail.recipient_thread_id)
        {
            Some(runtime) => runtime,
            None => {
                let stored = read_thread_state_from_storage(
                    &request.config.workspace_root,
                    &request.mail.recipient_thread_id,
                )?
                .ok_or_else(|| {
                    AppServerError::ThreadNotFound(request.mail.recipient_thread_id.clone())
                })?;
                let config = self
                    .config_for_stored_thread(&request.config, &stored.snapshot)
                    .await?;
                self.runtime_loader.ensure_runtime_loaded_with_control(
                    &request.mail.recipient_thread_id,
                    config,
                    true,
                    self,
                    Some(request.control.clone()),
                )?
            }
        };
        runtime
            .enqueue_inter_agent_communication(request.mail.clone())
            .await?;

        let mut started_turn_id = None;
        let mut target_busy = false;
        if request.followup {
            if runtime.active_turn_id().is_some() {
                target_busy = true;
            } else {
                match runtime
                    .submit_user_input(followup_task_prompt(&request.mail.author_path), None)
                    .await
                {
                    Ok(turn_id) => started_turn_id = Some(turn_id),
                    Err(err) if err.to_string().contains("thread is busy") => {
                        target_busy = true;
                    }
                    Err(err) => return Err(err),
                }
            }
        }

        Ok(SendMessageResponse {
            mail: request.mail,
            followup: request.followup,
            started_turn_id,
            target_busy,
        })
    }
}

fn child_token_total_before_close(
    runtime_loader: &RuntimeLoader,
    workspace_root: &Path,
    child_thread_id: &ThreadId,
) -> Result<i64> {
    if let Some(runtime) = runtime_loader.runtime_for(child_thread_id) {
        return Ok(runtime
            .live_view()
            .snapshot
            .token_info
            .map(|info| info.total_token_usage.total_tokens)
            .unwrap_or_default());
    }
    Ok(
        read_thread_state_from_storage(workspace_root, child_thread_id)?
            .and_then(|stored| stored.snapshot.token_info)
            .map(|info| info.total_token_usage.total_tokens)
            .unwrap_or_default(),
    )
}

async fn account_child_token_rollup(
    goal_store: Option<&crate::index_db::IndexDb>,
    edge_store: &ThreadSpawnEdgeStore,
    child_thread_id: &ThreadId,
    token_total: i64,
) -> Result<()> {
    let Some(goal_store) = goal_store else {
        return Ok(());
    };
    if token_total <= 0 {
        return Ok(());
    }
    let Some(edge) = edge_store.mark_token_rollup_blocking(child_thread_id, token_total)? else {
        return Ok(());
    };
    if let Err(err) = goal_store
        .account_thread_goal_usage(
            &edge.parent_thread_id,
            0,
            token_total,
            GoalAccountingMode::ActiveOnly,
            None,
        )
        .await
    {
        let _ = edge_store.clear_token_rollup_blocking(child_thread_id, token_total);
        return Err(err.into());
    }
    Ok(())
}

impl AppServerSubagentLifecycle {
    async fn config_for_stored_thread(
        &self,
        base_config: &AgentConfig,
        snapshot: &ThreadSnapshot,
    ) -> Result<AgentConfig> {
        let mut config = base_config.clone();
        config.workspace_root = snapshot.workspace_root.clone();
        config.cwd = snapshot.cwd.clone();
        config.permission_profile = snapshot.permission_profile;

        if let Some(turn_context) = snapshot.reference_turn_context.as_ref() {
            config.workspace_root = turn_context.workspace_root.clone();
            config.cwd = turn_context.cwd.clone();
            config.permission_profile = turn_context.permission_profile;
            config.model = self.model_resolver.resolve(&turn_context.model).await?;
            config.thinking_mode = turn_context.thinking_mode;
            config.command_timeout_secs = turn_context.command_timeout_secs;
            config.max_output_bytes = turn_context.max_output_bytes;
            config.policy_mode = turn_context.policy_mode;
        }

        Ok(config)
    }
}

fn followup_task_prompt(author_path: &str) -> String {
    format!("Process the pending inter-agent message from {author_path}.")
}

fn runtime_agent_factory_from_parts(
    llm_factory: Arc<dyn LlmClientFactory>,
    registry_factory: RegistryFactory,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    #[cfg(test)] mcp_client_factory: Option<Arc<dyn McpClientFactory>>,
) -> AgentFactory {
    Arc::new(move |config: AgentConfig| -> Result<Agent> {
        let llm = llm_factory.build(&config.model)?;
        #[cfg(test)]
        if let Some(mcp_client_factory) = mcp_client_factory.clone() {
            return Ok(Agent::with_runtime_and_mcp_client_factory_for_tests(
                config,
                llm,
                (registry_factory)(),
                exec_sessions.clone(),
                policy.clone(),
                mcp_client_factory,
            ));
        }

        Ok(Agent::with_runtime(
            config,
            llm,
            (registry_factory)(),
            exec_sessions.clone(),
            policy.clone(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_db::{IndexDb, ProjectUpsert, ThreadGoalStatusRecord};

    async fn db_with_thread(
        thread_id: &ThreadId,
    ) -> (tempfile::TempDir, IndexDb, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Rollup".into(),
                path: workspace.clone(),
            })
            .await
            .unwrap();
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project.id)
        .bind(workspace.join("rollout.jsonl").display().to_string())
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
        (dir, db, workspace)
    }

    #[tokio::test]
    async fn child_token_rollup_counts_once_and_can_budget_limit_parent_goal() {
        let parent_thread_id = ThreadId::new("thread_rollup_parent");
        let (_dir, db, workspace) = db_with_thread(&parent_thread_id).await;
        let goal = db
            .insert_thread_goal(&parent_thread_id, "count child tokens", Some(100))
            .await
            .unwrap()
            .unwrap();
        let edge_store = ThreadSpawnEdgeStore::for_workspace(&workspace);
        let first_child = ThreadId::new("thread_rollup_child_1");
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                parent_thread_id.clone(),
                first_child.clone(),
                parent_thread_id.clone(),
                "/root/reviewer",
            ))
            .unwrap();

        account_child_token_rollup(Some(&db), &edge_store, &first_child, 60)
            .await
            .unwrap();
        account_child_token_rollup(Some(&db), &edge_store, &first_child, 60)
            .await
            .unwrap();

        let after_first = db
            .get_thread_goal(&parent_thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_first.goal_id, goal.goal_id);
        assert_eq!(after_first.tokens_used, 60);
        assert_eq!(after_first.status, ThreadGoalStatusRecord::Active);

        let second_child = ThreadId::new("thread_rollup_child_2");
        edge_store
            .upsert_edge_blocking(ThreadSpawnEdge::open(
                parent_thread_id.clone(),
                second_child.clone(),
                parent_thread_id.clone(),
                "/root/reviewer-2",
            ))
            .unwrap();
        account_child_token_rollup(Some(&db), &edge_store, &second_child, 50)
            .await
            .unwrap();

        let after_second = db
            .get_thread_goal(&parent_thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_second.tokens_used, 110);
        assert_eq!(after_second.status, ThreadGoalStatusRecord::BudgetLimited);
    }
}
