use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::app_server::protocol::{TurnState, TurnStatus};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::policy::PolicyManager;
use crate::runtime::goal::runtime::GoalRuntime;
use crate::runtime::subagent::AgentControl;
use crate::runtime::thread_runtime::{
    AgentFactory, ThreadRuntime, ThreadRuntimeOptions, WorkspaceRuntimeOpGate,
    WorkspaceRuntimeOpPermit,
};
use crate::types::{ThreadId, TurnId};

pub(in crate::app_server) trait RuntimeSpawner {
    fn runtime_agent_factory(&self) -> AgentFactory;
    fn policy(&self) -> Arc<PolicyManager>;
    fn workspace_runtime_op_gate(&self) -> Option<Arc<dyn WorkspaceRuntimeOpGate>> {
        None
    }
    fn goal_store(&self) -> Option<crate::index_db::IndexDb> {
        None
    }
    fn forge_review_store(&self) -> Option<crate::runtime::forge::review::ReviewStore> {
        None
    }
    fn subagent_control_for_cold_load(
        &self,
        workspace_root: &Path,
        thread_id: &ThreadId,
    ) -> Result<Arc<AgentControl>>;
}

pub(in crate::app_server) struct LoadedRuntime {
    pub(in crate::app_server) runtime: Arc<ThreadRuntime>,
    pub(in crate::app_server) workspace_root: PathBuf,
}

#[derive(Clone, Default)]
pub(in crate::app_server) struct RuntimeLoader {
    loaded_threads: Arc<Mutex<HashMap<String, Arc<ThreadRuntime>>>>,
    loading_threads: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    thread_operation_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    restore_state: Arc<Mutex<WorkspaceRestoreState>>,
}

#[derive(Default)]
struct WorkspaceRestoreState {
    restoring: HashSet<PathBuf>,
    runtime_ops: HashMap<PathBuf, usize>,
}

pub(in crate::app_server) struct WorkspaceRestoreGuard {
    loader: RuntimeLoader,
    workspace_root: PathBuf,
}

pub(in crate::app_server) struct WorkspaceRuntimeOpGuard {
    loader: RuntimeLoader,
    workspace_root: PathBuf,
}

pub(in crate::app_server) struct ThreadRuntimeOpGuard {
    _guard: OwnedMutexGuard<()>,
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

