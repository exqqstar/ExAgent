use crate::app_server::protocol::{
    ThreadItem, ThreadStatus, ThreadView, TurnState, TurnStatus, TurnView,
};
use crate::config::ThinkingMode;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::resolved::ModelRef;
use crate::session::{ApprovalId, ApprovalStatus};
use crate::state::fork_history::FORK_CONTEXT_TURN_ID;
use crate::state::rollout::ResponseItem;
use crate::types::{
    ConversationContentPart, ConversationMessage, MessageRole, ThreadId, TurnId, UserInput,
};

const SYSTEM_GOAL_REPORT_TURN_ID: &str = "system_goal_reports";

pub(in crate::app_server) fn latest_turn_state(events: &[RuntimeEvent]) -> Option<TurnState> {
    events.iter().rev().find_map(|event| {
        let turn_id = event.turn_id.clone()?;
        let status = match &event.kind {
            RuntimeEventKind::TurnStarted => Some(TurnStatus::InProgress),
            RuntimeEventKind::TurnCompleted => Some(TurnStatus::Completed),
            RuntimeEventKind::TurnInterrupted => Some(TurnStatus::Interrupted),
            RuntimeEventKind::RuntimeError { .. } => Some(TurnStatus::Failed),
            RuntimeEventKind::ApprovalRequested { .. } => Some(TurnStatus::InProgress),
            RuntimeEventKind::AssistantTurn { .. } => Some(TurnStatus::Completed),
            RuntimeEventKind::AssistantTextDelta { .. }
            | RuntimeEventKind::Reasoning { .. }
            | RuntimeEventKind::ReasoningDelta { .. } => Some(TurnStatus::InProgress),
            RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ToolInvocationStarted { .. }
            | RuntimeEventKind::ToolInvocationWaitingApproval { .. }
            | RuntimeEventKind::ToolInvocationWaitingUserInput { .. }
            | RuntimeEventKind::ToolInvocationOutputDelta { .. }
            | RuntimeEventKind::ToolInvocationCompleted { .. }
            | RuntimeEventKind::ToolInvocationFailed { .. }
            | RuntimeEventKind::ToolInvocationCancelled { .. }
            | RuntimeEventKind::ExecOutput { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::UserInputRequested { .. }
            | RuntimeEventKind::UserInputResolved { .. }
            | RuntimeEventKind::CompactionWritten { .. }
            | RuntimeEventKind::SubagentSpawned { .. }
            | RuntimeEventKind::SubagentClosed { .. }
            | RuntimeEventKind::InterAgentMessageSent { .. }
            | RuntimeEventKind::ReviewSubmitted { .. }
            | RuntimeEventKind::TokenCount { .. } => Some(TurnStatus::InProgress),
            RuntimeEventKind::ThreadGoalUpdated { .. }
            | RuntimeEventKind::ThreadGoalCleared { .. }
            | RuntimeEventKind::ThreadGoalContinuationStarted { .. }
            | RuntimeEventKind::ThreadGoalContinuationSuppressed { .. }
            | RuntimeEventKind::ThreadGoalTurnStarted { .. }
            | RuntimeEventKind::ThreadGoalToolCompleted { .. }
            | RuntimeEventKind::ThreadGoalReport { .. } => None,
        }?;
        Some(TurnState { turn_id, status })
    })
}

#[cfg(test)]
pub(in crate::app_server) fn build_thread_view(
    thread_id: ThreadId,
    status: ThreadStatus,
    active_turn: Option<TurnState>,
    events: Vec<RuntimeEvent>,
    response_items: &[ResponseItem],
) -> ThreadView {
    build_thread_view_with_selection(
        thread_id,
        status,
        active_turn,
        None,
        None,
        events,
        response_items,
    )
}

pub(in crate::app_server) fn build_thread_view_with_selection(
    thread_id: ThreadId,
    status: ThreadStatus,
    active_turn: Option<TurnState>,
    model: Option<ModelRef>,
    thinking_mode: Option<ThinkingMode>,
    events: Vec<RuntimeEvent>,
    response_items: &[ResponseItem],
) -> ThreadView {
    let mut turns = build_turn_views(events);
    insert_response_items(&mut turns, response_items);
    let active_turn_view = active_turn.map(|state| {
        let index = ensure_turn_view(&mut turns, &state.turn_id);
        turns[index].status = state.status;
        turns[index].clone()
    });

    ThreadView {
        id: thread_id,
        status,
        active_turn: active_turn_view,
        turns,
        model,
        thinking_mode,
        goal: None,
    }
}

fn build_turn_views(events: Vec<RuntimeEvent>) -> Vec<TurnView> {
    let mut turns = Vec::new();
    let mut current_turn_id: Option<TurnId> = None;

    for event in events {
        match &event.kind {
            RuntimeEventKind::TurnStarted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::InProgress;
                current_turn_id = Some(turn_id);
            }
            RuntimeEventKind::TurnCompleted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::Completed;
                if current_turn_id.as_ref() == Some(&turn_id) {
                    current_turn_id = None;
                }
            }
            RuntimeEventKind::TurnInterrupted => {
                let Some(turn_id) = event.turn_id.clone() else {
                    continue;
                };
                let index = ensure_turn_view(&mut turns, &turn_id);
                turns[index].status = TurnStatus::Interrupted;
                if current_turn_id.as_ref() == Some(&turn_id) {
                    current_turn_id = None;
                }
            }
            RuntimeEventKind::RuntimeError { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    turns[index].status = TurnStatus::Failed;
                    if let Some(item) = thread_item_from_event(&event) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::ApprovalRequested { .. }
            | RuntimeEventKind::UserInputRequested { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    if let Some(item) = thread_item_from_event(&event) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::AssistantTurn { .. }
            | RuntimeEventKind::AssistantTextDelta { .. }
            | RuntimeEventKind::Reasoning { .. }
            | RuntimeEventKind::ReasoningDelta { .. }
            | RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ExecOutput { .. }
            | RuntimeEventKind::CompactionWritten { .. }
            | RuntimeEventKind::SubagentSpawned { .. }
            | RuntimeEventKind::SubagentClosed { .. }
            | RuntimeEventKind::InterAgentMessageSent { .. }
            | RuntimeEventKind::ReviewSubmitted { .. }
            | RuntimeEventKind::TokenCount { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    if let Some(item) = thread_item_from_event(&event) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::ApprovalDecision {
                approval_id,
                status,
                ..
            } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    apply_approval_decision_to_tool_invocation(
                        &mut turns[index].items,
                        approval_id,
                        status,
                    );
                    if let Some(item) = thread_item_from_event(&event) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::UserInputResolved {
                request_id,
                dismissed: _,
            } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    apply_user_input_resolved_to_tool_invocation(
                        &mut turns[index].items,
                        request_id,
                    );
                    if let Some(item) = thread_item_from_event(&event) {
                        turns[index].items.push(item);
                    }
                }
            }
            RuntimeEventKind::ToolInvocationStarted { .. }
            | RuntimeEventKind::ToolInvocationWaitingApproval { .. }
            | RuntimeEventKind::ToolInvocationWaitingUserInput { .. }
            | RuntimeEventKind::ToolInvocationOutputDelta { .. }
            | RuntimeEventKind::ToolInvocationCompleted { .. }
            | RuntimeEventKind::ToolInvocationFailed { .. }
            | RuntimeEventKind::ToolInvocationCancelled { .. } => {
                if let Some(turn_id) = view_turn_id(&event, current_turn_id.as_ref()) {
                    let index = ensure_turn_view(&mut turns, &turn_id);
                    apply_tool_invocation_event(&mut turns[index].items, &event.kind);
                }
            }
            RuntimeEventKind::ThreadGoalUpdated { .. }
            | RuntimeEventKind::ThreadGoalCleared { .. }
            | RuntimeEventKind::ThreadGoalContinuationStarted { .. }
            | RuntimeEventKind::ThreadGoalContinuationSuppressed { .. }
            | RuntimeEventKind::ThreadGoalTurnStarted { .. }
            | RuntimeEventKind::ThreadGoalToolCompleted { .. } => {}
            RuntimeEventKind::ThreadGoalReport { .. } => {
                let turn_id = goal_report_turn_id(&event, current_turn_id.as_ref());
                let synthetic = event.turn_id.is_none() && current_turn_id.is_none();
                let index = ensure_turn_view(&mut turns, &turn_id);
                if synthetic {
                    turns[index].status = TurnStatus::Completed;
                }
                if let Some(item) = thread_item_from_event(&event) {
                    turns[index].items.push(item);
                }
            }
        }
    }

    turns
}

