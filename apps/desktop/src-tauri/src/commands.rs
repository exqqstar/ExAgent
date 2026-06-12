use base64::{engine::general_purpose, Engine as _};
use exagent::app_server::desktop_facade::NewProjectRequest;
use exagent::app_server::protocol::{
    AgentTreeParams, AgentTreeResponse, ApprovalDecisionParams, ApprovalDecisionResponse,
    ApprovalDecisionStatus, ApprovalsListParams, ApprovalsListResponse, CheckpointRestoreParams,
    CheckpointRestoreResponse, EventsReplayParams, EventsReplayResponse, EventsSubscribeParams,
    ThreadCompactParams, ThreadCompactResponse, ThreadForkParams, ThreadForkResponse,
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
use exagent::model::image_input::{validate_image_bytes_for_prompt, MAX_IMAGE_SOURCE_BYTES};
use exagent::runtime::turn_mode::TurnMode;
use exagent::session::ApprovalId;
use exagent::types::{EventId, ThreadId, TurnId};
use reqwest::Url;
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::settings::{
    ChatGptDeviceCode, GitHubCopilotDeviceCode, ProviderConnectionTestRequest,
    ProviderConnectionTestResponse, ProviderModelListRequest, ProviderModelListResponse,
    ProviderSettingsResponse, ProviderSettingsSaveRequest, RuntimeSettingsResponse,
    RuntimeSettingsSaveRequest, SkillCatalogScanResponse,
};
use crate::state::DesktopState;

type CommandResult<T> = Result<T, String>;

const MAX_IMAGE_ATTACHMENT_BYTES: usize = MAX_IMAGE_SOURCE_BYTES;

#[tauri::command]
pub async fn image_attachments_import(
    app: AppHandle,
    paths: Vec<String>,
) -> CommandResult<Vec<String>> {
    let cache_root = attachment_cache_root(&app)?;
    let mut imported = Vec::with_capacity(paths.len());

    for raw_path in paths {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let source = PathBuf::from(trimmed);
        let file_name = attachment_file_name(
            source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image"),
            None,
        );
        let source_label = source.display().to_string();
        let metadata = tokio::fs::metadata(&source)
            .await
            .map_err(|err| format!("Could not read selected image `{source_label}`: {err}"))?;
        validate_attachment_file_size(metadata.len(), &source_label)?;
        let bytes = tokio::fs::read(&source)
            .await
            .map_err(|err| format!("Could not read selected image `{source_label}`: {err}"))?;
        let cached_path =
            cache_image_attachment_bytes(&cache_root, file_name, bytes, &source_label).await?;
        imported.push(cached_path);
    }

    Ok(imported)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentBytesImport {
    file_name: String,
    mime_type: Option<String>,
    bytes_base64: String,
}

#[tauri::command]
pub async fn image_attachments_import_bytes(
    app: AppHandle,
    items: Vec<ImageAttachmentBytesImport>,
) -> CommandResult<Vec<String>> {
    let cache_root = attachment_cache_root(&app)?;
    let mut imported = Vec::with_capacity(items.len());

    for item in items {
        let mime_type = item.mime_type.as_deref().or(Some("image/png"));
        let file_name = attachment_file_name(&item.file_name, mime_type);
        let source_label = if item.file_name.trim().is_empty() {
            file_name.clone()
        } else {
            item.file_name.clone()
        };
        let encoded = item.bytes_base64.trim();
        validate_attachment_base64_size(encoded, &source_label)?;
        let bytes = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|err| format!("Could not decode pasted image `{source_label}`: {err}"))?;
        let cached_path =
            cache_image_attachment_bytes(&cache_root, file_name, bytes, &source_label).await?;
        imported.push(cached_path);
    }

    Ok(imported)
}

fn attachment_cache_root(app: &AppHandle) -> CommandResult<PathBuf> {
    Ok(app
        .path()
        .app_cache_dir()
        .map_err(|err| err.to_string())?
        .join("attachments"))
}

async fn cache_image_attachment_bytes(
    cache_root: &Path,
    file_name: String,
    bytes: Vec<u8>,
    source_label: &str,
) -> CommandResult<String> {
    validate_attachment_size(bytes.len(), source_label)?;
    validate_supported_image_attachment(&bytes, source_label)?;

    let hash = attachment_content_hash(&bytes);
    let target_dir = cache_root.join(format!("{hash:016x}"));
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|err| {
            format!(
                "Could not prepare image attachment cache `{}`: {err}",
                target_dir.display()
            )
        })?;

    let target = target_dir.join(file_name);
    tokio::fs::write(&target, bytes)
        .await
        .map_err(|err| format!("Could not cache selected image `{source_label}`: {err}"))?;
    Ok(target.to_string_lossy().into_owned())
}

