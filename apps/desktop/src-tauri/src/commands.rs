use exagent::app_server::desktop_facade::NewProjectRequest;
use exagent::app_server::protocol::{
    AgentTreeParams, AgentTreeResponse, ApprovalDecisionParams, ApprovalDecisionResponse,
    ApprovalDecisionStatus, EventsReplayParams, EventsReplayResponse, EventsSubscribeParams,
    ThreadGoalClearParams, ThreadGoalClearResponse, ThreadGoalGetParams, ThreadGoalGetResponse,
    ThreadGoalSetParams, ThreadGoalSetResponse, ThreadGoalStatus, ThreadReadParams,
    ThreadReadResponse, ThreadResumeParams, ThreadResumeResponse, ThreadStartResponse,
    TurnContextOverrides, TurnInterruptParams, TurnInterruptResponse, TurnStartParams,
    TurnStartResponse,
};
use exagent::config::ThinkingMode;
use exagent::events::{
    redact_runtime_event_for_public_boundary, redact_runtime_events_for_public_boundary,
};
use exagent::index_db::{ProjectRecord, ThreadListFilter, ThreadRecord};
use exagent::runtime::turn_mode::TurnMode;
use exagent::session::ApprovalId;
use exagent::types::{EventId, ThreadId, TurnId};
use reqwest::Url;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{Emitter, State, Window};