#[derive(Default)]
struct ProjectedTurnMessages {
    user: Option<ConversationMessage>,
    assistants: Vec<ConversationMessage>,
}

/// Projects rollout response items onto turn views by their `turn_id`.
///
/// Response items are always tagged with the turn that produced them (see
/// `RolloutItem::response_item_for_turn`).
fn insert_response_items(turns: &mut Vec<TurnView>, response_items: &[ResponseItem]) {
    let response_items = response_items
        .iter()
        .filter(|item| item.turn_id.as_str() != FORK_CONTEXT_TURN_ID)
        .collect::<Vec<_>>();
    for turn_id in response_items.iter().map(|item| &item.turn_id) {
        let existing = turns.iter().any(|turn| &turn.id == turn_id);
        let index = ensure_turn_view(turns, turn_id);
        if !existing {
            turns[index].status = TurnStatus::Completed;
        }
    }

    for turn in turns.iter_mut() {
        let projected = response_items
            .iter()
            .filter(|item| item.turn_id == turn.id)
            .map(|item| &item.message)
            .fold(
                ProjectedTurnMessages::default(),
                |mut projected, message| {
                    if !is_projectable_message(message) {
                        return projected;
                    }
                    match message.role {
                        MessageRole::User => projected.user = Some(message.clone()),
                        MessageRole::Assistant => projected.assistants.push(message.clone()),
                        MessageRole::System | MessageRole::Tool => {}
                    }
                    projected
                },
            );
        insert_projected_turn_messages(turn, projected);
    }
}