fn validate_attachment_size(len: usize, source_label: &str) -> CommandResult<()> {
    validate_attachment_size_bytes(len as u64, source_label)
}

fn validate_attachment_file_size(len: u64, source_label: &str) -> CommandResult<()> {
    validate_attachment_size_bytes(len, source_label)
}

fn validate_attachment_size_bytes(len: u64, source_label: &str) -> CommandResult<()> {
    if len == 0 {
        return Err(format!(
            "Could not cache image `{source_label}`: file is empty"
        ));
    }
    if len > MAX_IMAGE_ATTACHMENT_BYTES as u64 {
        return Err(format!(
            "Could not cache image `{source_label}`: {len} bytes exceeds the {MAX_IMAGE_ATTACHMENT_BYTES} byte limit"
        ));
    }
    Ok(())
}

fn validate_attachment_base64_size(encoded: &str, source_label: &str) -> CommandResult<()> {
    let decoded_len_upper_bound =
        base64_decoded_len_upper_bound(encoded.len(), attachment_base64_padding_len(encoded));
    validate_attachment_base64_decoded_upper_bound(decoded_len_upper_bound, source_label)
}

fn validate_attachment_base64_decoded_upper_bound(
    decoded_len_upper_bound: usize,
    source_label: &str,
) -> CommandResult<()> {
    if decoded_len_upper_bound > MAX_IMAGE_ATTACHMENT_BYTES {
        return Err(format!(
            "Could not decode pasted image `{source_label}`: encoded payload exceeds the {MAX_IMAGE_ATTACHMENT_BYTES} byte decoded limit"
        ));
    }
    Ok(())
}

fn attachment_base64_padding_len(encoded: &str) -> usize {
    encoded
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .take(2)
        .count()
}

fn base64_decoded_len_upper_bound(encoded_len: usize, padding_len: usize) -> usize {
    let remainder = match encoded_len % 4 {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => 3,
    };
    (encoded_len / 4)
        .saturating_mul(3)
        .saturating_add(remainder)
        .saturating_sub(padding_len.min(2))
}

fn validate_supported_image_attachment(bytes: &[u8], source_label: &str) -> CommandResult<()> {
    validate_image_bytes_for_prompt(Path::new(source_label), bytes).map_err(|err| {
        format!(
            "Could not cache image `{source_label}`: unsupported or invalid image format; expected PNG, JPEG, WebP, or GIF ({err})"
        )
    })
}

fn attachment_content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn attachment_file_name(raw_file_name: &str, mime_type: Option<&str>) -> String {
    let safe = safe_attachment_file_name(raw_file_name);

    if safe.is_empty() {
        return format!("image.{}", attachment_extension_for_mime(mime_type));
    }

    let file_name = if Path::new(&safe).extension().is_some() || mime_type.is_none() {
        safe
    } else {
        format!("{safe}.{}", attachment_extension_for_mime(mime_type))
    };

    avoid_reserved_attachment_file_name(file_name)
}

fn attachment_extension_for_mime(mime_type: Option<&str>) -> &'static str {
    let normalized = mime_type.unwrap_or_default().trim().to_ascii_lowercase();
    let subtype = normalized.strip_prefix("image/").unwrap_or(&normalized);
    match subtype {
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        _ => "png",
    }
}

fn safe_attachment_file_name(file_name: &str) -> String {
    file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(['.', ' ', '_'])
        .to_string()
}

fn avoid_reserved_attachment_file_name(file_name: String) -> String {
    if is_reserved_windows_attachment_name(&file_name) {
        format!("image_{file_name}")
    } else {
        file_name
    }
}

