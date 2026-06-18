use anyhow::{anyhow, Result};

use super::super::{LiveEventSink, ThreadEventRecorder, ThreadSession};
use crate::events::RuntimeEventKind;
use crate::runtime::context::ContextManager;
use crate::runtime::tool::effects::ToolExecutionOutcome;
use crate::session::ThreadSnapshot;
use crate::state::rollout::RolloutItem;
use crate::types::{AssistantTurn, ConversationMessage, TurnId};
use tokio::sync::oneshot;

impl ThreadSession {
    pub(super) fn record_turn_started(
        &mut self,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
        start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    ) -> Result<()> {
        let result = self
            .append_and_broadcast_snapshot(snapshot, Some(turn_id), RuntimeEventKind::TurnStarted)
            .map(|_| turn_id.clone());
        if let Some(start_tx) = start_tx {
            let ack = result
                .as_ref()
                .map(|turn_id| turn_id.clone())
                .map_err(|err| anyhow!(err.to_string()));
            let _ = start_tx.send(ack);
        }
        result.map(|_| ())
    }

    pub(super) fn record_runtime_error(
        &mut self,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
        err: &anyhow::Error,
    ) -> Result<()> {
        self.append_and_broadcast_snapshot(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::RuntimeError {
                message: err.to_string(),
            },
        )?;
        Ok(())
    }

    pub(super) fn record_runtime_error_for_turn_from_live(
        &mut self,
        turn_id: &TurnId,
        err: &anyhow::Error,
    ) -> Result<()> {
        let snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.record_runtime_error(&snapshot, turn_id, err)
    }

    pub(crate) fn record_runtime_error_without_turn(&mut self, message: String) -> Result<()> {
        let snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.append_and_broadcast_snapshot(
            &snapshot,
            None,
            RuntimeEventKind::RuntimeError { message },
        )?;
        Ok(())
    }

    pub(super) fn publish_snapshot(&self, snapshot: &ThreadSnapshot) -> Result<()> {
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state.snapshot = snapshot.clone();
        Ok(())
    }

    pub(super) fn record_turn_interrupted(
        &mut self,
        snapshot: &mut ThreadSnapshot,
        turn_id: &TurnId,
        interrupted: &std::sync::Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        for invocation in self.recorder.take_active_tool_invocations()? {
            let tool_call_id = invocation.tool_call_id.clone();
            self.append_and_broadcast_snapshot(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ToolInvocationCancelled {
                    invocation_id: invocation.invocation_id,
                    tool_call_id: invocation.tool_call_id,
                    tool_name: invocation.tool_name,
                    reason: "interrupted".to_string(),
                },
            )?;
            let message = ConversationMessage::tool(tool_call_id, "interrupted");
            self.context_manager.record_items([message.clone()]);
            self.context_manager.sync_snapshot(snapshot);
            self.rollout_store
                .append_items_blocking(&[RolloutItem::response_item_for_turn(
                    turn_id.clone(),
                    message,
                )])?;
        }
        self.append_and_broadcast_snapshot(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;
        interrupted.notify_one();
        Ok(())
    }
}

pub(super) fn record_token_count_event(
    recorder: &mut ThreadEventRecorder,
    context_manager: &ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: Option<&TurnId>,
) -> Result<()> {
    context_manager.sync_snapshot(snapshot);
    recorder.record(
        snapshot,
        turn_id,
        RuntimeEventKind::TokenCount {
            info: context_manager.token_info(),
        },
    )?;
    Ok(())
}

pub(super) fn current_token_usage(context_manager: &ContextManager) -> crate::types::TokenUsage {
    context_manager
        .token_info()
        .map(|info| info.total_token_usage)
        .unwrap_or_default()
}

pub(super) fn assistant_turn_has_activity(turn: &AssistantTurn) -> bool {
    turn.text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
        || !turn.reasoning.is_empty()
        || !turn.tool_calls.is_empty()
}

pub(super) fn record_assistant_turn(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    turn: &AssistantTurn,
) -> Result<()> {
    if let Some((summary, content)) = displayable_reasoning_parts(turn) {
        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::Reasoning { summary, content },
        )?;
    }
    if turn.text.is_some() || !turn.tool_calls.is_empty() {
        let message = ConversationMessage::assistant_with_reasoning(
            turn.text.clone(),
            turn.reasoning.clone(),
            turn.tool_calls.clone(),
        );
        context_manager.record_items([message.clone()]);
        context_manager.sync_snapshot(snapshot);
        rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
            turn_id.clone(),
            message,
        )])?;
    }
    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::AssistantTurn { turn: turn.clone() },
    )?;
    Ok(())
}

fn displayable_reasoning_parts(turn: &AssistantTurn) -> Option<(Vec<String>, Vec<String>)> {
    let content = turn
        .reasoning
        .iter()
        .filter(|block| !block.redacted)
        .map(|block| block.text.trim())
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if content.is_empty() {
        None
    } else {
        Some((Vec::new(), content))
    }
}

pub(super) fn record_tool_outcome(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    outcome: ToolExecutionOutcome,
) -> Result<()> {
    outcome.apply_effects(recorder, snapshot, turn_id)?;

    let result = outcome.result;
    let message = ConversationMessage::tool_with_parts(
        result.tool_call_id.clone(),
        model_tool_message_content(&result),
        result.parts.clone(),
    );
    context_manager.record_items([message.clone()]);
    context_manager.sync_snapshot(snapshot);
    rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
        turn_id.clone(),
        message,
    )])?;
    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::ToolResult {
            result: result.clone(),
        },
    )?;
    Ok(())
}

pub(super) fn record_approval_decision_outcome(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    outcome: ToolExecutionOutcome,
) -> Result<()> {
    outcome.apply_effects(recorder, snapshot, turn_id)?;

    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::ToolResult {
            result: outcome.result,
        },
    )?;
    Ok(())
}

fn model_tool_message_content(result: &crate::types::ToolResult) -> String {
    result.content.clone()
}
