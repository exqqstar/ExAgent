use anyhow::{anyhow, Result};

use super::super::{LiveEventSink, ThreadEventRecorder, ThreadSession};
use super::recording::record_token_count_event;
use super::turn_config::{
    agent_profile_context_for_turn, config_for_turn, effective_profile_agent_type_for_turn,
    TurnThinkingModeOverride,
};
use crate::agent::Agent;
use crate::events::RuntimeEventKind;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::context::{ContextManager, PromptContext, TurnPaths};
use crate::runtime::thread_runtime::ThreadOpResult;
use crate::runtime::turn_mode::TurnMode;
use crate::session::{CompactionSummary, ThreadSnapshot};
use crate::state::rollout::{CompactedItem, RolloutItem};
use crate::types::{ConversationMessage, MessageRole, TurnId};
use std::path::PathBuf;

impl ThreadSession {
    pub(super) async fn handle_manual_compaction_inner(&mut self) -> Result<ThreadOpResult> {
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        let input_modalities = &self.agent.config().model.capabilities.input_modalities;
        let history = self.context_manager.for_compaction(input_modalities);
        if history.is_empty() {
            return Ok(ThreadOpResult::Ack);
        }

        let result = crate::runtime::compaction::compact_history(&self.agent, &history).await?;
        record_compaction_checkpoint(
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            None,
            result.summary,
            result.replacement_history,
        )?;
        Ok(ThreadOpResult::Ack)
    }

    pub(super) async fn compact_before_turn_if_needed(
        &mut self,
        turn_id: &TurnId,
        snapshot: &mut ThreadSnapshot,
    ) -> Result<()> {
        let Some(limit) = self.agent.config().resolved_auto_compact_token_limit() else {
            return Ok(());
        };
        if self.context_manager.active_context_tokens() < limit {
            return Ok(());
        }
        let input_modalities = &self.agent.config().model.capabilities.input_modalities;
        if self
            .context_manager
            .for_compaction(input_modalities)
            .is_empty()
        {
            return Ok(());
        }

        let history = self.context_manager.for_compaction(input_modalities);
        let result = crate::runtime::compaction::compact_history(&self.agent, &history).await?;
        record_compaction_checkpoint(
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            snapshot,
            Some(turn_id),
            result.summary,
            result.replacement_history,
        )
    }
}

pub(super) async fn compact_after_context_window_error(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    turn_cwd: Option<&PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: TurnThinkingModeOverride,
    turn_mode: TurnMode,
) -> Result<()> {
    let context_window = agent
        .config()
        .model
        .capabilities
        .context_window
        .ok_or_else(|| anyhow!("model context window is required for context-window retry"))?;
    context_manager.set_token_usage_full(context_window);
    record_token_count_event(recorder, context_manager, snapshot, Some(turn_id))?;

    let history =
        context_manager.for_compaction(&agent.config().model.capabilities.input_modalities);
    let last_user_message = history
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .cloned();
    let result = crate::runtime::compaction::compact_history(agent, &history).await?;

    record_compaction_checkpoint(
        recorder,
        rollout_store,
        context_manager,
        snapshot,
        Some(turn_id),
        result.summary,
        result.replacement_history,
    )?;
    restore_retry_context_after_compaction(
        agent,
        rollout_store,
        context_manager,
        snapshot,
        turn_id,
        turn_cwd,
        turn_model,
        turn_thinking_mode,
        turn_mode,
        last_user_message,
    )
}

fn restore_retry_context_after_compaction(
    agent: &Agent,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    turn_cwd: Option<&PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: TurnThinkingModeOverride,
    turn_mode: TurnMode,
    last_user_message: Option<ConversationMessage>,
) -> Result<()> {
    let context_cwd = turn_cwd.cloned().unwrap_or_else(|| snapshot.cwd.clone());
    let agent_profile = agent_profile_context_for_turn(snapshot, turn_mode);
    let effective_profile_agent_type = effective_profile_agent_type_for_turn(snapshot, turn_mode);
    let turn_config = config_for_turn(
        agent.config(),
        turn_model,
        turn_thinking_mode,
        effective_profile_agent_type,
    );
    let prompt_context = PromptContext::for_turn(
        turn_id.clone(),
        &turn_config,
        TurnPaths {
            workspace_root: snapshot.workspace_root.clone(),
            cwd: context_cwd,
        },
        agent_profile,
        turn_mode,
    );
    let turn_context = prompt_context.turn_context.clone();
    let context_messages = context_manager.apply_context_updates(prompt_context);
    let mut rollout_items = Vec::with_capacity(context_messages.len() + 2);
    rollout_items.push(RolloutItem::TurnContext(turn_context));
    rollout_items.extend(
        context_messages
            .into_iter()
            .map(|message| RolloutItem::response_item_for_turn(turn_id.clone(), message)),
    );

    if let Some(last_user_message) = last_user_message {
        context_manager.record_items([last_user_message.clone()]);
        rollout_items.push(RolloutItem::response_item_for_turn(
            turn_id.clone(),
            last_user_message,
        ));
    }

    context_manager.sync_snapshot(snapshot);
    rollout_store.append_items_blocking(&rollout_items)?;
    Ok(())
}

pub(super) fn record_compaction_checkpoint(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: Option<&TurnId>,
    summary_text: String,
    replacement_history: Vec<ConversationMessage>,
) -> Result<()> {
    let summary = CompactionSummary {
        summary: summary_text.clone(),
        source_event_ids: vec![],
    };

    context_manager.replace_history(replacement_history.clone(), None);
    context_manager.sync_snapshot(snapshot);
    snapshot.latest_compaction = Some(summary.clone());
    rollout_store.append_items_blocking(&[RolloutItem::Compacted(CompactedItem {
        message: summary_text,
        replacement_history: Some(replacement_history),
    })])?;
    recorder.record(
        snapshot,
        turn_id,
        RuntimeEventKind::CompactionWritten { summary },
    )?;
    record_token_count_event(recorder, context_manager, snapshot, turn_id)
}
