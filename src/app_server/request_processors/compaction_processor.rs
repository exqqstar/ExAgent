use anyhow::Result;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{ThreadCompactParams, ThreadCompactResponse};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::thread_exists_in_storage;
use crate::app_server::AppServerError;

pub(in crate::app_server) async fn thread_compact(
    services: &AppServerServices,
    params: ThreadCompactParams,
) -> Result<ThreadCompactResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    let runtime = match services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &config.workspace_root,
    )? {
        Some(loaded) => loaded.runtime,
        None => {
            if !thread_exists_in_storage(&config.workspace_root, &params.thread_id) {
                return Err(AppServerError::ThreadNotFound(params.thread_id).into());
            }
            services.runtime_loader.ensure_runtime_loaded(
                &params.thread_id,
                config,
                requested_workspace_root,
                services,
            )?
        }
    };

    runtime.compact_now().await?;
    let live_view = runtime.live_view();

    Ok(ThreadCompactResponse {
        thread_id: params.thread_id,
        latest_compaction: live_view.snapshot.latest_compaction,
    })
}
