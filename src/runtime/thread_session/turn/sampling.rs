use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use super::super::{LiveEventSink, ThreadEventRecorder, ThreadInbox};
use super::compaction_flow::compact_after_context_window_error;
use super::goal_effects::{apply_goal_effect, changed_files_for_goal_report};
use super::recording::{
    current_token_usage, record_assistant_turn, record_token_count_event, record_tool_outcome,
};
use super::turn_config::{
    agent_tool_policy, config_for_turn, effective_profile_agent_type_for_turn,
    TurnThinkingModeOverride,
};
use crate::agent::Agent;
use crate::events::RuntimeEventKind;
use crate::llm::{LlmRequestOptions, LlmStreamEvent, LlmStreamSink};
use crate::resolved::ResolvedModelConfig;
use crate::runtime::context::ContextManager;
use crate::runtime::goal::runtime::{GoalRuntime, GoalRuntimeEvent};
use crate::runtime::turn_mode::TurnMode;
use crate::session::ThreadSnapshot;
use crate::state::rollout::RolloutItem;
use crate::tools::ToolSpec;
use crate::types::{AssistantTurn, ConversationMessage, InputModality, TurnId};

pub(super) async fn run_session_turn(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    goal_runtime: Option<&GoalRuntime>,
    snapshot: &mut ThreadSnapshot,
    runtime_turn_id: TurnId,
    turn_cwd: Option<PathBuf>,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: TurnThinkingModeOverride,
    turn_mode: TurnMode,
    inbox: Arc<ThreadInbox>,
) -> Result<AssistantTurn> {
    let cwd = turn_cwd.clone().unwrap_or_else(|| snapshot.cwd.clone());
    let effective_profile_agent_type = effective_profile_agent_type_for_turn(snapshot, turn_mode);
    let turn_config = config_for_turn(
        agent.config(),
        turn_model,
        turn_thinking_mode,
        effective_profile_agent_type,
    );
    let tool_runtime = agent
        .tool_runtime(
            snapshot.thread_id.clone(),
            runtime_turn_id.clone(),
            snapshot.workspace_root.clone(),
            cwd,
            Some(recorder.exec_output_event_sink()),
            agent_tool_policy(snapshot, turn_mode),
            Some(inbox.clone()),
        )
        .await?;
    let llm_options = LlmRequestOptions {
        model: turn_model
            .map(|model| model.identity.model_id.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        thinking_mode: turn_config.thinking_mode,
        reasoning_capabilities: turn_model.map(|model| model.capabilities.reasoning.clone()),
    };

    loop {
        drain_inbox_into_turn_context(
            inbox.as_ref(),
            recorder,
            rollout_store,
            context_manager,
            snapshot,
            &runtime_turn_id,
        )
        .await?;
        let tool_specs = tool_runtime.visible_specs();
        let prompt = prompt_for_sampling(
            context_manager,
            &turn_config.model.capabilities.input_modalities,
        );
        let completion = match stream_assistant_turn(
            agent,
            recorder,
            snapshot,
            &runtime_turn_id,
            &prompt,
            tool_specs,
            &llm_options,
        )
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
                    turn_mode,
                )
                .await?;
                let tool_specs = tool_runtime.visible_specs();
                let prompt = prompt_for_sampling(
                    context_manager,
                    &turn_config.model.capabilities.input_modalities,
                );
                stream_assistant_turn(
                    agent,
                    recorder,
                    snapshot,
                    &runtime_turn_id,
                    &prompt,
                    tool_specs,
                    &llm_options,
                )
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
            record_token_count_event(recorder, context_manager, snapshot, Some(&runtime_turn_id))?;
        }

        if turn.tool_calls.is_empty() {
            if inbox.has_pending().await {
                continue;
            }
            return Ok(turn);
        }

        for call in turn.tool_calls.clone() {
            let tool_name = call.name.clone();
            let outcome = tool_runtime
                .execute_with_lifecycle(call, recorder, snapshot, &runtime_turn_id)
                .await?;
            let changed_files = changed_files_for_goal_report(&tool_name, &outcome.result);
            record_tool_outcome(
                recorder,
                rollout_store,
                context_manager,
                snapshot,
                &runtime_turn_id,
                outcome,
            )?;
            if let Some(goal_runtime) = goal_runtime {
                let goal_id_for_tool = if changed_files.is_empty() {
                    None
                } else {
                    goal_runtime
                        .active_goal_id_for_turn(&snapshot.thread_id, &runtime_turn_id)
                        .await
                };
                if let Some(goal_id) = goal_id_for_tool {
                    recorder.record(
                        snapshot,
                        Some(&runtime_turn_id),
                        RuntimeEventKind::ThreadGoalToolCompleted {
                            goal_id,
                            changed_files: changed_files.clone(),
                        },
                    )?;
                }
                let event = if tool_name == "update_goal" {
                    GoalRuntimeEvent::ToolCompletedGoal {
                        thread_id: &snapshot.thread_id,
                        turn_id: &runtime_turn_id,
                        token_usage: current_token_usage(context_manager),
                    }
                } else {
                    GoalRuntimeEvent::ToolCompleted {
                        thread_id: &snapshot.thread_id,
                        turn_id: &runtime_turn_id,
                        tool_name: &tool_name,
                        token_usage: current_token_usage(context_manager),
                        changed_files,
                    }
                };
                let effect = goal_runtime.apply(event).await?;
                apply_goal_effect(
                    Some(agent),
                    recorder,
                    rollout_store,
                    context_manager,
                    snapshot,
                    Some(&runtime_turn_id),
                    effect,
                )
                .await?;
            }
        }
    }
}