fn is_reserved_windows_attachment_name(file_name: &str) -> bool {
    let stem = file_name
        .split_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

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
pub async fn thread_fork(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
    at_turn_id: String,
) -> CommandResult<ThreadForkResponse> {
    state
        .facade
        .read()
        .await
        .fork_thread(
            &project_id,
            ThreadForkParams {
                thread_id: ThreadId::new(thread_id),
                at_turn_id: TurnId::new(at_turn_id),
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn thread_compact(
    state: State<'_, DesktopState>,
    project_id: String,
    thread_id: String,
) -> CommandResult<ThreadCompactResponse> {
    state
        .facade
        .read()
        .await
        .compact_thread(
            &project_id,
            ThreadCompactParams {
                thread_id: ThreadId::new(thread_id),
                workspace_root: None,
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
    input: Option<Vec<exagent::types::UserInput>>,
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
                input: input.unwrap_or_default(),
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
pub async fn approvals_list(
    state: State<'_, DesktopState>,
    project_id: String,
) -> CommandResult<ApprovalsListResponse> {
    state
        .facade
        .read()
        .await
        .approvals_list(
            &project_id,
            ApprovalsListParams {
                workspace_root: None,
            },
        )
        .await
        .map_err(error_string)
}

#[tauri::command]
pub async fn checkpoint_restore(
    state: State<'_, DesktopState>,
    project_id: String,
    checkpoint_id: String,
) -> CommandResult<CheckpointRestoreResponse> {
    state
        .facade
        .read()
        .await
        .checkpoint_restore(
            &project_id,
            CheckpointRestoreParams {
                workspace_root: String::new(),
                checkpoint_id,
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
    use super::*;

    fn valid_png_attachment_bytes() -> Vec<u8> {
        general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGPgEpH7DwABpAE8k4sOtwAAAABJRU5ErkJggg==")
            .unwrap()
    }

    #[test]
    fn appends_extension_from_mime_for_byte_imports() {
        assert_eq!(
            attachment_file_name("clipboard", Some("image/jpeg")),
            "clipboard.jpg"
        );
        assert_eq!(
            attachment_file_name("clipboard", Some("image/png")),
            "clipboard.png"
        );
        assert_eq!(attachment_file_name("", Some("image/webp")), "image.webp");
        assert_eq!(
            attachment_file_name("shot.png", Some("image/png")),
            "shot.png"
        );
    }

    #[test]
    fn keeps_path_import_names_unchanged() {
        assert_eq!(attachment_file_name("photo", None), "photo");
        assert_eq!(attachment_file_name("photo.jpeg", None), "photo.jpeg");
    }

    #[test]
    fn sanitizes_attachment_file_names_strictly() {
        let sanitized = attachment_file_name("..\\evil:name?", Some("image/png"));
        assert_eq!(sanitized, "evil_name.png");
        assert!(!sanitized.contains(['/', '\\', ':']));
        assert_eq!(
            attachment_file_name(" \t:/", Some("image/gif")),
            "image.gif"
        );
    }

    #[test]
    fn rejects_empty_and_oversize_attachments() {
        assert!(validate_attachment_size(0, "empty.png").is_err());
        assert!(validate_attachment_size(MAX_IMAGE_ATTACHMENT_BYTES + 1, "huge.png").is_err());
        assert!(validate_attachment_size(1024, "ok.png").is_ok());
    }

    #[test]
    fn rejects_oversized_base64_before_decode() {
        assert_eq!(base64_decoded_len_upper_bound(4, 0), 3);
        assert_eq!(base64_decoded_len_upper_bound(4, 1), 2);
        assert_eq!(base64_decoded_len_upper_bound(4, 2), 1);
        assert!(validate_attachment_base64_decoded_upper_bound(
            MAX_IMAGE_ATTACHMENT_BYTES + 1,
            "huge.png"
        )
        .is_err());
        assert!(validate_attachment_base64_decoded_upper_bound(
            MAX_IMAGE_ATTACHMENT_BYTES,
            "ok.png"
        )
        .is_ok());
    }

    #[test]
    fn validates_supported_image_attachment() {
        assert!(
            validate_supported_image_attachment(&valid_png_attachment_bytes(), "image.png").is_ok()
        );
    }

    #[test]
    fn rejects_corrupt_image_with_valid_signature() {
        let error = validate_supported_image_attachment(b"\x89PNG\r\n\x1a\nnot-png", "image.png")
            .unwrap_err();

        assert!(
            error.contains("unsupported or invalid image format"),
            "{error}"
        );
    }

    #[test]
    fn rejects_unsupported_image_payloads() {
        assert!(validate_supported_image_attachment(b"not really an image", "image.png").is_err());
        assert!(validate_supported_image_attachment(b"II*\0rest", "image.tiff").is_err());
    }

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
