use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    AgentRunResponse, ApprovalDecisionParams, ApprovalDecisionResponse, ApprovalDecisionStatus,
    TurnContextOverrides, TurnInterruptParams, TurnInterruptResponse, TurnStartParams,
    TurnStartResponse, TurnState, TurnStatus, TurnView,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::thread_store::{
    read_thread_state_from_storage, thread_exists_in_storage, StoredThreadState,
};
use crate::app_server::AppServerError;
use crate::events::RuntimeEventKind;
use crate::policy::PendingCommandApproval;
use crate::runtime::thread_runtime::{ThreadOpResult, ThreadRuntimeError, ThreadTurnContext};
use crate::runtime::thread_session::RuntimeOverlay;
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ApprovalId, ApprovalStatus, ThreadSnapshot};
use crate::types::{AssistantTurn, ThreadId, TurnId};

pub(in crate::app_server) struct TurnStartStarted {
    pub(in crate::app_server) thread_id: ThreadId,
    pub(in crate::app_server) turn_id: TurnId,
}

pub(in crate::app_server) async fn turn_start(
    services: &AppServerServices,
    params: TurnStartParams,
) -> Result<TurnStartResponse> {
    turn_start_direct(services, params).await
}

pub(in crate::app_server) async fn turn_start_and_wait(
    services: &AppServerServices,
    params: TurnStartParams,
) -> Result<AgentRunResponse> {
    let (thread_id, _workspace_root, final_turn) =
        run_turn_through_runtime(services, params).await?;
    Ok(agent_run_response(thread_id, final_turn))
}

pub(in crate::app_server) async fn turn_start_direct(
    services: &AppServerServices,
    params: TurnStartParams,
) -> Result<TurnStartResponse> {
    let TurnStartStarted {
        thread_id, turn_id, ..
    } = start_turn_in_background(services, params).await?;

    Ok(TurnStartResponse {
        thread_id,
        turn: TurnView {
            id: turn_id,
            status: TurnStatus::InProgress,
            items: vec![],
        },
    })
}

pub(in crate::app_server) async fn run_turn_through_runtime(
    services: &AppServerServices,
    params: TurnStartParams,
) -> Result<(ThreadId, PathBuf, AssistantTurn)> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config = OverridePolicy::merge_turn_start(&services.base_config, params.workspace_root)?;
    let thread_id = params.thread_id;
    let runtime = services.runtime_loader.ensure_runtime_loaded(
        &thread_id,
        config,
        requested_workspace_root,
        services,
    )?;
    let live_view = runtime.live_view();
    let runtime_workspace_root = live_view.snapshot.workspace_root.clone();
    let turn_context = resolve_turn_context(
        services,
        &live_view.snapshot,
        params.turn_context,
        params.turn_mode,
    )
    .await?;
    let prompt = params.prompt;
    let result = runtime
        .submit_user_input_and_wait(prompt, turn_context)
        .await
        .map_err(map_thread_runtime_error)?;
    let ThreadOpResult::UserInput { final_turn, .. } = result else {
        return Err(AppServerError::InvalidRequest(
            "turn_start returned non-user-input runtime result".into(),
        )
        .into());
    };

    Ok((thread_id, runtime_workspace_root, final_turn))
}

pub(in crate::app_server) async fn start_turn_in_background(
    services: &AppServerServices,
    params: TurnStartParams,
) -> Result<TurnStartStarted> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config = OverridePolicy::merge_turn_start(&services.base_config, params.workspace_root)?;
    let thread_id = params.thread_id;
    let runtime = services.runtime_loader.ensure_runtime_loaded(
        &thread_id,
        config,
        requested_workspace_root,
        services,
    )?;
    let live_view = runtime.live_view();
    let turn_context = resolve_turn_context(
        services,
        &live_view.snapshot,
        params.turn_context,
        params.turn_mode,
    )
    .await?;
    let turn_id = runtime
        .submit_user_input(params.prompt, turn_context)
        .await
        .map_err(map_thread_runtime_error)?;

    Ok(TurnStartStarted { thread_id, turn_id })
}

