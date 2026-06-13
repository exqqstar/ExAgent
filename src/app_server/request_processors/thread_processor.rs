use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::app_server::override_policy::{OverridePolicy, RuntimeOverrides};
use crate::app_server::protocol::{
    IgnoredOverrideField, ThreadGoalMode, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, ThreadStatus, ThreadView,
    TurnStatus,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_projection::{build_thread_view_with_selection, latest_turn_state};
use crate::app_server::thread_store::{read_thread_state_from_storage, thread_exists_in_storage};
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::runtime::subagent::AgentControl;
use crate::runtime::thread_runtime::ThreadRuntime;
use crate::runtime::thread_session::RuntimeOverlay;
use crate::session::{ThreadLineage, ThreadSnapshot, ThreadSource};
use crate::state::rollout::{
    rollout_paths, thread_meta_from_snapshot, ResponseItem, RolloutItem, RolloutStore,
};
use crate::types::ThreadId;

pub(in crate::app_server) struct StartThreadOptions {
    pub(in crate::app_server) config: AgentConfig,
    pub(in crate::app_server) initial_history: InitialHistory,
    pub(in crate::app_server) thread_source: ThreadSource,
    pub(in crate::app_server) lineage: Option<ThreadLineage>,
    pub(in crate::app_server) subagent_control: Option<Arc<AgentControl>>,
}

pub(in crate::app_server) enum InitialHistory {
    New,
    Resume { thread_id: ThreadId },
}

pub(in crate::app_server) struct NewThread {
    pub(in crate::app_server) thread_id: ThreadId,
    #[allow(dead_code)]
    pub(in crate::app_server) runtime: Arc<ThreadRuntime>,
}

pub(in crate::app_server) fn thread_start(
    services: &AppServerServices,
    params: ThreadStartParams,
) -> Result<ThreadStartResponse> {
    let config = OverridePolicy::merge_thread_start(
        &services.base_config,
        RuntimeOverrides {
            workspace_root: params.workspace_root,
            cwd: params.cwd,
            permission_profile: params.permission_profile,
        },
    )?;
    let new_thread = start_thread_with_options(
        services,
        StartThreadOptions {
            config,
            initial_history: InitialHistory::New,
            thread_source: ThreadSource::User,
            lineage: None,
            subagent_control: None,
        },
    )?;

    Ok(ThreadStartResponse {
        thread: ThreadView {
            id: new_thread.thread_id,
            status: ThreadStatus::Idle,
            active_turn: None,
            turns: vec![],
            model: None,
            thinking_mode: None,
            goal: None,
            goal_mode: ThreadGoalMode::Standard,
        },
    })
}

pub(in crate::app_server) fn thread_read(
    services: &AppServerServices,
    params: ThreadReadParams,
) -> Result<ThreadReadResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let config = OverridePolicy::merge_thread_read(&services.base_config, params.workspace_root)?;
    thread_read_resolved(
        services,
        params.thread_id,
        requested_workspace_root.is_some(),
        &config.workspace_root,
    )
}

pub(in crate::app_server) fn thread_resume(
    services: &AppServerServices,
    params: ThreadResumeParams,
) -> Result<ThreadResumeResponse> {
    let ignored_overrides = ignored_resume_overrides(&params);
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config = OverridePolicy::merge_thread_resume(&services.base_config, params.workspace_root)?;
    let workspace_root = config.workspace_root.clone();
    if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &workspace_root,
    )? {
        let thread =
            thread_read_resolved(services, params.thread_id, false, &loaded.workspace_root)?;
        return Ok(ThreadResumeResponse {
            thread: thread.thread,
            ignored_overrides,
        });
    }

    let new_thread = start_thread_with_options(
        services,
        StartThreadOptions {
            config,
            initial_history: InitialHistory::Resume {
                thread_id: params.thread_id,
            },
            thread_source: ThreadSource::User,
            lineage: None,
            subagent_control: None,
        },
    )?;
    let thread = thread_read_resolved(services, new_thread.thread_id, false, &workspace_root)?;

    Ok(ThreadResumeResponse {
        thread: thread.thread,
        ignored_overrides,
    })
}

