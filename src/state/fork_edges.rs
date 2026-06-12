use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::types::{ThreadId, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadForkEdge {
    pub parent_thread_id: ThreadId,
    pub child_thread_id: ThreadId,
    pub fork_point_turn_id: TurnId,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ThreadForkEdgeStore {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ThreadForkEdgeFile {
    #[serde(default)]
    edges: Vec<ThreadForkEdge>,
}

static FORK_EDGE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

impl ThreadForkEdgeStore {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self::new(fork_edges_path(workspace_root))
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn read_edges_blocking(&self) -> std::io::Result<Vec<ThreadForkEdge>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let file: ThreadForkEdgeFile =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        Ok(file.edges)
    }

    pub fn upsert_edge_blocking(&self, edge: ThreadForkEdge) -> std::io::Result<()> {
        self.update_edges_blocking(|edges| {
            if let Some(existing) = edges
                .iter_mut()
                .find(|existing| existing.child_thread_id == edge.child_thread_id)
            {
                *existing = edge;
            } else {
                edges.push(edge);
            }
            Ok(())
        })
    }

    pub fn list_by_parent_blocking(
        &self,
        parent_thread_id: &ThreadId,
    ) -> std::io::Result<Vec<ThreadForkEdge>> {
        self.list_edges_blocking(|edge| &edge.parent_thread_id == parent_thread_id)
    }

    pub fn list_for_workspace_blocking(&self) -> std::io::Result<Vec<ThreadForkEdge>> {
        self.read_edges_blocking()
    }

    fn list_edges_blocking(
        &self,
        predicate: impl Fn(&ThreadForkEdge) -> bool,
    ) -> std::io::Result<Vec<ThreadForkEdge>> {
        Ok(self
            .read_edges_blocking()?
            .into_iter()
            .filter(predicate)
            .collect())
    }

    fn update_edges_blocking<T>(
        &self,
        update: impl FnOnce(&mut Vec<ThreadForkEdge>) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let lock = self.mutation_lock();
        let _guard = lock
            .lock()
            .map_err(|_| std::io::Error::other("fork edge store lock poisoned"))?;
        let mut edges = self.read_edges_blocking()?;
        let result = update(&mut edges)?;
        self.write_edges_blocking(&edges)?;
        Ok(result)
    }

    fn mutation_lock(&self) -> Arc<Mutex<()>> {
        let locks = FORK_EDGE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut locks = locks.lock().expect("fork edge lock map poisoned");
        locks
            .entry(self.path.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn write_edges_blocking(&self, edges: &[ThreadForkEdge]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ThreadForkEdgeFile {
            edges: edges.to_vec(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, text)?;
        std::fs::rename(tmp_path, &self.path)
    }
}

pub fn fork_edges_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".exagent")
        .join("threads")
        .join("fork_edges.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_lists_edges_by_parent_and_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadForkEdgeStore::for_workspace(dir.path());
        let parent = ThreadId::new("thread_parent");
        let edge = ThreadForkEdge {
            parent_thread_id: parent.clone(),
            child_thread_id: ThreadId::new("thread_child"),
            fork_point_turn_id: TurnId::new("turn_2"),
            created_at_ms: 1_700_000_000_000,
        };
        let other_edge = ThreadForkEdge {
            parent_thread_id: ThreadId::new("thread_other_parent"),
            child_thread_id: ThreadId::new("thread_other_child"),
            fork_point_turn_id: TurnId::new("turn_4"),
            created_at_ms: 1_700_000_001_000,
        };

        store
            .upsert_edge_blocking(edge.clone())
            .expect("upsert edge");
        store
            .upsert_edge_blocking(other_edge.clone())
            .expect("upsert other edge");

        assert_eq!(
            store
                .list_by_parent_blocking(&parent)
                .expect("list parent edges"),
            vec![edge.clone()]
        );
        assert_eq!(
            store
                .list_for_workspace_blocking()
                .expect("list workspace edges"),
            vec![edge, other_edge]
        );
    }

    #[test]
    fn store_returns_empty_edges_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadForkEdgeStore::for_workspace(dir.path());

        assert_eq!(
            store
                .list_for_workspace_blocking()
                .expect("list missing workspace file"),
            Vec::<ThreadForkEdge>::new()
        );
        assert_eq!(
            store
                .list_by_parent_blocking(&ThreadId::new("thread_parent"))
                .expect("list missing parent file"),
            Vec::<ThreadForkEdge>::new()
        );
    }

    #[test]
    fn store_upserts_edges_by_child_thread_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadForkEdgeStore::for_workspace(dir.path());
        let child = ThreadId::new("thread_child");
        let first = ThreadForkEdge {
            parent_thread_id: ThreadId::new("thread_parent"),
            child_thread_id: child.clone(),
            fork_point_turn_id: TurnId::new("turn_2"),
            created_at_ms: 1_700_000_000_000,
        };
        let replacement = ThreadForkEdge {
            parent_thread_id: ThreadId::new("thread_new_parent"),
            child_thread_id: child,
            fork_point_turn_id: TurnId::new("turn_3"),
            created_at_ms: 1_700_000_002_000,
        };

        store
            .upsert_edge_blocking(first)
            .expect("upsert first edge");
        store
            .upsert_edge_blocking(replacement.clone())
            .expect("upsert replacement edge");

        assert_eq!(
            store
                .list_for_workspace_blocking()
                .expect("list workspace edges"),
            vec![replacement]
        );
    }
}
