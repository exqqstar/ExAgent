use anyhow::Result;

use super::super::context_refresh::refresh_file_backed_contexts;
use super::super::ThreadSession;
use super::goal_effects::apply_goal_effect;
use super::recording::current_token_usage;
use super::turn_config::{
    agent_profile_context_for_turn, config_for_turn, effective_profile_agent_type_for_turn,
    TurnThinkingModeOverride,
};
use crate::resolved::ResolvedModelConfig;
use crate::runtime::context::{PromptContext, TurnPaths};
use crate::runtime::goal::runtime::{GoalRuntimeEvent, GoalTurnTrigger};
use crate::runtime::memory::context::{format_auto_memory_context, format_frozen_memory_block};
use crate::runtime::turn_mode::TurnMode;
use crate::session::ThreadSnapshot;
use crate::state::memory::query::should_auto_recall;
use crate::state::memory::{MemoryRecallMode, MemoryScope, MemorySearchQuery};
use crate::state::rollout::RolloutItem;
use crate::types::{ConversationMessage, TurnId, UserInput};
use std::path::PathBuf;

impl ThreadSession {
    pub(super) async fn ensure_agent_for_turn_model(
        &mut self,
        resolved_model: Option<&ResolvedModelConfig>,
    ) -> Result<()> {
        let desired_model = resolved_model.unwrap_or(&self.base_config.model);
        if &self.agent.config().model == desired_model {
            return Ok(());
        }

        let mut config = self.base_config.clone();
        config.model = desired_model.clone();
        self.agent.shutdown().await;
        let goal_api = self.goal_runtime.as_ref().map(|runtime| {
            std::sync::Arc::new(crate::runtime::goal::GoalToolApi::new(runtime.clone()))
        });
        let memory_api = self.memory_runtime.as_ref().map(|runtime| {
            std::sync::Arc::new(crate::runtime::memory::MemoryToolApi::new(runtime.clone()))
        });
        self.agent = (self.agent_factory)(config)?
            .with_subagent_control(self.subagent_control.clone())
            .with_goal_api(goal_api)
            .with_memory_api(memory_api)
            .with_forge_review_store(self.forge_review_store.clone());
        Ok(())
    }

    pub(super) async fn record_user_turn_start(
        &mut self,
        turn_id: &TurnId,
        input: &[UserInput],
        turn_cwd: Option<PathBuf>,
        turn_model: Option<&ResolvedModelConfig>,
        turn_thinking_mode: TurnThinkingModeOverride,
        turn_mode: TurnMode,
        snapshot: &mut ThreadSnapshot,
    ) -> Result<()> {
        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let context_cwd = turn_cwd.unwrap_or_else(|| snapshot.cwd.clone());
        let agent_profile = agent_profile_context_for_turn(snapshot, turn_mode);
        let effective_profile_agent_type =
            effective_profile_agent_type_for_turn(snapshot, turn_mode);
        let turn_config = config_for_turn(
            self.agent.config(),
            turn_model,
            turn_thinking_mode,
            effective_profile_agent_type,
        );
        let prompt_context = PromptContext::for_turn(
            turn_id.clone(),
            &turn_config,
            TurnPaths {
                workspace_root: snapshot.workspace_root.clone(),
                cwd: context_cwd.clone(),
            },
            agent_profile,
            turn_mode,
        );
        let turn_context = prompt_context.turn_context.clone();
        let user_message = ConversationMessage::user_parts(input.to_vec());
        let prompt = user_message.content.clone();
        self.ensure_frozen_memory_context(snapshot).await?;
        let context_messages = self.context_manager.apply_context_updates(prompt_context);
        refresh_file_backed_contexts(
            &turn_config,
            &snapshot.workspace_root,
            &context_cwd,
            &prompt,
            &mut self.context_manager,
        );
        self.refresh_dynamic_memory_context(snapshot, &prompt)
            .await?;
        if !matches!(turn_mode, TurnMode::Plan) {
            if let Some(goal_runtime) = self.goal_runtime.as_ref() {
                if let Some(goal) = goal_runtime
                    .active_goal_snapshot(&snapshot.thread_id)
                    .await?
                {
                    let content = goal_runtime
                        .active_goal_snapshot_prompt_for_goal(&snapshot.thread_id, &goal)
                        .await?;
                    self.context_manager.upsert_ephemeral_internal_context(
                        "goal_snapshot",
                        ConversationMessage::injected_user_context("goal_snapshot", content),
                    );
                } else {
                    self.context_manager
                        .clear_ephemeral_internal_context("goal_snapshot");
                }
                let effect = goal_runtime
                    .apply(GoalRuntimeEvent::TurnStarted {
                        thread_id: &snapshot.thread_id,
                        turn_id,
                        trigger: GoalTurnTrigger::User,
                        token_usage: current_token_usage(&self.context_manager),
                    })
                    .await?;
                apply_goal_effect(
                    Some(&self.agent),
                    &mut self.recorder,
                    &self.rollout_store,
                    &mut self.context_manager,
                    snapshot,
                    Some(turn_id),
                    effect,
                )
                .await?;
            }
        }
        let mailbox_messages = self
            .context_manager
            .record_inter_agent_communications(self.inbox.drain().await);
        self.context_manager.record_items([user_message.clone()]);
        self.context_manager.sync_snapshot(snapshot);
        let mut rollout_items =
            Vec::with_capacity(context_messages.len() + mailbox_messages.len() + 2);
        rollout_items.push(RolloutItem::TurnContext(turn_context));
        rollout_items.extend(
            context_messages
                .into_iter()
                .map(|message| RolloutItem::response_item_for_turn(turn_id.clone(), message)),
        );
        rollout_items.extend(
            mailbox_messages
                .into_iter()
                .map(|message| RolloutItem::response_item_for_turn(turn_id.clone(), message)),
        );
        rollout_items.push(RolloutItem::response_item_for_turn(
            turn_id.clone(),
            user_message,
        ));
        self.rollout_store.append_items_blocking(&rollout_items)?;
        self.publish_snapshot(snapshot)?;
        Ok(())
    }

