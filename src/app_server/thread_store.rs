use std::path::Path;

use anyhow::Result;

use crate::events::RuntimeEvent;
use crate::session::ThreadSnapshot;
use crate::state::rollout::{
    events_from_rollout_items, response_items_from_rollout_items, rollout_paths,
    snapshot_from_rollout_items, ResponseItem, RolloutStore,
};
use crate::types::ThreadId;

pub(in crate::app_server) struct StoredThreadState {
    pub(in crate::app_server) snapshot: ThreadSnapshot,
    pub(in crate::app_server) response_items: Vec<ResponseItem>,
    pub(in crate::app_server) events: Vec<RuntimeEvent>,
}

pub(in crate::app_server) fn thread_exists_in_storage(
    workspace_root: &Path,
    thread_id: &ThreadId,
) -> bool {
    let rollout_paths = rollout_paths(workspace_root, thread_id);
    std::fs::metadata(&rollout_paths.rollout_path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}

pub(in crate::app_server) fn read_thread_state_from_storage(
    workspace_root: &Path,
    thread_id: &ThreadId,
) -> Result<Option<StoredThreadState>> {
    let rollout_paths = rollout_paths(workspace_root, thread_id);
    let rollout_items = RolloutStore::read_items_blocking(&rollout_paths.rollout_path)?;
    if rollout_items.is_empty() {
        return Ok(None);
    }

    Ok(Some(StoredThreadState {
        snapshot: snapshot_from_rollout_items(thread_id, &rollout_items)?,
        response_items: response_items_from_rollout_items(&rollout_items),
        events: events_from_rollout_items(&rollout_items),
    }))
}