use crate::settings::{
    ChatGptDeviceCode, GitHubCopilotDeviceCode, ProviderConnectionTestRequest,
    ProviderConnectionTestResponse, ProviderModelListRequest, ProviderModelListResponse,
    ProviderSettingsResponse, ProviderSettingsSaveRequest, RuntimeSettingsResponse,
    RuntimeSettingsSaveRequest, SkillCatalogScanResponse,
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
pub async fn project_rename(
    state: State<'_, DesktopState>,
    project_id: String,
    name: String,
) -> CommandResult<ProjectRecord> {
    state
        .facade
        .read()
        .await
        .rename_project(&project_id, &name)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_pin(
    state: State<'_, DesktopState>,
    project_id: String,
    pinned: bool,
) -> CommandResult<ProjectRecord> {
    state
        .facade
        .read()
        .await
        .set_project_pinned(&project_id, pinned)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_archive(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .archive_project(&project_id)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_remove(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .remove_project(&project_id)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_archive_conversations(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<()> {
    state
        .facade
        .read()
        .await
        .archive_project_threads(&project_id)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn project_create_worktree(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<ProjectRecord> {
    state
        .facade
        .read()
        .await
        .create_project_worktree(&project_id)
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
pub async fn project_reveal_in_file_manager(path: String) -> CommandResult<()> {
    reveal_in_file_manager(Path::new(&path)).map_err(error_string)
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
pub async fn agent_tree(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<AgentTreeResponse> {
    state
        .facade
        .read()
        .await
        .agent_tree(
            &project_id,
            AgentTreeParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_goal_set(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    objective: Option<String>,
    status: Option<ThreadGoalStatus>,
    token_budget: Option<Option<i64>>,
    clear_token_budget: Option<bool>,
) -> CommandResult<ThreadGoalSetResponse> {
    let token_budget = if clear_token_budget.unwrap_or(false) {
        Some(None)
    } else {
        token_budget
    };
    state
        .facade
        .read()
        .await
        .thread_goal_set(
            &project_id,
            ThreadGoalSetParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
                objective,
                status,
                token_budget,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_goal_get(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<ThreadGoalGetResponse> {
    state
        .facade
        .read()
        .await
        .thread_goal_get(
            &project_id,
            ThreadGoalGetParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_goal_clear(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<ThreadGoalClearResponse> {
    state
        .facade
        .read()
        .await
        .thread_goal_clear(
            &project_id,
            ThreadGoalClearParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
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
    clear_thinking_mode: bool,
    turn_mode: Option<TurnMode>,
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
    let turn_context = match (model, thinking_mode, clear_thinking_mode) {
        (None, None, false) => None,
        (model, thinking_mode, clear_thinking_mode) => Some(TurnContextOverrides {
            cwd: None,
            model,
            thinking_mode,
            clear_thinking_mode,
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
                turn_mode: turn_mode.unwrap_or_default(),
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
        .map(redact_events_replay_response_for_public_boundary)
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

    let window_label = window.label().to_string();
    let handle = tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let event = redact_runtime_event_for_public_boundary(event);
                    let _ = window.emit("exagent://runtime-event", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    state
        .event_subscriptions
        .replace(window_label, project_id, thread_id, handle)
        .await;

    Ok(())
}

fn redact_events_replay_response_for_public_boundary(
    mut response: EventsReplayResponse,
) -> EventsReplayResponse {
    response.events = redact_runtime_events_for_public_boundary(response.events);
    response
}

#[tauri::command]
pub async fn events_unsubscribe(
    window: Window,
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<()> {
    state
        .event_subscriptions
        .unsubscribe(window.label(), &project_id, &thread_id)
        .await;
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
pub async fn provider_chatgpt_oauth_device_start(
    state: State<'_, DesktopState>,
) -> CommandResult<ChatGptDeviceCode> {
    state
        .settings
        .start_chatgpt_device_login()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn provider_chatgpt_oauth_device_complete(
    state: State<'_, DesktopState>,
    device: ChatGptDeviceCode,
) -> CommandResult<ProviderSettingsResponse> {
    let response = state
        .settings
        .complete_chatgpt_device_login(&device)
        .await
        .map_err(error_string)?;
    state
        .rebuild_facade_from_settings()
        .await
        .map_err(error_string)?;
    Ok(response)
}

#[tauri::command]
pub async fn provider_github_copilot_oauth_device_start(
    state: State<'_, DesktopState>,
) -> CommandResult<GitHubCopilotDeviceCode> {
    state
        .settings
        .start_github_copilot_device_login()
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn provider_github_copilot_oauth_device_complete(
    state: State<'_, DesktopState>,
    device: GitHubCopilotDeviceCode,
) -> CommandResult<ProviderSettingsResponse> {
    let response = state
        .settings
        .complete_github_copilot_device_login(&device)
        .await
        .map_err(error_string)?;
    state
        .rebuild_facade_from_settings()
        .await
        .map_err(error_string)?;
    Ok(response)
}

#[tauri::command]
pub async fn open_external_url(url: String) -> CommandResult<()> {
    open_external_url_in_system_browser(&url).map_err(error_string)
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
        .save_runtime_settings(request)
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn skill_catalog_scan(
    state: State<'_, DesktopState>,
    workspace_root: Option<String>,
) -> CommandResult<SkillCatalogScanResponse> {
    state
        .settings
        .scan_skill_catalog(workspace_root.map(PathBuf::from))
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

fn reveal_in_file_manager(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg("-R").arg(path).status()?;

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer").arg(path).status()?;

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let status = Command::new("xdg-open").arg(path).status()?;

    if !status.success() {
        anyhow::bail!("failed to reveal project path in file manager");
    }
    Ok(())
}

fn open_external_url_in_system_browser(url: &str) -> anyhow::Result<()> {
    let parsed = validate_external_http_url(url)?;

    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(parsed.as_str()).status()?;

    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", "", parsed.as_str()])
        .status()?;

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let status = Command::new("xdg-open").arg(parsed.as_str()).status()?;

    if !status.success() {
        anyhow::bail!("failed to open verification page");
    }
    Ok(())
}

fn validate_external_http_url(url: &str) -> anyhow::Result<Url> {
    let parsed = Url::parse(url).map_err(|_| anyhow::anyhow!("invalid verification URL"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => anyhow::bail!("verification URL must use http or https"),
    }
}

fn error_string(error: anyhow::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::validate_external_http_url;

    #[test]
    fn external_url_validation_allows_http_and_https() {
        assert!(validate_external_http_url("https://github.com/login/device").is_ok());
        assert!(validate_external_http_url("http://localhost:1455/auth/callback").is_ok());
    }

    #[test]
    fn external_url_validation_rejects_non_web_urls() {
        assert!(validate_external_http_url("file:///etc/passwd").is_err());
        assert!(validate_external_http_url("not a url").is_err());
    }
}
