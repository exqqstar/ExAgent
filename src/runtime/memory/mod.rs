use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub mod context;

use crate::index_db::IndexDb;

pub struct MemoryRuntime {
    db: IndexDb,
    project_id_cache: Mutex<HashMap<PathBuf, String>>,
}

impl MemoryRuntime {
    pub fn new(db: IndexDb) -> Arc<Self> {
        Arc::new(Self {
            db,
            project_id_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn db(&self) -> &IndexDb {
        &self.db
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