pub(in crate::app_server) async fn turn_interrupt(
    services: &AppServerServices,
    params: TurnInterruptParams,
) -> Result<TurnInterruptResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config =
        OverridePolicy::merge_turn_start(&services.base_config, params.workspace_root.clone())?;
    if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &config.workspace_root,
    )? {
        let runtime = loaded.runtime;
        if runtime.active_turn_id().is_some() {
            let turn_id = runtime
                .interrupt_active_turn(params.turn_id.as_ref())
                .await
                .map_err(map_thread_runtime_error)?;
            return Ok(TurnInterruptResponse {
                thread_id: params.thread_id,
                interrupted_turn: Some(TurnState {
                    turn_id,
                    status: TurnStatus::Interrupted,
                }),
            });
        }
        let turn_id = runtime
            .interrupt_waiting_approval_turn(params.turn_id.clone())
            .await
            .map_err(map_thread_runtime_error)?;
        return Ok(TurnInterruptResponse {
            thread_id: params.thread_id,
            interrupted_turn: Some(TurnState {
                turn_id,
                status: TurnStatus::Interrupted,
            }),
        });
    }

    if thread_exists_in_storage(&config.workspace_root, &params.thread_id) {
        return Err(AppServerError::TurnRejected {
            thread_id: params.thread_id,
            reason: "thread has no active turn".to_string(),
        }
        .into());
    }

    Err(AppServerError::ThreadNotFound(params.thread_id).into())
}

pub(in crate::app_server) async fn approval_decision(
    services: &AppServerServices,
    params: ApprovalDecisionParams,
) -> Result<ApprovalDecisionResponse> {
    let requested_workspace_root = params.workspace_root.clone();
    let requested_workspace_root = requested_workspace_root.is_some();
    let config =
        OverridePolicy::merge_events_replay(&services.base_config, params.workspace_root.clone())?;
    if let Some(loaded) = services.runtime_loader.resolve_loaded_runtime(
        &params.thread_id,
        requested_workspace_root,
        &config.workspace_root,
    )? {
        let runtime = loaded.runtime;
        let workspace_root = loaded.workspace_root;
        restore_pending_command_approval_from_storage(
            services,
            &workspace_root,
            &params.thread_id,
            &params.approval_id,
        )
        .await?;
        let status = approval_decision_status_to_session(&params.decision);
        let result = runtime
            .approval_decision(params.turn_id, params.approval_id, status, params.note)
            .await
            .map_err(map_thread_runtime_error)?;
        return match result {
            ThreadOpResult::ApprovalDecision {
                turn_id,
                approval_id,
                status,
            } => Ok(ApprovalDecisionResponse {
                thread_id: params.thread_id,
                turn_id,
                approval_id,
                status: session_approval_status_to_decision(status)?,
            }),
            response => Err(unexpected_runtime_result("approval_decision", &response).into()),
        };
    }

    if let Some(stored) = read_thread_state_from_storage(&config.workspace_root, &params.thread_id)?
    {
        let overlay = RuntimeOverlay::from_events(&stored.events);
        if overlay.has_pending_approval_id(&params.approval_id) {
            let status = approval_decision_status_to_session(&params.decision);
            let thread_id = params.thread_id.clone();
            if let Some(approval) =
                pending_command_approval_from_stored_state(&stored, &params.approval_id)
            {
                services.policy.restore_command_approval(approval).await;
            }
            let runtime = services.runtime_loader.ensure_runtime_loaded(
                &params.thread_id,
                config,
                requested_workspace_root,
                services,
            )?;
            let result = runtime
                .approval_decision(params.turn_id, params.approval_id, status, params.note)
                .await
                .map_err(map_thread_runtime_error)?;
            return match result {
                ThreadOpResult::ApprovalDecision {
                    turn_id,
                    approval_id,
                    status,
                } => Ok(ApprovalDecisionResponse {
                    thread_id,
                    turn_id,
                    approval_id,
                    status: session_approval_status_to_decision(status)?,
                }),
                response => Err(unexpected_runtime_result("approval_decision", &response).into()),
            };
        }

        return Err(AppServerError::TurnRejected {
            thread_id: params.thread_id,
            reason: "thread has no pending approval".to_string(),
        }
        .into());
    }

    Err(AppServerError::ThreadNotFound(params.thread_id).into())
}

pub(in crate::app_server) async fn restore_pending_command_approval_from_storage(
    services: &AppServerServices,
    workspace_root: &Path,
    thread_id: &ThreadId,
    approval_id: &ApprovalId,
) -> Result<()> {
    let Some(stored) = read_thread_state_from_storage(workspace_root, thread_id)? else {
        return Ok(());
    };
    let overlay = RuntimeOverlay::from_events(&stored.events);
    if !overlay.has_pending_approval_id(approval_id) {
        return Ok(());
    }
    if let Some(approval) = pending_command_approval_from_stored_state(&stored, approval_id) {
        services.policy.restore_command_approval(approval).await;
    }
    Ok(())
}

