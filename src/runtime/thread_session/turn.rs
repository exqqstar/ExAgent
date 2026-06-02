use std::path::PathBuf;

use anyhow::{anyhow, Result};

use super::{LiveEventSink, RuntimeInterrupt, ThreadEventRecorder, ThreadSession};
use crate::agent::Agent;
use crate::config::{AgentConfig, ThinkingMode};
use crate::events::RuntimeEventKind;
use crate::llm::LlmRequestOptions;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::context::{ContextManager, PromptContext, TurnPaths};
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::runtime::tool_call_runtime::{ApprovalUpdate, ToolEffect, ToolExecutionOutcome};
use crate::session::{ApprovalId, ApprovalStatus, CompactionSummary, ThreadSnapshot};
use crate::state::rollout::{CompactedItem, RolloutItem};
use crate::types::{AssistantTurn, ConversationMessage, MessageRole, ToolCall, TurnId};

impl ThreadSession {
    pub(crate) async fn handle_user_input(
        &mut self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self
            .handle_user_input_inner(turn_id, prompt, turn_context, interrupt)
            .await;
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    async fn handle_user_input_inner(
        &mut self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let turn_cwd = turn_context
            .as_ref()
            .and_then(|context| context.cwd.clone());
        let turn_resolved_model = turn_context
            .as_ref()
            .and_then(|context| context.resolved_model.clone());
        let turn_thinking_mode = turn_context
            .as_ref()
            .and_then(|context| context.thinking_mode);

        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.ensure_agent_for_turn_model(turn_resolved_model.as_ref())?;

        let final_turn = if let Some(mut interrupt) = interrupt {
            let interrupted_during_compaction = tokio::select! {
                result = self.compact_before_turn_if_needed(&turn_id, &mut snapshot) => {
                    result?;
                    false
                }
                _ = &mut interrupt.interrupt_rx => true,
            };
            if interrupted_during_compaction {
                self.record_turn_interrupted(&snapshot, &turn_id, &interrupt.interrupted)?;
                return Err(ThreadRuntimeError::TurnInterrupted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                }
                .into());
            }

            let runtime_turn_cwd = turn_cwd.clone();
            self.record_user_turn_start(
                &turn_id,
                prompt,
                turn_cwd,
                turn_resolved_model.as_ref(),
                turn_thinking_mode,
                &mut snapshot,
            )?;
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                ..
            } = self;
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = run_session_turn(agent, recorder, rollout_store, context_manager, &mut snapshot, runtime_turn_id, runtime_turn_cwd, turn_resolved_model.as_ref(), turn_thinking_mode) => {
                    match result {
                        Ok(turn) => turn,
                        Err(err) => {
                            let message = err.to_string();
                            self.append_and_broadcast_snapshot(
                                &snapshot,
                                Some(&turn_id),
                                RuntimeEventKind::RuntimeError { message },
                            )?;
                            return Err(err);
                        }
                    }
                }
                _ = &mut interrupt.interrupt_rx => {
                    self.record_turn_interrupted(&snapshot, &turn_id, &interrupt.interrupted)?;
                    return Err(ThreadRuntimeError::TurnInterrupted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                    }.into());
                }
            }
        } else {
            self.compact_before_turn_if_needed(&turn_id, &mut snapshot)
                .await?;
            let runtime_turn_cwd = turn_cwd.clone();
            self.record_user_turn_start(
                &turn_id,
                prompt,
                turn_cwd,
                turn_resolved_model.as_ref(),
                turn_thinking_mode,
                &mut snapshot,
            )?;
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                ..
            } = self;
            match run_session_turn(
                agent,
                recorder,
                rollout_store,
                context_manager,
                &mut snapshot,
                turn_id.clone(),
                runtime_turn_cwd,
                turn_resolved_model.as_ref(),
                turn_thinking_mode,
            )
            .await
            {
                Ok(turn) => turn,
                Err(err) => {
                    let message = err.to_string();
                    self.append_and_broadcast_snapshot(
                        &snapshot,
                        Some(&turn_id),
                        RuntimeEventKind::RuntimeError { message },
                    )?;
                    return Err(err);
                }
            }
        };

        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::TurnCompleted,
        )?;

        Ok(ThreadOpResult::UserInput {
            turn_id,
            final_turn,
        })
    }

    fn ensure_agent_for_turn_model(
        &mut self,
        resolved_model: Option<&ResolvedModelConfig>,
    ) -> Result<()> {
        let Some(resolved_model) = resolved_model else {
            return Ok(());
        };
        if &self.agent.config().model == resolved_model {
            return Ok(());
        }

        let mut config = self.agent.config().clone();
        config.model = resolved_model.clone();
        self.agent = (self.agent_factory)(config)?;
        Ok(())
    }

    fn record_user_turn_start(
        &mut self,
        turn_id: &TurnId,
        prompt: String,
        turn_cwd: Option<PathBuf>,
        turn_model: Option<&ResolvedModelConfig>,
        turn_thinking_mode: Option<ThinkingMode>,
        snapshot: &mut ThreadSnapshot,
    ) -> Result<()> {
        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let context_cwd = turn_cwd.unwrap_or_else(|| snapshot.cwd.clone());
        let turn_config = config_for_turn(self.agent.config(), turn_model, turn_thinking_mode);
        let prompt_context = PromptContext::for_turn(
            &turn_config,
            TurnPaths {
                workspace_root: snapshot.workspace_root.clone(),
                cwd: context_cwd,
            },
        );
        let turn_context = prompt_context.turn_context.clone();
        let context_messages = self.context_manager.apply_context_updates(prompt_context);
        let user_message = ConversationMessage::user(prompt);
        self.context_manager.record_items([user_message.clone()]);
        self.context_manager.sync_snapshot(snapshot);
        let mut rollout_items = Vec::with_capacity(context_messages.len() + 2);
        rollout_items.push(RolloutItem::TurnContext(turn_context));
        rollout_items.extend(context_messages.into_iter().map(RolloutItem::ResponseItem));
        rollout_items.push(RolloutItem::ResponseItem(user_message));
        self.rollout_store.append_items_blocking(&rollout_items)?;
        self.append_and_broadcast_snapshot(snapshot, Some(turn_id), RuntimeEventKind::TurnStarted)?;
        Ok(())
    }

    fn record_turn_interrupted(
        &mut self,
        snapshot: &ThreadSnapshot,
        turn_id: &TurnId,
        interrupted: &std::sync::Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        self.append_and_broadcast_snapshot(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::TurnInterrupted,
        )?;
        interrupted.notify_one();
        Ok(())
    }

    pub(crate) async fn handle_approval_decision(
        &mut self,
        requested_turn_id: Option<TurnId>,
        approval_id: ApprovalId,
        status: ApprovalStatus,
        note: Option<String>,
    ) -> Result<ThreadOpResult> {
        if matches!(status, ApprovalStatus::Pending) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "approval decision cannot be pending".to_string(),
            }
            .into());
        }

        let (turn_id, mut snapshot) =
            self.resolve_pending_approval_turn(requested_turn_id, &approval_id)?;
        let cwd = snapshot.cwd.clone();
        let tool_runtime = self.agent.tool_runtime(
            snapshot.thread_id.clone(),
            turn_id.clone(),
            snapshot.workspace_root.clone(),
            cwd,
        );
        let decision = match status {
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Pending => unreachable!("pending status is rejected above"),
        };
        let call = ToolCall {
            id: format!("approval_decision_{}", approval_id.as_str()),
            name: "run_command".to_string(),
            arguments: serde_json::json!({
                "approval_id": approval_id.as_str(),
                "decision": decision,
            }),
        };
        let mut outcome = tool_runtime.execute(call).await;
        attach_approval_note(&mut outcome, &approval_id, note.clone());
        if !approval_outcome_matches(&outcome, &approval_id, &status) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: outcome.result.content,
            }
            .into());
        }
        record_tool_outcome(
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            &turn_id,
            outcome,
        )?;

        Ok(ThreadOpResult::ApprovalDecision {
            turn_id,
            approval_id,
            status,
        })
    }

    fn resolve_pending_approval_turn(
        &self,
        requested_turn_id: Option<TurnId>,
        approval_id: &ApprovalId,
    ) -> Result<(TurnId, ThreadSnapshot)> {
        let state = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        if !state
            .overlay
            .pending_approvals
            .iter()
            .any(|approval| &approval.approval_id == approval_id)
        {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: format!("unknown approval id: {}", approval_id.as_str()),
            }
            .into());
        }
        let approval_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested {
                    approval_id: event_approval_id,
                    ..
                } if event_approval_id == approval_id => event.turn_id.clone(),
                _ => None,
            });
        let latest_turn_id = state
            .events
            .iter()
            .rev()
            .find_map(|event| event.turn_id.clone());
        let resolved_turn_id = requested_turn_id
            .or(approval_turn_id)
            .or(latest_turn_id)
            .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: "approval has no turn id".to_string(),
            })?;
        if let Some(event_turn_id) = state
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                RuntimeEventKind::ApprovalRequested {
                    approval_id: event_approval_id,
                    ..
                } if event_approval_id == approval_id => event.turn_id.clone(),
                _ => None,
            })
        {
            if event_turn_id != resolved_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: self.thread_id.clone(),
                    reason: format!("approval turn is {}", event_turn_id.as_str()),
                }
                .into());
            }
        }

        Ok((resolved_turn_id, state.snapshot.clone()))
    }

    async fn compact_before_turn_if_needed(
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
        if self.context_manager.for_prompt().is_empty() {
            return Ok(());
        }

        let history = self.context_manager.for_prompt();
        let result = crate::runtime::compaction::compact_history(&self.agent, &history).await?;
        record_compaction_checkpoint(
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            snapshot,
            turn_id,
            result.summary,
            result.replacement_history,
        )
    }
}