fn insert_projected_turn_messages(turn: &mut TurnView, projected: ProjectedTurnMessages) {
    if let Some(user) = projected.user {
        let text = user.content.clone();
        let input = projected_user_input(&user);
        if !turn
            .items
            .iter()
            .any(|item| matches!(item, ThreadItem::UserMessage { text: existing, .. } if existing == &text))
        {
            turn.items.insert(0, ThreadItem::UserMessage { text, input });
        }
    }

    for assistant in projected.assistants {
        let reasoning_content = displayable_reasoning_content(&assistant);
        if !reasoning_content.is_empty() && !reasoning_item_exists(turn, &reasoning_content) {
            let insert_at = if assistant.tool_calls.is_empty() {
                turn.items.len()
            } else {
                first_tool_item_index(turn).unwrap_or(turn.items.len())
            };
            turn.items.insert(
                insert_at,
                ThreadItem::Reasoning {
                    event_id: None,
                    summary: vec![],
                    content: reasoning_content,
                },
            );
        }

        if assistant_message_exists(turn, &assistant.content) {
            continue;
        }
        let item = ThreadItem::AssistantMessage {
            event_id: None,
            text: Some(assistant.content.clone()),
        };
        let insert_at = if assistant.tool_calls.is_empty() {
            turn.items.len()
        } else {
            first_tool_item_index(turn).unwrap_or(turn.items.len())
        };
        turn.items.insert(insert_at, item);
    }
}

fn is_projectable_message(message: &ConversationMessage) -> bool {
    if message.injected {
        return false;
    }

    if !message.content.trim().is_empty() {
        return true;
    }

    if matches!(message.role, MessageRole::User)
        && message.parts.iter().any(ConversationContentPart::is_image)
    {
        return true;
    }

    matches!(message.role, MessageRole::Assistant)
        && (!message.reasoning.is_empty() || !message.tool_calls.is_empty())
}

fn projected_user_input(message: &ConversationMessage) -> Vec<UserInput> {
    let parts = message.effective_parts();
    if !parts.iter().any(ConversationContentPart::is_image) {
        return vec![];
    }

    parts
        .into_iter()
        .map(|part| match part {
            ConversationContentPart::Text { text } => UserInput::Text { text },
            ConversationContentPart::LocalImage { path, detail } => {
                UserInput::LocalImage { path, detail }
            }
            ConversationContentPart::ImageUrl { url, detail } => {
                UserInput::ImageUrl { url, detail }
            }
        })
        .collect()
}

