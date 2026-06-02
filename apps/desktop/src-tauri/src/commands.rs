use exagent::app_server::desktop_facade::NewProjectRequest;
use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionResponse, ApprovalDecisionStatus, EventsReplayParams,
    EventsReplayResponse, EventsSubscribeParams, ThreadReadParams, ThreadReadResponse,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartResponse, TurnContextOverrides,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use exagent::config::ThinkingMode;
use exagent::index_db::{ProjectRecord, ThreadListFilter, ThreadRecord};
use exagent::session::ApprovalId;
use exagent::types::{EventId, ThreadId, TurnId};
use tauri::{Emitter, State, Window};

use crate::settings::{
    ProviderConnectionTestRequest, ProviderConnectionTestResponse, ProviderModelListRequest,
    ProviderModelListResponse, ProviderSettingsResponse, ProviderSettingsSaveRequest,
    RuntimeSettingsResponse, RuntimeSettingsSaveRequest,
};
use crate::state::DesktopState;

type CommandResult<T> = Result<T, String>;

#[tauri::command]
pub async fn project_add(
    state: State<'_, DesktopState>,
    name: String,
    path: String,
) -> CommandResult<ProjectRecord> {
    state
        .facade
        .read()
        .await
        .add_project(NewProjectRequest {
            name,
            path: path.into(),
        })
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_list(state: State<'_, DesktopState>) -> CommandResult<Vec<ProjectRecord>> {
    state
        .facade
        .read()
        .await
        .list_projects()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_reindex(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<Vec<ThreadRecord>> {
    state
        .facade
        .read()
        .await
        .reindex_project(&project_id)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_list(
    state: State<'_, DesktopState>,
    project_id: String,
    include_archived: bool,
    search: Option<String>,
) -> CommandResult<Vec<ThreadRecord>> {
    state
        .facade
        .read()
        .await
        .list_threads(ThreadListFilter {
            project_id,
            include_archived,
            search,
        })
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_start(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<ThreadStartResponse> {
    state
        .facade
        .read()
        .await
        .start_thread(&project_id)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_read(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<ThreadReadResponse> {
    state
        .facade
        .read()
        .await
        .read_thread(
            &project_id,
            ThreadReadParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_resume(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<ThreadResumeResponse> {
    state
        .facade
        .read()
        .await
        .resume_thread(
            &project_id,
            ThreadResumeParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
                cwd: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_rename(
    state: State<'_, DesktopState>,
    thread_id: String,
    title: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .rename_thread(&ThreadId::new(thread_id), &title)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_pin(
    state: State<'_, DesktopState>,
    thread_id: String,
    pinned: bool,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .set_thread_pinned(&ThreadId::new(thread_id), pinned)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_archive(
    state: State<'_, DesktopState>,
    thread_id: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .archive_thread(&ThreadId::new(thread_id))
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_unarchive(
    state: State<'_, DesktopState>,
    thread_id: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .unarchive_thread(&ThreadId::new(thread_id))
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn turn_start(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    prompt: String,
    model: Option<exagent::resolved::ModelRef>,
    thinking_mode: Option<ThinkingMode>,
) -> CommandResult<TurnStartResponse> {
    let model = model.and_then(|model| {
        let provider_id = model.provider_id.trim();
        let model_id = model.model_id.trim();
        if provider_id.is_empty() || model_id.is_empty() {
            None
        } else {
            Some(exagent::resolved::ModelRef::new(provider_id, model_id))
        }
    });
    let turn_context = match (model, thinking_mode) {
        (None, None) => None,
        (model, thinking_mode) => Some(TurnContextOverrides {
            cwd: None,
            model,
            thinking_mode,
        }),
    };

    state
        .facade
        .read()
        .await
        .start_turn(
            &project_id,
            TurnStartParams {
                thread_id: ThreadId::new(thread_id),
                prompt,
                workspace_root: None,
                turn_context,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn turn_interrupt(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    turn_id: Option<String>,
) -> CommandResult<TurnInterruptResponse> {
    state
        .facade
        .read()
        .await
        .interrupt_turn(
            &project_id,
            TurnInterruptParams {
                thread_id: ThreadId::new(thread_id),
                turn_id: turn_id.map(TurnId::new),
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn approval_decision(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    turn_id: Option<String>,
    approval_id: String,
    decision: ApprovalDecisionStatus,
    note: Option<String>,
) -> CommandResult<ApprovalDecisionResponse> {
    state
        .facade
        .read()
        .await
        .approval_decision(
            &project_id,
            ApprovalDecisionParams {
                thread_id: ThreadId::new(thread_id),
                turn_id: turn_id.map(TurnId::new),
                approval_id: ApprovalId::new(approval_id),
                decision,
                note,
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn events_replay(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    after_event_id: Option<String>,
    include_snapshot: bool,
) -> CommandResult<EventsReplayResponse> {
    state
        .facade
        .read()
        .await
        .events_replay(
            &project_id,
            EventsReplayParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
                after_event_id: after_event_id.map(EventId::new),
                limit: None,
                include_snapshot,
                event_kinds: vec![],
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn events_subscribe(
    window: Window,
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    after_event_id: Option<String>,
) -> CommandResult<()> {
    let mut events = state
        .facade
        .read()
        .await
        .events_subscribe(
            &project_id,
            EventsSubscribeParams {
                thread_id: ThreadId::new(thread_id.clone()),
                workspace_root: None,
                after_event_id: after_event_id.map(EventId::new),
            },
        )
        .await
        .map_err(error_string)?;

    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let _ = window.emit("exagent://runtime-event", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn provider_settings_get(
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSettingsResponse> {
    state
        .settings
        .load_provider_settings()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn provider_settings_save(
    state: State<'_, DesktopState>,
    request: ProviderSettingsSaveRequest,
) -> CommandResult<ProviderSettingsResponse> {
    let response = state
        .settings
        .save_provider_settings(request)
        .await
        .map_err(error_string)?;
    state
        .rebuild_facade_from_settings()
        .await
        .map_err(error_string)?;
    Ok(response)
}

#[tauri::command]
pub async fn runtime_settings_get(
    state: State<'_, DesktopState>,
) -> CommandResult<RuntimeSettingsResponse> {
    state
        .settings
        .load_runtime_settings()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn runtime_settings_save(
    state: State<'_, DesktopState>,
    request: RuntimeSettingsSaveRequest,
) -> CommandResult<RuntimeSettingsResponse> {
    state
        .settings
        .save_runtime_settings(request)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn provider_connection_test(
    state: State<'_, DesktopState>,
    request: ProviderConnectionTestRequest,
) -> CommandResult<ProviderConnectionTestResponse> {
    state
        .settings
        .test_provider_connection(request)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn provider_models_list(
    state: State<'_, DesktopState>,
    request: ProviderModelListRequest,
) -> CommandResult<ProviderModelListResponse> {
    state
        .settings
        .list_provider_models(request)
        .await
        .map_err(error_string)
}

fn error_string(error: anyhow::Error) -> String {
    error.to_string()
}