async fn run_session_turn(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    runtime_turn_id: TurnId,
    turn_cwd: Option<PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: Option<ThinkingMode>,
) -> Result<AssistantTurn> {
    let cwd = turn_cwd.clone().unwrap_or_else(|| snapshot.cwd.clone());
    let tool_runtime = agent.tool_runtime(
        snapshot.thread_id.clone(),
        runtime_turn_id.clone(),
        snapshot.workspace_root.clone(),
        cwd,
    );
    let llm_options = LlmRequestOptions {
        model: turn_model
            .map(|model| model.identity.model_id.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        thinking_mode: turn_thinking_mode.or(agent.config().thinking_mode),
    };

    loop {
        let prompt = context_manager.for_prompt();
        let completion = match agent
            .sample_assistant_turn(&prompt, &tool_runtime.schemas(), &llm_options)
            .await
        {
            Ok(completion) => completion,
            Err(err)
                if crate::llm::is_context_window_error(&err)
                    && agent.config().model.capabilities.context_window.is_some() =>
            {
                compact_after_context_window_error(
                    agent,
                    recorder,
                    rollout_store,
                    context_manager,
                    snapshot,
                    &runtime_turn_id,
                    turn_cwd.as_ref(),
                    turn_model,
                    turn_thinking_mode,
                )
                .await?;
                let prompt = context_manager.for_prompt();
                agent
                    .sample_assistant_turn(&prompt, &tool_runtime.schemas(), &llm_options)
                    .await?
            }
            Err(err) => return Err(err),
        };
        let turn = completion.turn;
        let token_usage = completion.token_usage;
        record_assistant_turn(
            recorder,
            rollout_store,
            context_manager,
            snapshot,
            &runtime_turn_id,
            &turn,
        )?;
        if let Some(usage) = token_usage.as_ref() {
            context_manager.update_token_info_from_usage(
                usage,
                agent.config().model.capabilities.context_window,
            );
            record_token_count_event(recorder, context_manager, snapshot, &runtime_turn_id)?;
        }

        if turn.tool_calls.is_empty() {
            return Ok(turn);
        }

        for call in turn.tool_calls.clone() {
            let outcome = tool_runtime.execute(call).await;
            record_tool_outcome(
                recorder,
                rollout_store,
                context_manager,
                snapshot,
                &runtime_turn_id,
                outcome,
            )?;
        }
    }
}

async fn compact_after_context_window_error(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    turn_cwd: Option<&PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: Option<ThinkingMode>,
) -> Result<()> {
    let context_window = agent
        .config()
        .model
        .capabilities
        .context_window
        .ok_or_else(|| anyhow!("model context window is required for context-window retry"))?;
    context_manager.set_token_usage_full(context_window);
    record_token_count_event(recorder, context_manager, snapshot, turn_id)?;

    let history = context_manager.for_prompt();
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
        turn_id,
        result.summary,
        result.replacement_history,
    )?;
    restore_retry_context_after_compaction(
        agent,
        rollout_store,
        context_manager,
        snapshot,
        turn_cwd,
        turn_model,
        turn_thinking_mode,
        last_user_message,
    )
}

