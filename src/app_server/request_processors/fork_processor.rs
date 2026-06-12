use anyhow::{anyhow, Result};

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{ThreadForkParams, ThreadForkResponse};
use crate::app_server::services::AppServerServices;
use crate::app_server::AppServerError;
use crate::session::{ThreadSnapshot, ThreadSource};
use crate::state::fork_edges::{ThreadForkEdge, ThreadForkEdgeStore};
use crate::state::fork_history::build_thread_fork_history;
use crate::state::rollout::{
    rollout_paths, snapshot_from_rollout_items, thread_meta_from_snapshot, RolloutItem,
    RolloutStore,
};

pub(in crate::app_server) async fn thread_fork(
    services: &AppServerServices,
    params: ThreadForkParams,
) -> Result<ThreadForkResponse> {
    let requested_workspace_root = params.workspace_root.clone().is_some();
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let _thread_op_guard = services
        .runtime_loader
        .begin_thread_runtime_op(&params.thread_id)
        .await?;
    let workspace_root = services
        .runtime_loader
        .resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )?
        .map(|loaded| loaded.workspace_root)
        .unwrap_or(config.workspace_root);

    if services
        .runtime_loader
        .active_turn_state(&params.thread_id)
        .is_some()
    {
        return Err(AppServerError::InvalidRequest(
            "cannot fork while a turn is in progress".to_string(),
        )
        .into());
    }

    let parent_paths = rollout_paths(&workspace_root, &params.thread_id);
    let parent_items = RolloutStore::read_items_blocking(&parent_paths.rollout_path)?;
    if parent_items.is_empty() {
        return Err(AppServerError::ThreadNotFound(params.thread_id).into());
    }
    let parent_snapshot = snapshot_from_rollout_items(&params.thread_id, &parent_items)?;
    let fork_history = build_thread_fork_history(&parent_items, &params.at_turn_id)?;
    let created_at_ms = current_unix_ms()?;

    let new_thread_id = crate::transcript::new_thread_id();
    let child_snapshot = ThreadSnapshot::new_thread_with_options(
        new_thread_id.clone(),
        parent_snapshot.workspace_root.clone(),
        parent_snapshot.cwd,
        parent_snapshot.permission_profile,
        ThreadSource::Fork,
        None,
    );
    let child_paths = rollout_paths(&child_snapshot.workspace_root, &new_thread_id);
    let mut child_items = Vec::with_capacity(1 + fork_history.len());
    child_items.push(RolloutItem::ThreadMeta(thread_meta_from_snapshot(
        &child_snapshot,
    )));
    child_items.extend(fork_history);
    if let Err(error) =
        RolloutStore::new(child_paths.rollout_path.clone()).append_items_blocking(&child_items)
    {
        rollback_child_rollout(&child_paths.thread_dir);
        return Err(error.into());
    }

    let edge = ThreadForkEdge {
        parent_thread_id: params.thread_id.clone(),
        child_thread_id: new_thread_id.clone(),
        fork_point_turn_id: params.at_turn_id.clone(),
        created_at_ms,
    };
    if let Err(error) = ThreadForkEdgeStore::for_workspace(&child_snapshot.workspace_root)
        .upsert_edge_blocking(edge)
    {
        rollback_child_rollout(&child_paths.thread_dir);
        return Err(error.into());
    }

    Ok(ThreadForkResponse {
        new_thread_id,
        parent_thread_id: params.thread_id,
        fork_point_turn_id: params.at_turn_id,
    })
}

fn rollback_child_rollout(thread_dir: &std::path::Path) {
    if let Err(error) = std::fs::remove_dir_all(thread_dir) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                error = %error,
                thread_dir = %thread_dir.display(),
                "failed to roll back child rollout after thread fork error"
            );
        }
    }
}

fn current_unix_ms() -> Result<i64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| anyhow!("system time is before unix epoch: {err}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| anyhow!("current unix timestamp exceeds i64 milliseconds"))
}
