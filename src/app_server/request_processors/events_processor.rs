use anyhow::Result;
use tokio::sync::broadcast;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    EventsReplayParams, EventsReplayResponse, EventsSubscribeParams, ReplaySnapshotView,
    RuntimeEventKindFilter,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::{read_thread_state_from_storage, thread_exists_in_storage};
use crate::app_server::AppServerError;
use crate::events::{redact_runtime_events_for_public_boundary, RuntimeEvent, RuntimeEventKind};
use crate::runtime::thread_session::RuntimeOverlay;
use crate::session::ThreadSnapshot;

pub(in crate::app_server) fn events_replay(
    services: &AppServerServices,
    params: EventsReplayParams,
) -> Result<EventsReplayResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config =
        OverridePolicy::merge_events_replay(&services.base_config, params.workspace_root.clone())?;
    let workspace_root = services
        .runtime_loader
        .resolve_loaded_runtime(
            &params.thread_id,
            requested_workspace_root,
            &config.workspace_root,
        )?
        .map(|loaded| loaded.workspace_root)
        .unwrap_or_else(|| config.workspace_root.clone());
    let (events, snapshot) = if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &config.workspace_root,
    )? {
        let live_view = loaded.runtime.live_view();
        (
            filter_replay_events(live_view.events, &params),
            params
                .include_snapshot
                .then(|| replay_snapshot_view(live_view.snapshot, &live_view.overlay)),
        )
    } else {
        let Some(stored) = read_thread_state_from_storage(&workspace_root, &params.thread_id)?
        else {
            return Err(AppServerError::ThreadNotFound(params.thread_id).into());
        };
        let overlay = RuntimeOverlay::from_events(&stored.events);
        (
            filter_replay_events(stored.events, &params),
            params
                .include_snapshot
                .then(|| replay_snapshot_view(stored.snapshot, &overlay)),
        )
    };

    Ok(EventsReplayResponse {
        thread_id: params.thread_id,
        events: redact_runtime_events_for_public_boundary(events),
        snapshot,
    })
}

pub(in crate::app_server) fn events_subscribe(
    services: &AppServerServices,
    params: EventsSubscribeParams,
) -> Result<broadcast::Receiver<RuntimeEvent>> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config =
        OverridePolicy::merge_events_replay(&services.base_config, params.workspace_root.clone())?;
    if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &config.workspace_root,
    )? {
        return Ok(loaded.runtime.subscribe_events());
    }
    if !thread_exists_in_storage(&config.workspace_root, &params.thread_id) {
        return Err(AppServerError::ThreadNotFound(params.thread_id).into());
    }
    let runtime = services.runtime_loader.ensure_runtime_loaded(
        &params.thread_id,
        config,
        requested_workspace_root,
        services,
    )?;
    Ok(runtime.subscribe_events())
}

fn filter_replay_events(
    events: Vec<crate::events::RuntimeEvent>,
    params: &EventsReplayParams,
) -> Vec<crate::events::RuntimeEvent> {
    let after_index = params.after_event_id.as_ref().and_then(|after_event_id| {
        events
            .iter()
            .position(|event| &event.event_id == after_event_id)
    });

    let events = events
        .into_iter()
        .enumerate()
        .filter(|(index, _event)| after_index.map_or(true, |after_index| *index > after_index))
        .map(|(_index, event)| event)
        .filter(|event| {
            params.event_kinds.is_empty()
                || params
                    .event_kinds
                    .iter()
                    .any(|filter| runtime_event_kind_matches(filter, &event.kind))
        });

    match params.limit {
        Some(limit) => events.take(limit).collect(),
        None => events.collect(),
    }
}

fn runtime_event_kind_matches(filter: &RuntimeEventKindFilter, kind: &RuntimeEventKind) -> bool {
    matches!(
        (filter, kind),
        (
            RuntimeEventKindFilter::TurnStarted,
            RuntimeEventKind::TurnStarted
        ) | (
            RuntimeEventKindFilter::TurnCompleted,
            RuntimeEventKind::TurnCompleted,
        ) | (
            RuntimeEventKindFilter::TurnInterrupted,
            RuntimeEventKind::TurnInterrupted,
        ) | (
            RuntimeEventKindFilter::AssistantTextDelta,
            RuntimeEventKind::AssistantTextDelta { .. },
        ) | (
            RuntimeEventKindFilter::AssistantTurn,
            RuntimeEventKind::AssistantTurn { .. },
        ) | (
            RuntimeEventKindFilter::ReasoningDelta,
            RuntimeEventKind::ReasoningDelta { .. },
        ) | (
            RuntimeEventKindFilter::Reasoning,
            RuntimeEventKind::Reasoning { .. },
        ) | (
            RuntimeEventKindFilter::ToolResult,
            RuntimeEventKind::ToolResult { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationStarted,
            RuntimeEventKind::ToolInvocationStarted { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationWaitingApproval,
            RuntimeEventKind::ToolInvocationWaitingApproval { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationOutputDelta,
            RuntimeEventKind::ToolInvocationOutputDelta { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationCompleted,
            RuntimeEventKind::ToolInvocationCompleted { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationFailed,
            RuntimeEventKind::ToolInvocationFailed { .. },
        ) | (
            RuntimeEventKindFilter::ToolInvocationCancelled,
            RuntimeEventKind::ToolInvocationCancelled { .. },
        ) | (
            RuntimeEventKindFilter::ExecOutput,
            RuntimeEventKind::ExecOutput { .. },
        ) | (
            RuntimeEventKindFilter::ApprovalRequested,
            RuntimeEventKind::ApprovalRequested { .. },
        ) | (
            RuntimeEventKindFilter::ApprovalDecision,
            RuntimeEventKind::ApprovalDecision { .. },
        ) | (
            RuntimeEventKindFilter::CompactionWritten,
            RuntimeEventKind::CompactionWritten { .. },
        ) | (
            RuntimeEventKindFilter::SubagentSpawned,
            RuntimeEventKind::SubagentSpawned { .. },
        ) | (
            RuntimeEventKindFilter::SubagentClosed,
            RuntimeEventKind::SubagentClosed { .. },
        ) | (
            RuntimeEventKindFilter::InterAgentMessageSent,
            RuntimeEventKind::InterAgentMessageSent { .. },
        ) | (
            RuntimeEventKindFilter::TokenCount,
            RuntimeEventKind::TokenCount { .. },
        ) | (
            RuntimeEventKindFilter::RuntimeError,
            RuntimeEventKind::RuntimeError { .. },
        )
    )
}

fn replay_snapshot_view(snapshot: ThreadSnapshot, overlay: &RuntimeOverlay) -> ReplaySnapshotView {
    ReplaySnapshotView {
        thread_id: snapshot.thread_id,
        cwd: snapshot.cwd,
        permission_profile: snapshot.permission_profile,
        latest_compaction: snapshot.latest_compaction,
        open_exec_session_count: overlay.open_exec_sessions.len(),
        conversation_message_count: snapshot.conversation.len(),
        pending_approval_count: overlay.pending_approvals.len(),
    }
}