fn restore_retry_context_after_compaction(
    agent: &Agent,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_cwd: Option<&PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: Option<ThinkingMode>,
    last_user_message: Option<ConversationMessage>,
) -> Result<()> {
    let context_cwd = turn_cwd.cloned().unwrap_or_else(|| snapshot.cwd.clone());
    let turn_config = config_for_turn(agent.config(), turn_model, turn_thinking_mode);
    let prompt_context = PromptContext::for_turn(
        &turn_config,
        TurnPaths {
            workspace_root: snapshot.workspace_root.clone(),
            cwd: context_cwd,
        },
    );
    let turn_context = prompt_context.turn_context.clone();
    let context_messages = context_manager.apply_context_updates(prompt_context);
    let mut rollout_items = Vec::with_capacity(context_messages.len() + 2);
    rollout_items.push(RolloutItem::TurnContext(turn_context));
    rollout_items.extend(context_messages.into_iter().map(RolloutItem::ResponseItem));

    if let Some(last_user_message) = last_user_message {
        context_manager.record_items([last_user_message.clone()]);
        rollout_items.push(RolloutItem::ResponseItem(last_user_message));
    }

    context_manager.sync_snapshot(snapshot);
    rollout_store.append_items_blocking(&rollout_items)?;
    Ok(())
}

fn config_for_turn(
    config: &AgentConfig,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: Option<ThinkingMode>,
) -> AgentConfig {
    let mut config = config.clone();
    if let Some(model) = turn_model {
        config.model = model.clone();
    }
    if let Some(thinking_mode) = turn_thinking_mode {
        config.thinking_mode = Some(thinking_mode);
    }
    config
}