pub(in crate::app_server) async fn resolve_turn_context(
    services: &AppServerServices,
    snapshot: &ThreadSnapshot,
    overrides: Option<TurnContextOverrides>,
    turn_mode: TurnMode,
) -> Result<Option<ThreadTurnContext>> {
    if overrides.is_none() && turn_mode.is_default() {
        return Ok(None);
    }

    let (cwd, model_ref, thinking_mode, clear_thinking_mode) = match overrides {
        Some(overrides) => {
            let model_ref = overrides.model.clone();
            let thinking_mode = overrides.thinking_mode;
            let clear_thinking_mode = overrides.clear_thinking_mode;
            let resolved_snapshot = OverridePolicy::apply_turn_context(snapshot, overrides)?;
            (
                Some(resolved_snapshot.cwd),
                model_ref,
                thinking_mode,
                clear_thinking_mode,
            )
        }
        None => (None, None, None, false),
    };

    let resolved_model = match model_ref.as_ref() {
        Some(model_ref) => Some(services.model_resolver.resolve(model_ref).await?),
        None => None,
    };
    Ok(Some(ThreadTurnContext {
        cwd,
        resolved_model,
        thinking_mode,
        clear_thinking_mode,
        turn_mode,
    }))
}

pub(in crate::app_server) fn agent_run_response(
    thread_id: ThreadId,
    final_turn: AssistantTurn,
) -> AgentRunResponse {
    AgentRunResponse {
        text: final_turn.text,
        tool_calls: final_turn.tool_calls,
        thread_id,
    }
}

fn pending_command_approval_from_stored_state(
    stored: &StoredThreadState,
    approval_id: &ApprovalId,
) -> Option<PendingCommandApproval> {
    let (tool_name, reason, command) = stored.events.iter().rev().find_map(|event| match &event
        .kind
    {
        RuntimeEventKind::ApprovalRequested {
            approval_id: event_approval_id,
            tool_name,
            reason,
            command: Some(command),
            ..
        } if event_approval_id == approval_id => {
            Some((tool_name.clone(), reason.clone(), command.clone()))
        }
        _ => None,
    })?;
    if !matches!(tool_name.as_str(), "run_command" | "exec_command") {
        return None;
    }

    let cwd = PathBuf::from(command.cwd);

    Some(PendingCommandApproval {
        approval_id: approval_id.clone(),
        thread_id: stored.snapshot.thread_id.clone(),
        tool_name,
        command: command.command,
        cwd,
        timeout_secs: command.timeout_secs,
        persistent: command.persistent,
        reason,
    })
}

fn unexpected_runtime_result(operation: &str, result: &ThreadOpResult) -> AppServerError {
    AppServerError::InvalidRequest(format!(
        "{operation} returned unexpected {} runtime result",
        runtime_result_name(result)
    ))
}

fn runtime_result_name(result: &ThreadOpResult) -> &'static str {
    match result {
        ThreadOpResult::UserInput { .. } => "user_input",
        ThreadOpResult::Interrupted { .. } => "interrupted",
        ThreadOpResult::ApprovalDecision { .. } => "approval_decision",
        ThreadOpResult::Ack => "ack",
    }
}

fn approval_decision_status_to_session(status: &ApprovalDecisionStatus) -> ApprovalStatus {
    match status {
        ApprovalDecisionStatus::Approved => ApprovalStatus::Approved,
        ApprovalDecisionStatus::Denied => ApprovalStatus::Denied,
    }
}

fn session_approval_status_to_decision(status: ApprovalStatus) -> Result<ApprovalDecisionStatus> {
    match status {
        ApprovalStatus::Approved => Ok(ApprovalDecisionStatus::Approved),
        ApprovalStatus::Denied => Ok(ApprovalDecisionStatus::Denied),
        ApprovalStatus::Pending => Err(AppServerError::InvalidRequest(
            "approval decision returned pending status".to_string(),
        )
        .into()),
    }
}

fn map_thread_runtime_error(err: anyhow::Error) -> anyhow::Error {
    let Some(runtime_error) = err.downcast_ref::<ThreadRuntimeError>() else {
        return err;
    };

    match runtime_error {
        ThreadRuntimeError::ThreadBusy(thread_id) => {
            AppServerError::ThreadBusy(thread_id.clone()).into()
        }
        ThreadRuntimeError::TurnRejected { thread_id, reason } => AppServerError::TurnRejected {
            thread_id: thread_id.clone(),
            reason: reason.clone(),
        }
        .into(),
        ThreadRuntimeError::TurnInterrupted { thread_id, turn_id } => {
            AppServerError::TurnInterrupted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            }
            .into()
        }
    }
}