fn displayable_reasoning_content(message: &ConversationMessage) -> Vec<String> {
    message
        .reasoning
        .iter()
        .filter(|block| !block.redacted)
        .map(|block| block.text.trim())
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn reasoning_item_exists(turn: &TurnView, content: &[String]) -> bool {
    turn.items.iter().any(|item| {
        matches!(item, ThreadItem::Reasoning { content: existing, .. } if existing == content)
    })
}

fn assistant_message_exists(turn: &TurnView, text: &str) -> bool {
    turn.items.iter().any(|item| {
        matches!(item, ThreadItem::AssistantMessage { text: Some(existing), .. } if existing == text)
    })
}

fn first_tool_item_index(turn: &TurnView) -> Option<usize> {
    turn.items.iter().position(|item| {
        matches!(
            item,
            ThreadItem::ToolInvocation { .. }
                | ThreadItem::ToolResult { .. }
                | ThreadItem::ExecOutput { .. }
        )
    })
}

fn ensure_turn_view(turns: &mut Vec<TurnView>, turn_id: &TurnId) -> usize {
    if let Some(index) = turns.iter().position(|turn| &turn.id == turn_id) {
        return index;
    }

    turns.push(TurnView {
        id: turn_id.clone(),
        status: TurnStatus::InProgress,
        items: vec![],
    });
    turns.len() - 1
}

fn view_turn_id(event: &RuntimeEvent, current_turn_id: Option<&TurnId>) -> Option<TurnId> {
    current_turn_id.cloned().or_else(|| event.turn_id.clone())
}

fn goal_report_turn_id(event: &RuntimeEvent, current_turn_id: Option<&TurnId>) -> TurnId {
    view_turn_id(event, current_turn_id).unwrap_or_else(|| TurnId::new(SYSTEM_GOAL_REPORT_TURN_ID))
}

fn thread_item_from_event(event: &RuntimeEvent) -> Option<ThreadItem> {
    match &event.kind {
        RuntimeEventKind::AssistantTurn { turn } => {
            turn.text.as_ref().map(|text| ThreadItem::AssistantMessage {
                event_id: Some(event.event_id.clone()),
                text: Some(text.clone()),
            })
        }
        RuntimeEventKind::Reasoning { summary, content } => Some(ThreadItem::Reasoning {
            event_id: Some(event.event_id.clone()),
            summary: summary.clone(),
            content: content.clone(),
        }),
        RuntimeEventKind::ToolResult { result } => Some(ThreadItem::ToolResult {
            event_id: Some(event.event_id.clone()),
            name: result.tool_name.clone(),
        }),
        RuntimeEventKind::ExecOutput { chunk, .. } => Some(ThreadItem::ExecOutput {
            event_id: Some(event.event_id.clone()),
            text: chunk.clone(),
        }),
        RuntimeEventKind::ApprovalRequested {
            approval_id,
            tool_name,
            reason,
            checkpoint_id,
            permission_profile,
            filesystem_sandbox,
            network_sandbox,
            env_isolation,
            ..
        } => Some(ThreadItem::ApprovalRequested {
            event_id: Some(event.event_id.clone()),
            approval_id: approval_id.clone(),
            tool_name: tool_name.clone(),
            reason: reason.clone(),
            checkpoint_id: checkpoint_id.clone(),
            permission_profile: *permission_profile,
            filesystem_sandbox: filesystem_sandbox.clone(),
            network_sandbox: network_sandbox.clone(),
            env_isolation: env_isolation.clone(),
        }),
        RuntimeEventKind::ApprovalDecision {
            approval_id,
            status,
            note,
        } => Some(ThreadItem::ApprovalDecision {
            event_id: Some(event.event_id.clone()),
            approval_id: Some(approval_id.clone()),
            status: approval_status_name(status).to_string(),
            note: note.clone(),
        }),
        RuntimeEventKind::UserInputRequested {
            request_id,
            tool_name,
            questions,
        } => Some(ThreadItem::UserInputRequested {
            event_id: Some(event.event_id.clone()),
            request_id: request_id.clone(),
            tool_name: tool_name.clone(),
            questions: questions.clone(),
            status: "pending".to_string(),
        }),
        RuntimeEventKind::UserInputResolved {
            request_id,
            dismissed,
        } => Some(ThreadItem::UserInputResolved {
            event_id: Some(event.event_id.clone()),
            request_id: request_id.clone(),
            dismissed: *dismissed,
        }),
        RuntimeEventKind::RuntimeError { message } => Some(ThreadItem::RuntimeError {
            event_id: Some(event.event_id.clone()),
            message: message.clone(),
        }),
        RuntimeEventKind::CompactionWritten { .. } => Some(ThreadItem::CompactionWritten),
        RuntimeEventKind::SubagentSpawned {
            invocation_id,
            tool_call_id,
            parent_thread_id,
            child_thread_id,
            task_name,
            message_preview,
        } => Some(ThreadItem::SubagentSpawn {
            event_id: Some(event.event_id.clone()),
            invocation_id: invocation_id.clone(),
            tool_call_id: tool_call_id.clone(),
            parent_thread_id: parent_thread_id.clone(),
            child_thread_id: child_thread_id.clone(),
            task_name: task_name.clone(),
            message_preview: message_preview.clone(),
        }),
        RuntimeEventKind::SubagentClosed {
            invocation_id,
            tool_call_id,
            parent_thread_id,
            closed_thread_id,
            agent_path,
        } => Some(ThreadItem::SubagentClose {
            event_id: Some(event.event_id.clone()),
            invocation_id: invocation_id.clone(),
            tool_call_id: tool_call_id.clone(),
            parent_thread_id: parent_thread_id.clone(),
            closed_thread_id: closed_thread_id.clone(),
            agent_path: agent_path.clone(),
        }),
        RuntimeEventKind::InterAgentMessageSent {
            invocation_id,
            tool_call_id,
            author_thread_id,
            recipient_thread_id,
            author_path,
            recipient_path,
            content_preview,
            followup,
            started_turn_id,
        } => Some(ThreadItem::InterAgentMessage {
            event_id: Some(event.event_id.clone()),
            invocation_id: invocation_id.clone(),
            tool_call_id: tool_call_id.clone(),
            author_thread_id: author_thread_id.clone(),
            recipient_thread_id: recipient_thread_id.clone(),
            author_path: author_path.clone(),
            recipient_path: recipient_path.clone(),
            content_preview: content_preview.clone(),
            followup: *followup,
            started_turn_id: started_turn_id.clone(),
        }),
        RuntimeEventKind::ThreadGoalReport { report } => Some(ThreadItem::GoalReport {
            event_id: Some(event.event_id.clone()),
            report: report.clone(),
        }),
        RuntimeEventKind::TurnStarted
        | RuntimeEventKind::TurnCompleted
        | RuntimeEventKind::TurnInterrupted
        | RuntimeEventKind::AssistantTextDelta { .. }
        | RuntimeEventKind::ReasoningDelta { .. }
        | RuntimeEventKind::ToolInvocationStarted { .. }
        | RuntimeEventKind::ToolInvocationWaitingApproval { .. }
        | RuntimeEventKind::ToolInvocationWaitingUserInput { .. }
        | RuntimeEventKind::ToolInvocationOutputDelta { .. }
        | RuntimeEventKind::ToolInvocationCompleted { .. }
        | RuntimeEventKind::ToolInvocationFailed { .. }
        | RuntimeEventKind::ToolInvocationCancelled { .. }
        | RuntimeEventKind::TokenCount { .. }
        | RuntimeEventKind::ThreadGoalUpdated { .. }
        | RuntimeEventKind::ThreadGoalCleared { .. }
        | RuntimeEventKind::ThreadGoalContinuationStarted { .. }
        | RuntimeEventKind::ThreadGoalContinuationSuppressed { .. }
        | RuntimeEventKind::ThreadGoalTurnStarted { .. }
        | RuntimeEventKind::ThreadGoalToolCompleted { .. }
        | RuntimeEventKind::ReviewSubmitted { .. } => None,
    }
}

fn apply_tool_invocation_event(items: &mut Vec<ThreadItem>, kind: &RuntimeEventKind) {
    match kind {
        RuntimeEventKind::ToolInvocationStarted {
            invocation_id,
            tool_call_id,
            tool_name,
            mutating,
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                tool_call_id: item_tool_call_id,
                tool_name: item_tool_name,
                approval_id,
                request_id,
                status,
                mutating: item_mutating,
                reason,
                message,
                ..
            } = item
            {
                *item_tool_call_id = Some(tool_call_id.clone());
                *item_tool_name = Some(tool_name.clone());
                *approval_id = None;
                *request_id = None;
                *status = "started".to_string();
                *item_mutating = Some(*mutating);
                *reason = None;
                *message = None;
            }
        }
        RuntimeEventKind::ToolInvocationWaitingApproval {
            invocation_id,
            approval_id,
            reason,
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                approval_id: item_approval_id,
                request_id,
                status,
                reason: item_reason,
                message,
                ..
            } = item
            {
                *item_approval_id = Some(approval_id.clone());
                *request_id = None;
                *status = "waiting_approval".to_string();
                *item_reason = Some(reason.clone());
                *message = None;
            }
        }
        RuntimeEventKind::ToolInvocationWaitingUserInput {
            invocation_id,
            request_id: event_request_id,
            reason,
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                approval_id,
                request_id,
                status,
                reason: item_reason,
                message,
                ..
            } = item
            {
                *approval_id = None;
                *request_id = Some(event_request_id.clone());
                *status = "waiting_user_input".to_string();
                *item_reason = Some(reason.clone());
                *message = None;
            }
        }
        RuntimeEventKind::ToolInvocationOutputDelta {
            invocation_id,
            chunk,
            ..
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation { output_preview, .. } = item {
                match output_preview {
                    Some(preview) => preview.push_str(chunk),
                    None => *output_preview = Some(chunk.clone()),
                }
            }
        }
        RuntimeEventKind::ToolInvocationCompleted {
            invocation_id,
            tool_call_id,
            tool_name,
            ..
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                tool_call_id: item_tool_call_id,
                tool_name: item_tool_name,
                status,
                reason,
                message,
                request_id,
                ..
            } = item
            {
                *item_tool_call_id = Some(tool_call_id.clone());
                *item_tool_name = Some(tool_name.clone());
                *status = "completed".to_string();
                *reason = None;
                *message = None;
                *request_id = None;
            }
        }
        RuntimeEventKind::ToolInvocationFailed {
            invocation_id,
            tool_call_id,
            tool_name,
            message,
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                tool_call_id: item_tool_call_id,
                tool_name: item_tool_name,
                status,
                reason,
                message: item_message,
                request_id,
                ..
            } = item
            {
                *item_tool_call_id = Some(tool_call_id.clone());
                *item_tool_name = Some(tool_name.clone());
                *status = "failed".to_string();
                *reason = None;
                *item_message = Some(message.clone());
                *request_id = None;
            }
        }
        RuntimeEventKind::ToolInvocationCancelled {
            invocation_id,
            tool_call_id,
            tool_name,
            reason,
        } => {
            let item = ensure_tool_invocation_item(items, invocation_id);
            if let ThreadItem::ToolInvocation {
                tool_call_id: item_tool_call_id,
                tool_name: item_tool_name,
                status,
                reason: item_reason,
                message,
                request_id,
                ..
            } = item
            {
                *item_tool_call_id = Some(tool_call_id.clone());
                *item_tool_name = Some(tool_name.clone());
                *status = "cancelled".to_string();
                *item_reason = Some(reason.clone());
                *message = None;
                *request_id = None;
            }
        }
        _ => {}
    }
}

