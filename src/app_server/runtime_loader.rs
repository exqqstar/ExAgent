use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::app_server::protocol::{TurnState, TurnStatus};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::policy::PolicyManager;
use crate::runtime::goal::runtime::GoalRuntime;
use crate::runtime::subagent::AgentControl;
use crate::runtime::thread_runtime::{AgentFactory, ThreadRuntime, ThreadRuntimeOptions};
use crate::types::ThreadId;

pub(in crate::app_server) trait RuntimeSpawner {
    fn runtime_agent_factory(&self) -> AgentFactory;
    fn policy(&self) -> Arc<PolicyManager>;
    fn goal_store(&self) -> Option<crate::index_db::IndexDb> {
        None
    }
}

pub(in crate::app_server) struct LoadedRuntime {
    pub(in crate::app_server) runtime: Arc<ThreadRuntime>,
    pub(in crate::app_server) workspace_root: PathBuf,
}

#[derive(Clone, Default)]
pub(in crate::app_server) struct RuntimeLoader {
    loaded_threads: Arc<Mutex<HashMap<String, Arc<ThreadRuntime>>>>,
    loading_threads: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl RuntimeLoader {
    pub(in crate::app_server) fn new() -> Self {
        Self::default()
    }

    pub(in crate::app_server) fn runtime_for(
        &self,
        thread_id: &ThreadId,
    ) -> Option<Arc<ThreadRuntime>> {
        self.loaded_threads
            .lock()
            .ok()
            .and_then(|loaded_threads| loaded_threads.get(thread_id.as_str()).cloned())
    }

    pub(in crate::app_server) async fn shutdown_and_remove(
        &self,
        thread_id: &ThreadId,
    ) -> Result<bool> {
        let runtime = self
            .loaded_threads
            .lock()
            .expect("loaded threads mutex poisoned")
            .remove(thread_id.as_str());
        let Some(runtime) = runtime else {
            return Ok(false);
        };
        runtime.shutdown().await?;
        Ok(true)
    }

    pub(in crate::app_server) fn resolve_loaded_runtime(
        &self,
        thread_id: &ThreadId,
        requested_workspace_root: bool,
        workspace_root: &Path,
    ) -> Result<Option<LoadedRuntime>> {
        let Some(runtime) = self.runtime_for(thread_id) else {
            return Ok(None);
        };
        let live_workspace_root = runtime.live_view().snapshot.workspace_root;
        if requested_workspace_root && live_workspace_root != workspace_root {
            return Err(workspace_mismatch_error(
                thread_id,
                workspace_root,
                &live_workspace_root,
            ));
        }
        Ok(Some(LoadedRuntime {
            runtime,
            workspace_root: live_workspace_root,
        }))
    }

    pub(in crate::app_server) fn ensure_runtime_loaded(
        &self,
        thread_id: &ThreadId,
        config: AgentConfig,
        requested_workspace_root: bool,
        spawner: &impl RuntimeSpawner,
    ) -> Result<Arc<ThreadRuntime>> {
        self.ensure_runtime_loaded_with_control(
            thread_id,
            config,
            requested_workspace_root,
            spawner,
            None,
        )
    }

    pub(in crate::app_server) fn ensure_runtime_loaded_with_control(
        &self,
        thread_id: &ThreadId,
        config: AgentConfig,
        requested_workspace_root: bool,
        spawner: &impl RuntimeSpawner,
        subagent_control: Option<Arc<AgentControl>>,
    ) -> Result<Arc<ThreadRuntime>> {
        if let Some(runtime) = self.runtime_for(thread_id) {
            return validate_loaded_runtime(
                thread_id,
                runtime,
                requested_workspace_root,
                &config.workspace_root,
            );
        }

        let thread_key = thread_id.as_str().to_string();
        let load_lock = self.loading_lock(&thread_key);
        let _load_guard = load_lock.lock().expect("runtime load mutex poisoned");

        if let Some(runtime) = self.runtime_for(thread_id) {
            return validate_loaded_runtime(
                thread_id,
                runtime,
                requested_workspace_root,
                &config.workspace_root,
            );
        }

        let mut options =
            ThreadRuntimeOptions::new(thread_id.clone(), config, spawner.runtime_agent_factory())
                .with_policy(spawner.policy());
        if let Some(goal_store) = spawner.goal_store() {
            options = options.with_goal_runtime(Arc::new(GoalRuntime::new(goal_store)));
        }
        if let Some(subagent_control) = subagent_control {
            options = options.with_subagent_control(subagent_control);
        }
        let spawn_result = ThreadRuntime::spawn(options);
        let runtime = match spawn_result {
            Ok(runtime) => runtime,
            Err(err) => return Err(err),
        };
        {
            let mut loaded_threads = self
                .loaded_threads
                .lock()
                .expect("loaded threads mutex poisoned");
            loaded_threads.insert(thread_key.clone(), runtime.clone());
        }
        Ok(runtime)
    }

    fn loading_lock(&self, thread_key: &str) -> Arc<Mutex<()>> {
        let mut loading_threads = self
            .loading_threads
            .lock()
            .expect("loading threads mutex poisoned");
        loading_threads
            .entry(thread_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(in crate::app_server) fn active_turn_state(
        &self,
        thread_id: &ThreadId,
    ) -> Option<TurnState> {
        self.runtime_for(thread_id)
            .and_then(|runtime| runtime.active_turn_id())
            .map(|turn_id| TurnState {
                turn_id,
                status: TurnStatus::InProgress,
            })
    }
}

fn validate_loaded_runtime(
    thread_id: &ThreadId,
    runtime: Arc<ThreadRuntime>,
    requested_workspace_root: bool,
    workspace_root: &Path,
) -> Result<Arc<ThreadRuntime>> {
    let live_workspace_root = runtime.live_view().snapshot.workspace_root;
    if requested_workspace_root && live_workspace_root != workspace_root {
        return Err(workspace_mismatch_error(
            thread_id,
            workspace_root,
            &live_workspace_root,
        ));
    }

    Ok(runtime)
}

fn workspace_mismatch_error(
    thread_id: &ThreadId,
    requested_workspace_root: &Path,
    active_workspace_root: &Path,
) -> anyhow::Error {
    AppServerError::InvalidRequest(format!(
        "thread {} belongs to workspace `{}`, but request targeted workspace `{}`",
        thread_id.as_str(),
        active_workspace_root.display(),
        requested_workspace_root.display()
    ))
    .into()
}
