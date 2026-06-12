use anyhow::Result;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    CheckpointRestoreParams, CheckpointRestoreResponse, CheckpointRestoreStatus,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::read_thread_state_from_storage;
use crate::app_server::AppServerError;
use crate::events::RuntimeEventKind;

pub(in crate::app_server) async fn checkpoint_restore(
    services: &AppServerServices,
    params: CheckpointRestoreParams,
) -> Result<CheckpointRestoreResponse> {
    let workspace_root =
        OverridePolicy::merge_thread_read(&services.base_config, Some(params.workspace_root))?
            .workspace_root;
    let checkpoint_id = params.checkpoint_id;

    let _restore_guard = services
        .runtime_loader
        .begin_workspace_restore(&workspace_root)?;

    if !checkpoint_id_bound_to_approval(services, &workspace_root, &checkpoint_id)? {
        return Err(AppServerError::InvalidRequest(format!(
            "checkpoint `{}` is not bound to an approval in workspace `{}`",
            checkpoint_id,
            workspace_root.display()
        ))
        .into());
    }

    crate::workspace_checkpoint::restore_checkpoint(&workspace_root, &checkpoint_id).map_err(
        |err| {
            AppServerError::InvalidRequest(format!(
                "failed to restore checkpoint `{}` in workspace `{}`: {err}",
                checkpoint_id,
                workspace_root.display()
            ))
        },
    )?;

    Ok(CheckpointRestoreResponse {
        workspace_root: workspace_root.display().to_string(),
        checkpoint_id,
        status: CheckpointRestoreStatus::Restored,
        message: "workspace restored from checkpoint".to_string(),
    })
}

fn checkpoint_id_bound_to_approval(
    services: &AppServerServices,
    workspace_root: &std::path::Path,
    checkpoint_id: &str,
) -> Result<bool> {
    for thread_id in services
        .runtime_loader
        .loaded_thread_ids_in_workspace(workspace_root)
    {
        let Some(stored) = read_thread_state_from_storage(workspace_root, &thread_id)? else {
            continue;
        };
        if stored.events.iter().any(|event| {
            matches!(
                &event.kind,
                RuntimeEventKind::ApprovalRequested {
                    checkpoint_id: Some(event_checkpoint_id),
                    ..
                } if event_checkpoint_id == checkpoint_id
            )
        }) {
            return Ok(true);
        }
    }

    Ok(false)
}