    pub(super) async fn ensure_frozen_memory_context(
        &mut self,
        snapshot: &ThreadSnapshot,
    ) -> Result<()> {
        if self.frozen_memory_initialized {
            return Ok(());
        }
        self.frozen_memory_initialized = true;
        self.context_manager
            .clear_stable_internal_context("00_frozen_memory");

        if !self.base_config.memory_enabled || !self.base_config.memory_frozen_inject_enabled {
            return Ok(());
        }
        let Some(memory_runtime) = self.memory_runtime.as_ref().cloned() else {
            return Ok(());
        };

        let project_id = match self.resolve_memory_project_id(snapshot).await {
            Ok(project_id) => project_id,
            Err(error) => {
                tracing::warn!(
                    thread_id = snapshot.thread_id.as_str(),
                    workspace_root = %snapshot.workspace_root.display(),
                    error = ?error,
                    "failed to resolve project memory for frozen context; continuing without memory"
                );
                return Ok(());
            }
        };
        let hits = match memory_runtime
            .db()
            .frozen_memory_for_scope(
                project_id.as_deref(),
                None,
                self.base_config.memory_frozen_context_max_chars,
            )
            .await
        {
            Ok(hits) => hits,
            Err(error) => {
                tracing::warn!(
                    thread_id = snapshot.thread_id.as_str(),
                    project_id = project_id.as_deref(),
                    error = ?error,
                    "failed to load frozen memory context; continuing without memory"
                );
                return Ok(());
            }
        };
        let content =
            format_frozen_memory_block(&hits, self.base_config.memory_frozen_context_max_chars);
        if !content.is_empty() {
            self.context_manager.upsert_stable_internal_context(
                "00_frozen_memory",
                ConversationMessage::injected_user_context("00_frozen_memory", content),
            );
        }

        Ok(())
    }

    pub(super) async fn refresh_dynamic_memory_context(
        &mut self,
        snapshot: &ThreadSnapshot,
        prompt: &str,
    ) -> Result<()> {
        if !self.base_config.memory_enabled
            || !self.base_config.memory_auto_inject_enabled
            || !should_auto_recall(prompt)
        {
            self.context_manager
                .clear_ephemeral_internal_context("00_memory_recall");
            return Ok(());
        }
        let Some(memory_runtime) = self.memory_runtime.as_ref().cloned() else {
            self.context_manager
                .clear_ephemeral_internal_context("00_memory_recall");
            return Ok(());
        };

        self.context_manager
            .clear_ephemeral_internal_context("00_memory_recall");
        let project_id = match self.resolve_memory_project_id(snapshot).await {
            Ok(project_id) => project_id,
            Err(error) => {
                tracing::warn!(
                    thread_id = snapshot.thread_id.as_str(),
                    workspace_root = %snapshot.workspace_root.display(),
                    error = ?error,
                    "failed to resolve project memory for auto recall; continuing without memory"
                );
                return Ok(());
            }
        };
        let hits = match memory_runtime
            .db()
            .search_memory(MemorySearchQuery {
                scope: MemoryScope::Project,
                project_id,
                thread_id: Some(snapshot.thread_id.clone()),
                query: prompt.to_string(),
                mode: MemoryRecallMode::AutoInject,
                limit: self.base_config.memory_auto_max_hits,
                include_entries: true,
            })
            .await
        {
            Ok(hits) => hits,
            Err(error) => {
                tracing::warn!(
                    thread_id = snapshot.thread_id.as_str(),
                    error = ?error,
                    "failed to load auto memory context; continuing without memory"
                );
                return Ok(());
            }
        };
        let content =
            format_auto_memory_context(&hits, self.base_config.memory_auto_context_max_chars);
        if !content.is_empty() {
            self.context_manager.upsert_ephemeral_internal_context(
                "00_memory_recall",
                ConversationMessage::injected_user_context("00_memory_recall", content),
            );
        }

        Ok(())
    }

    async fn resolve_memory_project_id(
        &mut self,
        snapshot: &ThreadSnapshot,
    ) -> Result<Option<String>> {
        let Some(memory_runtime) = self.memory_runtime.as_ref().cloned() else {
            self.project_id = None;
            return Ok(None);
        };
        let project_id = memory_runtime
            .resolve_project_id_cached(&snapshot.workspace_root)
            .await?;
        self.project_id = project_id.clone();
        Ok(project_id)
    }
}