fn prompt_for_sampling(
    context_manager: &ContextManager,
    input_modalities: &[InputModality],
) -> Vec<ConversationMessage> {
    // Subagent collaboration guidance lives in the tool descriptions
    // themselves (see `spawn_agent`), not in a per-turn injected message.
    // Tool-level guidance is only visible when the tool is, so a worker that
    // cannot spawn never sees spawn guidance, and the prompt stays cacheable.
    context_manager.for_prompt(input_modalities)
}

async fn drain_inbox_into_turn_context(
    inbox: &ThreadInbox,
    recorder: &ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: &TurnId,
) -> Result<usize> {
    let mails = inbox.drain().await;
    if mails.is_empty() {
        return Ok(0);
    }

    let mailbox_messages = context_manager.record_inter_agent_communications(mails);
    let rollout_items = mailbox_messages
        .into_iter()
        .map(|message| RolloutItem::response_item_for_turn(turn_id.clone(), message))
        .collect::<Vec<_>>();
    rollout_store.append_items_blocking(&rollout_items)?;
    context_manager.sync_snapshot(snapshot);
    recorder.publish_snapshot(snapshot)?;
    Ok(rollout_items.len())
}

async fn stream_assistant_turn(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    _snapshot: &ThreadSnapshot,
    turn_id: &TurnId,
    prompt: &[ConversationMessage],
    tool_specs: &[ToolSpec],
    llm_options: &LlmRequestOptions,
) -> Result<crate::types::LlmCompletion> {
    let mut sink = RuntimeLlmStreamSink { recorder, turn_id };
    agent
        .stream_assistant_turn(prompt, tool_specs, llm_options, &mut sink)
        .await
}

struct RuntimeLlmStreamSink<'a> {
    recorder: &'a mut ThreadEventRecorder,
    turn_id: &'a TurnId,
}

#[async_trait::async_trait]
impl LlmStreamSink for RuntimeLlmStreamSink<'_> {
    async fn event(&mut self, event: LlmStreamEvent) -> Result<()> {
        match event {
            LlmStreamEvent::ReasoningDelta(delta) => {
                self.recorder.record_live_only(
                    Some(self.turn_id),
                    RuntimeEventKind::ReasoningDelta { delta },
                )?;
            }
            LlmStreamEvent::AssistantTextDelta(delta) => {
                self.recorder.record_live_only(
                    Some(self.turn_id),
                    RuntimeEventKind::AssistantTextDelta { delta },
                )?;
            }
            LlmStreamEvent::Completed(_) => {}
        }
        Ok(())
    }
}