        // Invariant: a loaded runtime always carries an AgentControl. A runtime
        // cached without one would silently drop the subagent tools from every
        // later turn on this thread, so when the caller has no control to hand
        // down (anything other than spawning a child agent) the spawner must
        // provide one for the cold load, regardless of which request path loads
        // the thread first.
        let subagent_control = match subagent_control {
            Some(control) => control,
            None => spawner.subagent_control_for_cold_load(&config.workspace_root, thread_id)?,
        };
        let mut options =
            ThreadRuntimeOptions::new(thread_id.clone(), config, spawner.runtime_agent_factory())
                .with_policy(spawner.policy())
                .with_subagent_control(subagent_control);
        if let Some(gate) = spawner.workspace_runtime_op_gate() {
            options = options.with_workspace_runtime_op_gate(gate);
        }
        if let Some(goal_store) = spawner.goal_store() {
            options = options.with_goal_runtime(Arc::new(GoalRuntime::new(goal_store)));
        }
        if let Some(review_store) = spawner.forge_review_store() {
            options = options.with_forge_review_store(review_store);
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

    pub(in crate::app_server) async fn begin_thread_runtime_op(
        &self,
        thread_id: &ThreadId,
    ) -> Result<ThreadRuntimeOpGuard> {
        let lock = {
            let mut locks = self
                .thread_operation_locks
                .lock()
                .expect("thread operation lock map poisoned");
            locks
                .entry(thread_id.as_str().to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        Ok(ThreadRuntimeOpGuard {
            _guard: lock.lock_owned().await,
        })
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

    pub(in crate::app_server) fn active_turn_in_workspace(
        &self,
        workspace_root: &Path,
    ) -> Option<(ThreadId, TurnId)> {
        let loaded: Vec<(String, Arc<ThreadRuntime>)> = self
            .loaded_threads
            .lock()
            .expect("loaded threads mutex poisoned")
            .iter()
            .map(|(thread_id, runtime)| (thread_id.clone(), runtime.clone()))
            .collect();

        loaded.into_iter().find_map(|(thread_id, runtime)| {
            let live_workspace_root = runtime.live_view().snapshot.workspace_root;
            if live_workspace_root == workspace_root {
                runtime
                    .active_turn_id()
                    .map(|turn_id| (ThreadId::new(thread_id), turn_id))
            } else {
                None
            }
        })
    }

    pub(in crate::app_server) fn loaded_thread_ids_in_workspace(
        &self,
        workspace_root: &Path,
    ) -> Vec<ThreadId> {
        let loaded: Vec<(String, Arc<ThreadRuntime>)> = self
            .loaded_threads
            .lock()
            .expect("loaded threads mutex poisoned")
            .iter()
            .map(|(thread_id, runtime)| (thread_id.clone(), runtime.clone()))
            .collect();

        loaded
            .into_iter()
            .filter_map(|(thread_id, runtime)| {
                let live_workspace_root = runtime.live_view().snapshot.workspace_root;
                (live_workspace_root == workspace_root).then(|| ThreadId::new(thread_id))
            })
            .collect()
    }

    pub(in crate::app_server) fn begin_workspace_restore(
        &self,
        workspace_root: &Path,
    ) -> Result<WorkspaceRestoreGuard> {
        let workspace_root = workspace_root.to_path_buf();
        {
            let mut state = self
                .restore_state
                .lock()
                .expect("workspace restore state mutex poisoned");
            if state.restoring.contains(&workspace_root) {
                return Err(AppServerError::InvalidRequest(format!(
                    "checkpoint restore is already in progress for workspace `{}`",
                    workspace_root.display()
                ))
                .into());
            }
            if state.runtime_ops.get(&workspace_root).copied().unwrap_or(0) > 0 {
                return Err(AppServerError::InvalidRequest(format!(
                    "cannot restore checkpoint in workspace `{}` while a runtime operation is starting",
                    workspace_root.display()
                ))
                .into());
            }
            state.restoring.insert(workspace_root.clone());
        }

        let guard = WorkspaceRestoreGuard {
            loader: self.clone(),
            workspace_root,
        };

        if let Some((thread_id, turn_id)) =
            guard.loader.active_turn_in_workspace(&guard.workspace_root)
        {
            let workspace_root = guard.workspace_root.clone();
            drop(guard);
            return Err(AppServerError::InvalidRequest(format!(
                "cannot restore checkpoint in workspace `{}` while turn is running in thread {} ({})",
                workspace_root.display(),
                thread_id.as_str(),
                turn_id.as_str()
            ))
            .into());
        }

        Ok(guard)
    }

    pub(in crate::app_server) fn begin_workspace_runtime_op(
        &self,
        workspace_root: &Path,
    ) -> Result<WorkspaceRuntimeOpGuard> {
        let workspace_root = workspace_root.to_path_buf();
        let mut state = self
            .restore_state
            .lock()
            .expect("workspace restore state mutex poisoned");
        if state.restoring.contains(&workspace_root) {
            return Err(AppServerError::InvalidRequest(format!(
                "checkpoint restore is in progress for workspace `{}`",
                workspace_root.display()
            ))
            .into());
        }
        *state.runtime_ops.entry(workspace_root.clone()).or_insert(0) += 1;
        Ok(WorkspaceRuntimeOpGuard {
            loader: self.clone(),
            workspace_root,
        })
    }
}

impl Drop for WorkspaceRestoreGuard {
    fn drop(&mut self) {
        let mut state = self
            .loader
            .restore_state
            .lock()
            .expect("workspace restore state mutex poisoned");
        state.restoring.remove(&self.workspace_root);
    }
}

impl Drop for WorkspaceRuntimeOpGuard {
    fn drop(&mut self) {
        let mut state = self
            .loader
            .restore_state
            .lock()
            .expect("workspace restore state mutex poisoned");
        let Some(count) = state.runtime_ops.get_mut(&self.workspace_root) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            state.runtime_ops.remove(&self.workspace_root);
        }
    }
}

impl WorkspaceRuntimeOpGate for RuntimeLoader {
    fn begin_runtime_op(&self, workspace_root: &Path) -> Result<WorkspaceRuntimeOpPermit> {
        self.begin_workspace_runtime_op(workspace_root)
            .map(|guard| Box::new(guard) as WorkspaceRuntimeOpPermit)
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