fn record_compaction_checkpoint(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
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
        Some(turn_id),
        RuntimeEventKind::CompactionWritten { summary },
    )?;
    record_token_count_event(recorder, context_manager, snapshot, turn_id)
}

fn record_token_count_event(
    recorder: &mut ThreadEventRecorder,
    context_manager: &ContextManager,
    snapshot: &ThreadSnapshot,
    turn_id: &TurnId,
) -> Result<()> {
    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::TokenCount {
            info: context_manager.token_info(),
        },
    )?;
    Ok(())
}

fn record_assistant_turn(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    turn: &AssistantTurn,
) -> Result<()> {
    if turn.text.is_some() || !turn.tool_calls.is_empty() {
        let message = ConversationMessage::assistant(turn.text.clone(), turn.tool_calls.clone());
        context_manager.record_items([message.clone()]);
        context_manager.sync_snapshot(snapshot);
        rollout_store.append_items_blocking(&[RolloutItem::ResponseItem(message)])?;
    }
    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::AssistantTurn { turn: turn.clone() },
    )?;
    Ok(())
}

fn record_tool_outcome(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    outcome: ToolExecutionOutcome,
) -> Result<()> {
    for effect in outcome.effects {
        apply_tool_effect(recorder, snapshot, turn_id, effect)?;
    }

    let result = outcome.result;
    let message =
        ConversationMessage::tool(result.tool_call_id.clone(), serde_json::to_string(&result)?);
    context_manager.record_items([message.clone()]);
    context_manager.sync_snapshot(snapshot);
    rollout_store.append_items_blocking(&[RolloutItem::ResponseItem(message)])?;
    recorder.record(
        snapshot,
        Some(turn_id),
        RuntimeEventKind::ToolResult {
            result: result.clone(),
        },
    )?;
    Ok(())
}

fn apply_tool_effect(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    effect: ToolEffect,
) -> Result<()> {
    match effect {
        ToolEffect::ExecSessionUpdate(update) => recorder.apply_exec_session_update(update),
        ToolEffect::ApprovalUpdate(update) => {
            apply_approval_update(recorder, snapshot, turn_id, update)
        }
    }
}

fn apply_approval_update(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
    update: ApprovalUpdate,
) -> Result<()> {
    match update {
        ApprovalUpdate::Requested {
            approval_id,
            tool_name,
            reason,
        } => {
            let event_id = recorder.reserve_event_id();
            recorder.apply_approval_requested(
                approval_id.clone(),
                event_id.clone(),
                tool_name.clone(),
                reason.clone(),
            )?;
            recorder.record_reserved(
                snapshot,
                Some(turn_id),
                event_id,
                RuntimeEventKind::ApprovalRequested {
                    approval_id,
                    tool_name,
                    reason,
                },
            )?;
        }
        ApprovalUpdate::Approved { approval_id, note } => {
            recorder.clear_approval(&approval_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Approved,
                    note,
                },
            )?;
        }
        ApprovalUpdate::Denied { approval_id, note } => {
            recorder.clear_approval(&approval_id)?;
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Denied,
                    note,
                },
            )?;
        }
    }

    Ok(())
}

fn attach_approval_note(
    outcome: &mut ToolExecutionOutcome,
    approval_id: &ApprovalId,
    note: Option<String>,
) {
    for effect in &mut outcome.effects {
        match effect {
            ToolEffect::ApprovalUpdate(ApprovalUpdate::Approved {
                approval_id: effect_approval_id,
                note: effect_note,
            })
            | ToolEffect::ApprovalUpdate(ApprovalUpdate::Denied {
                approval_id: effect_approval_id,
                note: effect_note,
            }) if effect_approval_id == approval_id => {
                *effect_note = note.clone();
            }
            _ => {}
        }
    }
}