fn ensure_tool_invocation_item<'a>(
    items: &'a mut Vec<ThreadItem>,
    invocation_id: &str,
) -> &'a mut ThreadItem {
    if let Some(index) = items.iter().position(|item| {
        matches!(
            item,
            ThreadItem::ToolInvocation {
                invocation_id: item_invocation_id,
                ..
            } if item_invocation_id == invocation_id
        )
    }) {
        return &mut items[index];
    }

    items.push(ThreadItem::ToolInvocation {
        invocation_id: invocation_id.to_string(),
        tool_call_id: None,
        tool_name: None,
        approval_id: None,
        request_id: None,
        status: "started".to_string(),
        mutating: None,
        reason: None,
        message: None,
        output_preview: None,
    });
    items.last_mut().expect("inserted tool invocation item")
}

fn apply_approval_decision_to_tool_invocation(
    items: &mut [ThreadItem],
    approval_id: &ApprovalId,
    decision_status: &ApprovalStatus,
) {
    for item in items {
        let ThreadItem::ToolInvocation {
            approval_id: item_approval_id,
            status,
            reason,
            message,
            ..
        } = item
        else {
            continue;
        };
        if item_approval_id.as_ref() != Some(approval_id) {
            continue;
        }

        *status = approval_status_name(decision_status).to_string();
        *reason = None;
        *message = None;
    }
}

