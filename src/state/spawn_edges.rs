use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::types::ThreadId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpawnEdgeStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSpawnEdge {
    pub parent_thread_id: ThreadId,
    pub child_thread_id: ThreadId,
    pub root_thread_id: ThreadId,
    pub agent_path: String,
    pub status: SpawnEdgeStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
}

impl ThreadSpawnEdge {
    pub fn open(
        parent_thread_id: ThreadId,
        child_thread_id: ThreadId,
        root_thread_id: ThreadId,
        agent_path: impl Into<String>,
    ) -> Self {
        Self {
            parent_thread_id,
            child_thread_id,
            root_thread_id,
            agent_path: agent_path.into(),
            status: SpawnEdgeStatus::Open,
            created_at: current_utc_timestamp(),
            closed_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThreadSpawnEdgeStore {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ThreadSpawnEdgeFile {
    #[serde(default)]
    edges: Vec<ThreadSpawnEdge>,
}

static SPAWN_EDGE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

impl ThreadSpawnEdgeStore {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self::new(spawn_edges_path(workspace_root))
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn read_edges_blocking(&self) -> std::io::Result<Vec<ThreadSpawnEdge>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let file: ThreadSpawnEdgeFile =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        Ok(file.edges)
    }

    pub fn upsert_edge_blocking(&self, edge: ThreadSpawnEdge) -> std::io::Result<()> {
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

    pub fn mark_closed_blocking(
        &self,
        child_thread_id: &ThreadId,
    ) -> std::io::Result<Option<ThreadSpawnEdge>> {
        self.update_edges_blocking(|edges| {
            let Some(edge) = edges
                .iter_mut()
                .find(|edge| &edge.child_thread_id == child_thread_id)
            else {
                return Ok(None);
            };
            edge.status = SpawnEdgeStatus::Closed;
            edge.closed_at = Some(current_utc_timestamp());
            Ok(Some(edge.clone()))
        })
    }

    pub fn list_by_parent_blocking(
        &self,
        parent_thread_id: &ThreadId,
        status: Option<SpawnEdgeStatus>,
    ) -> std::io::Result<Vec<ThreadSpawnEdge>> {
        self.list_edges_blocking(|edge| {
            &edge.parent_thread_id == parent_thread_id && status_matches(edge, status)
        })
    }

    pub fn list_by_root_blocking(
        &self,
        root_thread_id: &ThreadId,
        status: Option<SpawnEdgeStatus>,
    ) -> std::io::Result<Vec<ThreadSpawnEdge>> {
        self.list_edges_blocking(|edge| {
            &edge.root_thread_id == root_thread_id && status_matches(edge, status)
        })
    }

    fn list_edges_blocking(
        &self,
        predicate: impl Fn(&ThreadSpawnEdge) -> bool,
    ) -> std::io::Result<Vec<ThreadSpawnEdge>> {
        Ok(self
            .read_edges_blocking()?
            .into_iter()
            .filter(predicate)
            .collect())
    }

    fn update_edges_blocking<T>(
        &self,
        update: impl FnOnce(&mut Vec<ThreadSpawnEdge>) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let lock = self.mutation_lock();
        let _guard = lock
            .lock()
            .map_err(|_| std::io::Error::other("spawn edge store lock poisoned"))?;
        let mut edges = self.read_edges_blocking()?;
        let result = update(&mut edges)?;
        self.write_edges_blocking(&edges)?;
        Ok(result)
    }

    fn mutation_lock(&self) -> Arc<Mutex<()>> {
        let locks = SPAWN_EDGE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut locks = locks.lock().expect("spawn edge lock map poisoned");
        locks
            .entry(self.path.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn write_edges_blocking(&self, edges: &[ThreadSpawnEdge]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ThreadSpawnEdgeFile {
            edges: edges.to_vec(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
        let tmp_path = self.path.with_extension("json.tmp");
        std::fs::write(&tmp_path, text)?;
        std::fs::rename(tmp_path, &self.path)
    }
}

pub fn spawn_edges_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".exagent")
        .join("threads")
        .join("spawn_edges.json")
}

fn status_matches(edge: &ThreadSpawnEdge, status: Option<SpawnEdgeStatus>) -> bool {
    status.is_none_or(|status| edge.status == status)
}

fn current_utc_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn store_lists_edges_by_parent_and_root_status() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        let parent = ThreadId::new("thread_parent");
        let child = ThreadId::new("thread_child");
        let root = ThreadId::new("thread_root");
        let edge = ThreadSpawnEdge::open(
            parent.clone(),
            child.clone(),
            root.clone(),
            "/root/research",
        );

        store
            .upsert_edge_blocking(edge.clone())
            .expect("upsert edge");

        assert_eq!(
            store
                .list_by_parent_blocking(&parent, Some(SpawnEdgeStatus::Open))
                .expect("list parent edges"),
            vec![edge.clone()]
        );
        assert_eq!(
            store
                .list_by_root_blocking(&root, None)
                .expect("list root edges"),
            vec![edge.clone()]
        );
        assert!(store
            .list_by_parent_blocking(&ThreadId::new("other_parent"), None)
            .expect("list other parent")
            .is_empty());
    }

    #[test]
    fn store_marks_edges_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        let edge = ThreadSpawnEdge::open(
            ThreadId::new("thread_parent"),
            ThreadId::new("thread_child"),
            ThreadId::new("thread_root"),
            "/root/research",
        );
        let created_at = edge.created_at.clone();
        store.upsert_edge_blocking(edge).expect("upsert edge");

        let closed = store
            .mark_closed_blocking(&ThreadId::new("thread_child"))
            .expect("close edge")
            .expect("edge exists");

        assert_eq!(closed.status, SpawnEdgeStatus::Closed);
        assert_eq!(closed.created_at, created_at);
        assert!(closed.closed_at.is_some());
        assert!(store
            .list_by_root_blocking(&ThreadId::new("thread_root"), Some(SpawnEdgeStatus::Open))
            .expect("list open root")
            .is_empty());
    }

    #[test]
    fn store_serializes_concurrent_upserts() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThreadSpawnEdgeStore::for_workspace(dir.path());
        let barrier = Arc::new(Barrier::new(16));
        let handles = (0..16)
            .map(|index| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.upsert_edge_blocking(ThreadSpawnEdge::open(
                        ThreadId::new("thread_parent"),
                        ThreadId::new(format!("thread_child_{index}")),
                        ThreadId::new("thread_root"),
                        format!("/root/child_{index}"),
                    ))
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("join worker").expect("upsert edge");
        }

        let edges = store.read_edges_blocking().expect("read edges");
        assert_eq!(edges.len(), 16);
        for index in 0..16 {
            assert!(edges.iter().any(|edge| {
                edge.child_thread_id == ThreadId::new(format!("thread_child_{index}"))
            }));
        }
    }
}
