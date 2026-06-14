use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::mpsc;

pub mod context;

use crate::index_db::IndexDb;
use crate::state::memory::projector::project_memory_observations_from_rollout;
use crate::state::rollout::RolloutStore;
use crate::types::ThreadId;

#[derive(Debug, Clone)]
pub struct MemoryProjectionRequest {
    pub workspace_root: PathBuf,
    pub project_id: Option<String>,
    pub thread_id: ThreadId,
    pub rollout_path: PathBuf,
}

pub struct MemoryRuntime {
    db: IndexDb,
    projection_tx: mpsc::UnboundedSender<MemoryProjectionRequest>,
    project_id_cache: Mutex<HashMap<PathBuf, String>>,
}

impl MemoryRuntime {
    pub fn new(db: IndexDb) -> Arc<Self> {
        let (projection_tx, projection_rx) = mpsc::unbounded_channel();
        let runtime = Arc::new(Self {
            db,
            projection_tx,
            project_id_cache: Mutex::new(HashMap::new()),
        });
        spawn_projection_worker(Arc::downgrade(&runtime), projection_rx);
        runtime
    }

    pub fn db(&self) -> &IndexDb {
        &self.db
    }

    pub fn enqueue_projection(&self, request: MemoryProjectionRequest) {
        let _ = self.projection_tx.send(request);
    }

    pub async fn resolve_project_id_cached(
        &self,
        workspace_root: &Path,
    ) -> anyhow::Result<Option<String>> {
        let cache_key = tokio::fs::canonicalize(workspace_root).await?;
        if let Some(project_id) = self
            .project_id_cache
            .lock()
            .expect("memory project id cache mutex poisoned")
            .get(&cache_key)
            .cloned()
        {
            return Ok(Some(project_id));
        }

        let project_id = self.db.project_id_for_existing_path(&cache_key).await?;
        if let Some(project_id) = project_id.as_ref() {
            self.project_id_cache
                .lock()
                .expect("memory project id cache mutex poisoned")
                .insert(cache_key, project_id.clone());
        }
        Ok(project_id)
    }

    pub async fn project_thread_incremental(
        &self,
        project_id: Option<&str>,
        thread_id: &ThreadId,
        rollout_path: &Path,
    ) -> anyhow::Result<()> {
        let start_index = self.db.memory_projection_start_index(thread_id).await?;
        let (items, end_index) =
            RolloutStore::read_items_from_index(rollout_path, start_index).await?;
        let observations = project_memory_observations_from_rollout(
            project_id,
            thread_id,
            &items,
            0,
            now_unix_millis(),
        );
        self.db
            .upsert_memory_observations_incremental(observations)
            .await?;
        self.db
            .set_memory_projection_cursor(thread_id, rollout_path, end_index)
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct MemoryToolApi {
    runtime: Arc<MemoryRuntime>,
}

impl MemoryToolApi {
    pub fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &Arc<MemoryRuntime> {
        &self.runtime
    }
}

fn spawn_projection_worker(
    runtime: Weak<MemoryRuntime>,
    mut projection_rx: mpsc::UnboundedReceiver<MemoryProjectionRequest>,
) {
    let worker = async move {
        while let Some(request) = projection_rx.recv().await {
            let Some(runtime) = runtime.upgrade() else {
                break;
            };
            let project_id = match request.project_id {
                Some(project_id) => Some(project_id),
                None => match runtime
                    .resolve_project_id_cached(&request.workspace_root)
                    .await
                {
                    Ok(project_id) => project_id,
                    Err(_) => None,
                },
            };
            let _ = runtime
                .project_thread_incremental(
                    project_id.as_deref(),
                    &request.thread_id,
                    &request.rollout_path,
                )
                .await;
        }
    };

    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(worker);
        }
        Err(_) => {
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build memory projection worker runtime");
                runtime.block_on(worker);
            });
        }
    }
}

fn now_unix_millis() -> i64 {
    let now = time::OffsetDateTime::now_utc();
    (now.unix_timestamp_nanos() / 1_000_000) as i64
}