fn apply_user_input_resolved_to_tool_invocation(items: &mut [ThreadItem], request_id: &ApprovalId) {
    for item in items {
        let ThreadItem::ToolInvocation {
            request_id: item_request_id,
            status,
            reason,
            message,
            ..
        } = item
        else {
            continue;
        };
        if item_request_id.as_ref() != Some(request_id) {
            continue;
        }

        *status = "completed".to_string();
        *reason = None;
        *message = None;
    }
}

fn approval_status_name(status: &ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::{
        AssistantTurn, EventId, ReasoningBlock, ReasoningSignature, ToolCall, ToolStatus,
    };

    #[test]
    fn assistant_thread_item_projection_omits_reasoning_metadata() {
        let event = RuntimeEvent {
            event_id: EventId::new("evt_private"),
            thread_id: ThreadId::new("thread_private"),
            turn_id: Some(TurnId::new("turn_private")),
            kind: RuntimeEventKind::AssistantTurn {
                turn: AssistantTurn {
                    text: Some("visible answer".to_string()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "visible_tool".to_string(),
                        arguments: serde_json::json!({"visible": true}),
                        thought_signature: Some(serde_json::json!("hidden-tool-signature")),
                    }],
                    reasoning: vec![ReasoningBlock {
                        text: "hidden reasoning".to_string(),
                        signature: Some(ReasoningSignature::GeminiThoughtSignature(
                            "hidden-reasoning-signature".to_string(),
                        )),
                        redacted: false,
                    }],
                },
            },
        };

        let item = thread_item_from_event(&event).expect("assistant item");
        let value = serde_json::to_string(&item).expect("serialize item");

        assert!(value.contains("visible answer"));
        assert!(!value.contains("hidden reasoning"));
        assert!(!value.contains("hidden-reasoning-signature"));
        assert!(!value.contains("hidden-tool-signature"));
        assert!(!value.contains("reasoning"));
        assert!(!value.contains("thought_signature"));
    }

    #[test]
    fn assistant_thread_item_projection_exposes_reasoning_item_separately() {
        let thread_id = ThreadId::new("thread_reasoning_projection");
        let turn_id = TurnId::new("turn_reasoning_projection");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_start"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_reasoning"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::Reasoning {
                    summary: vec!["Checked the request shape.".to_string()],
                    content: vec!["raw provider reasoning".to_string()],
                },
            },
            RuntimeEvent {
                event_id: EventId::new("evt_assistant"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::AssistantTurn {
                    turn: AssistantTurn {
                        text: Some("visible answer".to_string()),
                        tool_calls: vec![],
                        reasoning: vec![ReasoningBlock {
                            text: "raw provider reasoning".to_string(),
                            signature: Some(ReasoningSignature::OpenAiField {
                                field: "reasoning_content".to_string(),
                            }),
                            redacted: false,
                        }],
                    },
                },
            },
            RuntimeEvent {
                event_id: EventId::new("evt_done"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &[]);
        assert_eq!(view.turns.len(), 1);
        assert_eq!(
            view.turns[0].items,
            vec![
                ThreadItem::Reasoning {
                    event_id: Some(EventId::new("evt_reasoning")),
                    summary: vec!["Checked the request shape.".to_string()],
                    content: vec!["raw provider reasoning".to_string()],
                },
                ThreadItem::AssistantMessage {
                    event_id: Some(EventId::new("evt_assistant")),
                    text: Some("visible answer".to_string()),
                },
            ]
        );
    }

    #[test]
    fn thread_view_projects_goal_report_as_distinct_item() {
        let thread_id = ThreadId::new("thread_goal_report_projection");
        let turn_id = TurnId::new("turn_goal_report_projection");
        let report = crate::app_server::protocol::ThreadGoalReport {
            goal_id: "goal_1".to_string(),
            objective: "ship morning report".to_string(),
            final_status: crate::app_server::protocol::ThreadGoalStatus::Complete,
            turns_run: 3,
            tokens_used: 800,
            token_budget: Some(1_000),
            time_used_seconds: 90,
            changed_files: vec![
                "src/runtime/goal/runtime.rs".to_string(),
                "apps/desktop/src/components/TranscriptList.tsx".to_string(),
            ],
            pending_approvals_count: 2,
            summary: "The goal completed after runtime and desktop updates.".to_string(),
        };
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_start"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_goal_report"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ThreadGoalReport {
                    report: report.clone(),
                },
            },
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &[]);

        assert_eq!(view.turns.len(), 1);
        assert_eq!(
            view.turns[0].items,
            vec![ThreadItem::GoalReport {
                event_id: Some(EventId::new("evt_goal_report")),
                report,
            }]
        );
    }

    #[test]
    fn thread_view_projects_goal_report_without_turn_id() {
        let thread_id = ThreadId::new("thread_goal_report_without_turn");
        let report = crate::app_server::protocol::ThreadGoalReport {
            goal_id: "goal_1".to_string(),
            objective: "ship morning report".to_string(),
            final_status: crate::app_server::protocol::ThreadGoalStatus::Complete,
            turns_run: 3,
            tokens_used: 800,
            token_budget: Some(1_000),
            time_used_seconds: 90,
            changed_files: vec!["src/runtime/goal/runtime.rs".to_string()],
            pending_approvals_count: 0,
            summary: "The goal completed after an external status update.".to_string(),
        };
        let events = vec![RuntimeEvent {
            event_id: EventId::new("evt_goal_report"),
            thread_id: thread_id.clone(),
            turn_id: None,
            kind: RuntimeEventKind::ThreadGoalReport {
                report: report.clone(),
            },
        }];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &[]);

        assert_eq!(view.turns.len(), 1);
        assert_eq!(view.turns[0].status, TurnStatus::Completed);
        assert_eq!(
            view.turns[0].items,
            vec![ThreadItem::GoalReport {
                event_id: Some(EventId::new("evt_goal_report")),
                report,
            }]
        );
    }

    #[test]
    fn thread_view_projects_reasoning_from_turn_id_response_items() {
        let thread_id = ThreadId::new("thread_reasoning_response_item");
        let turn_id = TurnId::new("turn_reasoning_response_item");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_start"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_done"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];
        let response_items = vec![
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::user("你真的没有在思考吗"),
            ),
            ResponseItem::for_turn(
                turn_id,
                ConversationMessage::assistant_with_reasoning(
                    Some("我在处理，但没有主观意识。".to_string()),
                    vec![
                        ReasoningBlock {
                            text: "provider reasoning".to_string(),
                            signature: Some(ReasoningSignature::OpenAiField {
                                field: "reasoning_content".to_string(),
                            }),
                            redacted: false,
                        },
                        ReasoningBlock {
                            text: "redacted provider reasoning".to_string(),
                            signature: None,
                            redacted: true,
                        },
                    ],
                    vec![],
                ),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &response_items);

        assert_eq!(
            view.turns[0].items,
            vec![
                ThreadItem::UserMessage {
                    text: "你真的没有在思考吗".to_string(),
                    input: vec![],
                },
                ThreadItem::Reasoning {
                    event_id: None,
                    summary: vec![],
                    content: vec!["provider reasoning".to_string()],
                },
                ThreadItem::AssistantMessage {
                    event_id: None,
                    text: Some("我在处理，但没有主观意识。".to_string()),
                },
            ]
        );
    }

    #[test]
    fn thread_view_projects_turn_id_response_items_without_event_skeletons() {
        let thread_id = ThreadId::new("thread_response_items_without_events");
        let turn_id = TurnId::new("turn_response_items_without_events");
        let response_items = vec![
            ResponseItem::for_turn(turn_id.clone(), ConversationMessage::user("hi")),
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::assistant(Some("hello".to_string()), vec![]),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, vec![], &response_items);

        assert_eq!(view.turns.len(), 1);
        assert_eq!(view.turns[0].id, turn_id);
        assert_eq!(view.turns[0].status, TurnStatus::Completed);
        assert_eq!(
            view.turns[0].items,
            vec![
                ThreadItem::UserMessage {
                    text: "hi".to_string(),
                    input: vec![],
                },
                ThreadItem::AssistantMessage {
                    event_id: None,
                    text: Some("hello".to_string())
                }
            ]
        );
    }

    #[test]
    fn thread_view_projects_user_image_input() {
        let thread_id = ThreadId::new("thread_response_item_with_image");
        let turn_id = TurnId::new("turn_response_item_with_image");
        let image_path = std::path::PathBuf::from("/tmp/exagent-input.png");
        let response_items = vec![ResponseItem::for_turn(
            turn_id.clone(),
            ConversationMessage::user_parts(vec![
                UserInput::Text {
                    text: "Use this screenshot".to_string(),
                },
                UserInput::LocalImage {
                    path: image_path.clone(),
                    detail: Some(crate::types::ImageDetail::High),
                },
            ]),
        )];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, vec![], &response_items);

        assert_eq!(view.turns.len(), 1);
        assert_eq!(
            view.turns[0].items,
            vec![ThreadItem::UserMessage {
                text: "Use this screenshot".to_string(),
                input: vec![
                    UserInput::Text {
                        text: "Use this screenshot".to_string()
                    },
                    UserInput::LocalImage {
                        path: image_path,
                        detail: Some(crate::types::ImageDetail::High)
                    }
                ],
            }]
        );
    }

    #[test]
    fn thread_view_ignores_fork_context_response_items() {
        let thread_id = ThreadId::new("thread_fork_context_response_items");
        let response_items = vec![
            ResponseItem::for_turn(
                TurnId::new(FORK_CONTEXT_TURN_ID),
                ConversationMessage::user("parent context"),
            ),
            ResponseItem::for_turn(
                TurnId::new(FORK_CONTEXT_TURN_ID),
                ConversationMessage::assistant(Some("parent answer".to_string()), vec![]),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, vec![], &response_items);

        assert!(view.turns.is_empty());
    }

    #[test]
    fn thread_view_projects_user_and_assistant_messages_from_turn_id_items() {
        let thread_id = ThreadId::new("thread_with_user_messages");
        let turn_id = TurnId::new("turn_1");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];
        let response_items = vec![
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::injected_system("runtime context"),
            ),
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::user("hi 介绍一下你自己吧"),
            ),
            ResponseItem::for_turn(
                turn_id,
                ConversationMessage::assistant(Some("你好，我是 ExAgent。".to_string()), vec![]),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &response_items);

        assert_eq!(view.turns.len(), 1);
        assert_eq!(
            view.turns[0].items.first(),
            Some(&ThreadItem::UserMessage {
                text: "hi 介绍一下你自己吧".to_string(),
                input: vec![],
            })
        );
        assert_eq!(
            view.turns[0].items.get(1),
            Some(&ThreadItem::AssistantMessage {
                event_id: None,
                text: Some("你好，我是 ExAgent。".to_string())
            })
        );
    }

    #[test]
    fn thread_view_hides_goal_internal_context() {
        let thread_id = ThreadId::new("thread_goal_internal_context");
        let turn_id = TurnId::new("turn_goal_internal_context");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_start"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_done"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];
        let response_items = vec![
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::injected_user_context("goal_snapshot", "hidden goal state"),
            ),
            ResponseItem::for_turn(turn_id, ConversationMessage::user("visible user message")),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &response_items);

        assert_eq!(
            view.turns[0].items,
            vec![ThreadItem::UserMessage {
                text: "visible user message".to_string(),
                input: vec![],
            }]
        );
    }

    #[test]
    fn thread_view_projects_assistant_messages_around_tool_invocations() {
        let thread_id = ThreadId::new("thread_with_tool_messages");
        let turn_id = TurnId::new("turn_1");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ToolInvocationStarted {
                    invocation_id: "inv_call_1".to_string(),
                    tool_call_id: "call_1".to_string(),
                    tool_name: "run_command".to_string(),
                    mutating: false,
                },
            },
            RuntimeEvent {
                event_id: EventId::new("evt_3"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::ToolInvocationCompleted {
                    invocation_id: "inv_call_1".to_string(),
                    tool_call_id: "call_1".to_string(),
                    tool_name: "run_command".to_string(),
                    status: ToolStatus::Success,
                },
            },
            RuntimeEvent {
                event_id: EventId::new("evt_4"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];
        let response_items = vec![
            ResponseItem::for_turn(turn_id.clone(), ConversationMessage::user("看看文件")),
            ResponseItem::for_turn(
                turn_id.clone(),
                ConversationMessage::assistant(
                    Some("我先看一下。".to_string()),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "run_command".to_string(),
                        arguments: serde_json::json!({"command": "ls"}),
                        thought_signature: None,
                    }],
                ),
            ),
            ResponseItem::for_turn(
                turn_id,
                ConversationMessage::assistant(Some("看完了。".to_string()), vec![]),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &response_items);
        let items = &view.turns[0].items;

        assert!(matches!(items[0], ThreadItem::UserMessage { .. }));
        assert_eq!(
            items[1],
            ThreadItem::AssistantMessage {
                event_id: None,
                text: Some("我先看一下。".to_string())
            }
        );
        assert!(matches!(items[2], ThreadItem::ToolInvocation { .. }));
        assert_eq!(
            items[3],
            ThreadItem::AssistantMessage {
                event_id: None,
                text: Some("看完了。".to_string())
            }
        );
    }

    #[test]
    fn thread_view_prefers_response_item_turn_ids_over_sequence_inference() {
        let thread_id = ThreadId::new("thread_with_explicit_turn_ids");
        let turn_a = TurnId::new("turn_a");
        let turn_b = TurnId::new("turn_b");
        let events = vec![
            RuntimeEvent {
                event_id: EventId::new("evt_a1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_a.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_a2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_a.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_b1"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_b.clone()),
                kind: RuntimeEventKind::TurnStarted,
            },
            RuntimeEvent {
                event_id: EventId::new("evt_b2"),
                thread_id: thread_id.clone(),
                turn_id: Some(turn_b.clone()),
                kind: RuntimeEventKind::TurnCompleted,
            },
        ];
        let response_items = vec![
            ResponseItem::for_turn(turn_a.clone(), ConversationMessage::user("question A")),
            ResponseItem::for_turn(turn_b.clone(), ConversationMessage::user("question B")),
            ResponseItem::for_turn(
                turn_a,
                ConversationMessage::assistant(Some("answer A".to_string()), vec![]),
            ),
            ResponseItem::for_turn(
                turn_b,
                ConversationMessage::assistant(Some("answer B".to_string()), vec![]),
            ),
        ];

        let view = build_thread_view(thread_id, ThreadStatus::Idle, None, events, &response_items);

        assert_eq!(
            view.turns[0].items,
            vec![
                ThreadItem::UserMessage {
                    text: "question A".to_string(),
                    input: vec![],
                },
                ThreadItem::AssistantMessage {
                    event_id: None,
                    text: Some("answer A".to_string())
                }
            ]
        );
        assert_eq!(
            view.turns[1].items,
            vec![
                ThreadItem::UserMessage {
                    text: "question B".to_string(),
                    input: vec![],
                },
                ThreadItem::AssistantMessage {
                    event_id: None,
                    text: Some("answer B".to_string())
                }
            ]
        );
    }
}