fn approval_outcome_matches(
    outcome: &ToolExecutionOutcome,
    approval_id: &ApprovalId,
    status: &ApprovalStatus,
) -> bool {
    outcome.effects.iter().any(|effect| match (effect, status) {
        (
            ToolEffect::ApprovalUpdate(ApprovalUpdate::Approved {
                approval_id: effect_approval_id,
                ..
            }),
            ApprovalStatus::Approved,
        )
        | (
            ToolEffect::ApprovalUpdate(ApprovalUpdate::Denied {
                approval_id: effect_approval_id,
                ..
            }),
            ApprovalStatus::Denied,
        ) => effect_approval_id == approval_id,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use async_trait::async_trait;
    use tempfile::tempdir;
    use tokio::sync::Mutex as AsyncMutex;

    use crate::agent::Agent;
    use crate::config::AgentConfig;
    use crate::events::RuntimeEventKind;
    use crate::llm::{LlmClient, LlmRequestOptions, MockLlm};
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult};
    use crate::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
    use crate::session::ThreadSnapshot;
    use crate::state::rollout::{RolloutItem, RolloutStore};
    use crate::types::{
        AssistantTurn, ConversationMessage, LlmCompletion, ThreadId, TokenUsage, ToolCall, TurnId,
    };

    fn write_rollout_meta(config: &AgentConfig, thread_id: &ThreadId) {
        let snapshot = ThreadSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
        crate::state::rollout::RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(&[crate::state::rollout::RolloutItem::ThreadMeta(
                crate::state::rollout::thread_meta_from_snapshot(&snapshot),
            )])
            .expect("write rollout session meta");
    }

    fn append_rollout_items(config: &AgentConfig, thread_id: &ThreadId, items: &[RolloutItem]) {
        let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
        RolloutStore::new(rollout_paths.rollout_path)
            .append_items_blocking(items)
            .expect("append rollout items");
    }

    fn read_rollout_items(config: &AgentConfig, thread_id: &ThreadId) -> Vec<RolloutItem> {
        let rollout_paths = crate::state::rollout::rollout_paths(&config.workspace_root, thread_id);
        RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("read rollout items")
    }

    struct RecordingLlm {
        turns: AsyncMutex<VecDeque<AssistantTurn>>,
        prompt_lens: Arc<Mutex<Vec<usize>>>,
        prompt_contents: Option<Arc<Mutex<Vec<Vec<String>>>>>,
    }

    impl RecordingLlm {
        fn new(turns: Vec<AssistantTurn>, prompt_lens: Arc<Mutex<Vec<usize>>>) -> Self {
            Self {
                turns: AsyncMutex::new(turns.into()),
                prompt_lens,
                prompt_contents: None,
            }
        }

        fn with_prompt_contents(
            turns: Vec<AssistantTurn>,
            prompt_lens: Arc<Mutex<Vec<usize>>>,
            prompt_contents: Arc<Mutex<Vec<Vec<String>>>>,
        ) -> Self {
            Self {
                turns: AsyncMutex::new(turns.into()),
                prompt_lens,
                prompt_contents: Some(prompt_contents),
            }
        }
    }

    #[async_trait]
    impl LlmClient for RecordingLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[serde_json::Value],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.prompt_lens.lock().unwrap().push(messages.len());
            if let Some(prompt_contents) = &self.prompt_contents {
                prompt_contents.lock().unwrap().push(
                    messages
                        .iter()
                        .map(|message| message.content.clone())
                        .collect(),
                );
            }
            self.turns
                .lock()
                .await
                .pop_front()
                .map(AssistantTurn::into_completion)
                .ok_or_else(|| anyhow::anyhow!("RecordingLlm is out of scripted turns"))
        }
    }

    enum ScriptedLlmStep {
        Completion(LlmCompletion),
        Error(&'static str),
    }

    struct ScriptedLlm {
        steps: AsyncMutex<VecDeque<ScriptedLlmStep>>,
        prompt_contents: Arc<Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[serde_json::Value],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.prompt_contents.lock().unwrap().push(
                messages
                    .iter()
                    .map(|message| message.content.clone())
                    .collect(),
            );
            match self.steps.lock().await.pop_front() {
                Some(ScriptedLlmStep::Completion(completion)) => Ok(completion),
                Some(ScriptedLlmStep::Error(message)) => Err(anyhow::anyhow!(message)),
                None => Err(anyhow::anyhow!("ScriptedLlm is out of scripted steps")),
            }
        }
    }

    fn assistant_completion(text: impl Into<String>) -> LlmCompletion {
        AssistantTurn {
            text: Some(text.into()),
            tool_calls: vec![],
        }
        .into_completion()
    }

    #[tokio::test]
    async fn thread_session_handles_user_input_and_records_turn_lifecycle() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_turn");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let final_turn = AssistantTurn {
            text: Some("session turn complete".into()),
            tool_calls: vec![],
        };
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![final_turn.clone()])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        let result = session
            .handle_user_input(turn_id.clone(), "continue".into(), None, None)
            .await
            .expect("run turn");

        let ThreadOpResult::UserInput { final_turn, .. } = result else {
            panic!("expected user input result");
        };
        assert_eq!(final_turn.text.as_deref(), Some("session turn complete"));

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let replay = live_view.events;
        assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
        assert!(matches!(
            replay[1].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(replay[2].kind, RuntimeEventKind::TurnCompleted));
        assert_eq!(replay[0].turn_id.as_ref(), Some(&turn_id));
        assert_eq!(replay[2].turn_id.as_ref(), Some(&turn_id));

        let snapshot = live_view.snapshot;
        assert!(snapshot.reference_turn_context.is_some());
        assert!(snapshot.conversation[0]
            .content
            .contains("Runtime context:"));
        assert!(snapshot.conversation[1]
            .content
            .contains("Environment context:"));
    }

    #[tokio::test]
    async fn thread_session_reuses_live_agent_and_snapshot_across_turns() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_live_state");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let factory_calls = Arc::new(AtomicUsize::new(0));
        let factory_call_counter = factory_calls.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            factory_call_counter.fetch_add(1, Ordering::SeqCst);
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![
                    AssistantTurn {
                        text: Some("first turn complete".into()),
                        tool_calls: vec![],
                    },
                    AssistantTurn {
                        text: Some("second turn complete".into()),
                        tool_calls: vec![],
                    },
                ])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        let first = session
            .handle_user_input(TurnId::new("turn_1"), "first input".into(), None, None)
            .await
            .expect("first turn");
        let second = session
            .handle_user_input(TurnId::new("turn_2"), "second input".into(), None, None)
            .await
            .expect("second turn");

        let ThreadOpResult::UserInput {
            final_turn: first_final_turn,
            ..
        } = first
        else {
            panic!("expected first user input result");
        };
        let ThreadOpResult::UserInput {
            final_turn: second_final_turn,
            ..
        } = second
        else {
            panic!("expected second user input result");
        };
        assert_eq!(
            first_final_turn.text.as_deref(),
            Some("first turn complete")
        );
        assert_eq!(
            second_final_turn.text.as_deref(),
            Some("second turn complete")
        );
        assert_eq!(factory_calls.load(Ordering::SeqCst), 1);

        let snapshot =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle())
                .snapshot;
        let contents = snapshot
            .conversation
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents.len(), 6);
        assert!(contents[0].contains("Runtime context:"));
        assert!(contents[1].contains("Environment context:"));
        assert_eq!(
            &contents[2..],
            &[
                "first input",
                "first turn complete",
                "second input",
                "second turn complete"
            ]
        );
    }

    #[tokio::test]
    async fn thread_session_next_sampling_uses_committed_history() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_committed_history");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::new(
                    vec![
                        AssistantTurn {
                            text: None,
                            tool_calls: vec![ToolCall {
                                id: "call_missing".into(),
                                name: "missing_tool".into(),
                                arguments: serde_json::json!({}),
                            }],
                        },
                        AssistantTurn {
                            text: Some("done".into()),
                            tool_calls: vec![],
                        },
                    ],
                    prompt_lens_for_llm.clone(),
                )),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "use a tool".into(), None, None)
            .await
            .expect("run turn");

        assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 5]);
    }

    #[tokio::test]
    async fn thread_session_continues_until_assistant_turn_has_no_tool_calls() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_no_legacy_max_turns");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let mut turns = Vec::new();
            for index in 0..13 {
                turns.push(AssistantTurn {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: format!("call_{index}"),
                        name: "missing_tool".into(),
                        arguments: serde_json::json!({}),
                    }],
                });
            }
            turns.push(AssistantTurn {
                text: Some("done after tools".into()),
                tool_calls: vec![],
            });
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::new(turns, prompt_lens_for_llm.clone())),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        let result = session
            .handle_user_input(turn_id, "keep going".into(), None, None)
            .await
            .expect("run turn");
        let ThreadOpResult::UserInput { final_turn, .. } = result else {
            panic!("expected user input result");
        };

        assert_eq!(final_turn.text.as_deref(), Some("done after tools"));
        assert_eq!(prompt_lens.lock().unwrap().len(), 14);
    }

    #[tokio::test]
    async fn thread_session_sampling_prompt_includes_runtime_context() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_prompt_context");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().join("subdir"),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let prompt_contents_for_llm = prompt_contents.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::with_prompt_contents(
                    vec![AssistantTurn {
                        text: Some("context received".into()),
                        tool_calls: vec![],
                    }],
                    prompt_lens_for_llm.clone(),
                    prompt_contents_for_llm.clone(),
                )),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "inspect context".into(), None, None)
            .await
            .expect("run turn");

        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(*prompt_lens.lock().unwrap(), vec![3]);
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0][0].contains("Runtime context:"));
        assert!(prompts[0][1].contains("Environment context:"));
        assert_eq!(prompts[0][2], "inspect context");
    }

    /// Verifies the F2 streaming contract at the ThreadSession boundary:
    /// assistant/tool events are recorded one step at a time, and the final
    /// snapshot contains the conversation items that produced those events.
    #[tokio::test]
    async fn thread_session_streams_events_paired_with_snapshot() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_streaming_capture");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![
                    AssistantTurn {
                        text: None,
                        tool_calls: vec![ToolCall {
                            id: "call_x".into(),
                            name: "missing_tool".into(),
                            arguments: serde_json::json!({}),
                        }],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                    },
                ])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        let result = session
            .handle_user_input(turn_id.clone(), "hi".into(), None, None)
            .await
            .expect("session should complete the two-step turn");
        let ThreadOpResult::UserInput { final_turn, .. } = result else {
            panic!("expected user input result");
        };

        assert_eq!(final_turn.text.as_deref(), Some("done"));
        let replay =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle())
                .events;
        assert_eq!(replay.len(), 5, "expected lifecycle plus three step events");
        assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
        assert!(matches!(
            replay[1].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(
            replay[2].kind,
            RuntimeEventKind::ToolResult { .. }
        ));
        assert!(matches!(
            replay[3].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(replay[4].kind, RuntimeEventKind::TurnCompleted));

        let snapshot =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle())
                .snapshot;
        assert_eq!(snapshot.conversation.len(), 6);
        assert!(snapshot.conversation[0]
            .content
            .contains("Runtime context:"));
        assert!(snapshot.conversation[1]
            .content
            .contains("Environment context:"));
        assert_eq!(snapshot.conversation[2].content, "hi");
        assert_eq!(snapshot.conversation[5].content, "done");
    }

    #[tokio::test]
    async fn thread_session_pre_turn_compaction_skips_when_under_budget() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_pre_turn_compaction_skip");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            auto_compact_token_limit: Some(100_000),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        append_rollout_items(
            &config,
            &thread_id,
            &[
                RolloutItem::ResponseItem(ConversationMessage::user("old user")),
                RolloutItem::ResponseItem(ConversationMessage::assistant(
                    Some("old assistant".to_string()),
                    vec![],
                )),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                }])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "new user".into(), None, None)
            .await
            .expect("run turn");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert!(!live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
        assert!(!read_rollout_items(&config, &thread_id)
            .iter()
            .any(|item| matches!(item, RolloutItem::Compacted(_))));
    }

    #[tokio::test]
    async fn thread_session_pre_turn_compaction_runs_before_new_user_message() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_pre_turn_compaction_before_user");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            auto_compact_token_limit: Some(1),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        append_rollout_items(
            &config,
            &thread_id,
            &[
                RolloutItem::ResponseItem(ConversationMessage::user("old user")),
                RolloutItem::ResponseItem(ConversationMessage::assistant(
                    Some("old assistant".to_string()),
                    vec![],
                )),
            ],
        );
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let prompt_contents_for_llm = prompt_contents.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::with_prompt_contents(
                    vec![
                        AssistantTurn {
                            text: Some("summary from compaction".into()),
                            tool_calls: vec![],
                        },
                        AssistantTurn {
                            text: Some("done".into()),
                            tool_calls: vec![],
                        },
                    ],
                    prompt_lens_for_llm.clone(),
                    prompt_contents_for_llm.clone(),
                )),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id.clone(), "new user".into(), None, None)
            .await
            .expect("run turn");

        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(*prompt_lens.lock().unwrap(), vec![2, 4]);
        assert!(prompts[0].join("\n").contains("old user"));
        assert!(prompts[0].join("\n").contains("old assistant"));
        assert!(!prompts[0].join("\n").contains("new user"));
        assert!(prompts[1].join("\n").contains("summary from compaction"));
        assert!(prompts[1].join("\n").contains("new user"));
        assert!(!prompts[1].join("\n").contains("old user"));

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let contents = live_view
            .snapshot
            .conversation
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert!(contents[0].contains("summary from compaction"));
        assert_eq!(contents[3], "new user");
        assert_eq!(contents[4], "done");
        assert!(live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
        assert!(live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::TokenCount { .. })));

        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items
            .iter()
            .any(|item| matches!(item, RolloutItem::Compacted(compacted)
                if compacted.message == "summary from compaction")));
    }

    #[tokio::test]
    async fn thread_session_pre_turn_compaction_replays_replacement_history() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_pre_turn_compaction_replay");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            auto_compact_token_limit: Some(1),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        append_rollout_items(
            &config,
            &thread_id,
            &[
                RolloutItem::ResponseItem(ConversationMessage::user("old user")),
                RolloutItem::ResponseItem(ConversationMessage::assistant(
                    Some("old assistant".to_string()),
                    vec![],
                )),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![
                    AssistantTurn {
                        text: Some("summary from compaction".into()),
                        tool_calls: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                    },
                ])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");
        session
            .handle_user_input(turn_id, "new user".into(), None, None)
            .await
            .expect("run turn");
        drop(session);

        let resumed = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            Arc::new(move |config| {
                Ok(Agent::new(
                    config,
                    Box::new(MockLlm::new(vec![])),
                    ToolRegistry::new(),
                ))
            }),
        ))
        .expect("resume thread session");
        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &resumed.live_state_handle());
        let contents = live_view
            .snapshot
            .conversation
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();

        assert!(contents[0].contains("summary from compaction"));
        assert!(contents.contains(&"new user"));
        assert!(contents.contains(&"done"));
        assert!(!contents.contains(&"old user"));
        assert!(!contents.contains(&"old assistant"));
    }

    #[tokio::test]
    async fn thread_session_records_token_usage_after_assistant_response() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_records_token_usage");
        let turn_id = TurnId::new("turn_1");
        let usage = TokenUsage {
            input_tokens: 40,
            cached_input_tokens: 5,
            output_tokens: 10,
            reasoning_output_tokens: 2,
            total_tokens: 52,
        };
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(1_000);
        write_rollout_meta(&config, &thread_id);
        let usage_for_llm = usage.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new_completions(vec![LlmCompletion {
                    turn: AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                    },
                    token_usage: Some(usage_for_llm.clone()),
                }])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "count tokens".into(), None, None)
            .await
            .expect("run turn");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let token_info = live_view.events.iter().find_map(|event| match &event.kind {
            RuntimeEventKind::TokenCount { info } => info.clone(),
            _ => None,
        });
        let token_info = token_info.expect("token count event");
        assert_eq!(token_info.last_token_usage, usage);
        assert_eq!(token_info.total_token_usage, usage);
        assert_eq!(token_info.model_context_window, Some(1_000));

        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(event)
                if matches!(event.kind, RuntimeEventKind::TokenCount { info: Some(_) })
        )));
    }

    #[tokio::test]
    async fn thread_session_does_not_emit_token_count_without_model_usage() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_no_bogus_token_usage");
        let turn_id = TurnId::new("turn_1");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(1_000);
        write_rollout_meta(&config, &thread_id);
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                }])),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "count tokens".into(), None, None)
            .await
            .expect("run turn");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert!(!live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::TokenCount { .. })));
    }

    #[tokio::test]
    async fn context_window_error_compacts_and_retries_once() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_context_window_retry");
        let turn_id = TurnId::new("turn_1");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(1_000);
        write_rollout_meta(&config, &thread_id);
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents_for_llm = prompt_contents.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(ScriptedLlm {
                    steps: AsyncMutex::new(
                        vec![
                            ScriptedLlmStep::Error("context_length_exceeded: too many tokens"),
                            ScriptedLlmStep::Completion(assistant_completion(
                                "summary after context error",
                            )),
                            ScriptedLlmStep::Completion(assistant_completion("done after retry")),
                        ]
                        .into(),
                    ),
                    prompt_contents: prompt_contents_for_llm.clone(),
                }),
                ToolRegistry::new(),
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        let result = session
            .handle_user_input(turn_id.clone(), "new user".into(), None, None)
            .await
            .expect("retry should recover");
        let ThreadOpResult::UserInput { final_turn, .. } = result else {
            panic!("expected user input result");
        };

        assert_eq!(final_turn.text.as_deref(), Some("done after retry"));
        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0].join("\n").contains("new user"));
        assert!(prompts[1].join("\n").contains("new user"));
        assert!(prompts[2]
            .join("\n")
            .contains("summary after context error"));
        assert!(prompts[2].join("\n").contains("new user"));
        assert!(prompts[2]
            .iter()
            .any(|message| message.contains("Runtime context:")));
        assert!(prompts[2]
            .iter()
            .any(|message| message.contains("Environment context:")));
        let runtime_context_index = prompts[2]
            .iter()
            .position(|message| message.contains("Runtime context:"))
            .expect("runtime context in retry prompt");
        let user_index = prompts[2]
            .iter()
            .position(|message| message == "new user")
            .expect("current user in retry prompt");
        assert!(runtime_context_index < user_index);

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert!(live_view.events.iter().any(|event| matches!(
            &event.kind,
            RuntimeEventKind::TokenCount {
                info: Some(info)
            } if info.last_token_usage.total_tokens == 1_000
        )));
        assert!(live_view
            .events
            .iter()
            .any(|event| matches!(event.kind, RuntimeEventKind::CompactionWritten { .. })));
    }

    #[tokio::test]
    async fn context_window_error_retry_does_not_loop() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_context_window_retry_once");
        let turn_id = TurnId::new("turn_1");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(1_000);
        write_rollout_meta(&config, &thread_id);
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents_for_llm = prompt_contents.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(ScriptedLlm {
                    steps: AsyncMutex::new(
                        vec![
                            ScriptedLlmStep::Error("context_length_exceeded: too many tokens"),
                            ScriptedLlmStep::Completion(assistant_completion(
                                "summary after context error",
                            )),
                            ScriptedLlmStep::Error("maximum context length is 1000 tokens"),
                        ]
                        .into(),
                    ),
                    prompt_contents: prompt_contents_for_llm.clone(),
                }),
                ToolRegistry::new(),
            ))
        });
        let mut session =
            ThreadSession::new(ThreadSessionOptions::new(thread_id, config, agent_factory))
                .expect("create thread session");

        let error = match session
            .handle_user_input(turn_id, "new user".into(), None, None)
            .await
        {
            Ok(_) => panic!("retry should fail once"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("maximum context length"));
        assert_eq!(prompt_contents.lock().unwrap().len(), 3);
    }
}