pub(in crate::app_server) fn start_thread_with_options(
    services: &AppServerServices,
    options: StartThreadOptions,
) -> Result<NewThread> {
    match options.initial_history {
        InitialHistory::New => {
            let thread_id = crate::transcript::new_thread_id();
            let snapshot = ThreadSnapshot::new_thread_with_options(
                thread_id.clone(),
                options.config.workspace_root.clone(),
                options.config.cwd.clone(),
                options.config.permission_profile,
                options.thread_source,
                options.lineage,
            );
            let rollout_paths = rollout_paths(&snapshot.workspace_root, &thread_id);
            RolloutStore::new(rollout_paths.rollout_path).append_items_blocking(&[
                RolloutItem::ThreadMeta(thread_meta_from_snapshot(&snapshot)),
            ])?;
            let subagent_control = options.subagent_control.unwrap_or_else(|| {
                AgentControl::new_root(
                    thread_id.clone(),
                    Arc::downgrade(&services.subagent_lifecycle),
                )
            });
            let runtime = services.runtime_loader.ensure_runtime_loaded_with_control(
                &thread_id,
                options.config,
                false,
                services,
                Some(subagent_control),
            )?;

            Ok(NewThread { thread_id, runtime })
        }
        InitialHistory::Resume { thread_id } => {
            if !thread_exists_in_storage(&options.config.workspace_root, &thread_id) {
                return Err(AppServerError::ThreadNotFound(thread_id).into());
            }
            let runtime = services.runtime_loader.ensure_runtime_loaded_with_control(
                &thread_id,
                options.config,
                false,
                services,
                options.subagent_control,
            )?;

            Ok(NewThread { thread_id, runtime })
        }
    }
}

pub(in crate::app_server) fn thread_read_resolved(
    services: &AppServerServices,
    thread_id: ThreadId,
    requested_workspace_root: bool,
    workspace_root: &Path,
) -> Result<ThreadReadResponse> {
    if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &thread_id,
        requested_workspace_root,
        workspace_root,
    )? {
        let runtime = loaded.runtime;
        let live_view = runtime.live_view();
        let response_items = read_thread_state_from_storage(&loaded.workspace_root, &thread_id)?
            .map(|stored| stored.response_items)
            .unwrap_or_default();
        return Ok(thread_read_from_state_view(
            services,
            thread_id,
            live_view.snapshot,
            response_items,
            live_view.overlay,
            live_view.events,
        ));
    }

    let Some(stored) = read_thread_state_from_storage(workspace_root, &thread_id)? else {
        return Err(AppServerError::ThreadNotFound(thread_id).into());
    };
    let overlay = RuntimeOverlay::from_events(&stored.events);
    Ok(thread_read_from_state_view(
        services,
        thread_id,
        stored.snapshot,
        stored.response_items,
        overlay,
        stored.events,
    ))
}

pub(in crate::app_server) fn thread_read_from_state_view(
    services: &AppServerServices,
    thread_id: ThreadId,
    snapshot: ThreadSnapshot,
    response_items: Vec<ResponseItem>,
    overlay: RuntimeOverlay,
    events: Vec<RuntimeEvent>,
) -> ThreadReadResponse {
    let active_turn = services.runtime_loader.active_turn_state(&thread_id);
    let latest_turn = latest_turn_state(&events);
    let status = if active_turn.is_some() {
        ThreadStatus::Running
    } else if overlay.has_pending_approval() || overlay.has_pending_user_input() {
        ThreadStatus::WaitingApproval
    } else if latest_turn
        .as_ref()
        .is_some_and(|turn| turn.status == TurnStatus::Failed)
    {
        ThreadStatus::Failed
    } else {
        ThreadStatus::Idle
    };
    let model = snapshot
        .reference_turn_context
        .as_ref()
        .map(|context| context.model.clone());
    let thinking_mode = snapshot
        .reference_turn_context
        .as_ref()
        .and_then(|context| context.thinking_mode);

    ThreadReadResponse {
        thread: build_thread_view_with_selection(
            thread_id,
            status,
            active_turn,
            model,
            thinking_mode,
            events,
            &response_items,
        ),
    }
}

fn ignored_resume_overrides(params: &ThreadResumeParams) -> Vec<IgnoredOverrideField> {
    let mut ignored = Vec::new();
    if params.cwd.is_some() {
        ignored.push(IgnoredOverrideField::Cwd);
    }
    ignored
}
