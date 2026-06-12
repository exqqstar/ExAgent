use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::{LiveEventSink, RuntimeInterrupt, ThreadEventRecorder, ThreadInbox, ThreadSession};
use crate::agent::Agent;
use crate::config::{AgentConfig, ThinkingMode};
use crate::events::RuntimeEventKind;
use crate::llm::{LlmRequestOptions, LlmStreamEvent, LlmStreamSink};
use crate::model::multimodal;
use crate::resolved::ResolvedModelConfig;
use crate::runtime::agent_profile::{profile_for_type, AgentType};
use crate::runtime::context::{
    AgentRuntimeProfileContext, ContextManager, PromptContext, TurnPaths,
};
use crate::runtime::goal::runtime::{
    GoalRuntime, GoalRuntimeEffect, GoalRuntimeEvent, GoalTurnTrigger,
};
use crate::runtime::project_docs::{load_project_docs, ProjectDocConfig};
use crate::runtime::skills::{
    load_skill_body, load_skills, render_available_skills, resolve_explicit_skill_mentions,
    SkillConfig,
};
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::runtime::tool_orchestrator::ToolExecutionOutcome;
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ApprovalId, ApprovalStatus, CompactionSummary, ThreadSnapshot};
use crate::state::rollout::{CompactedItem, RolloutItem};
use crate::tools::ToolSpec;
use crate::types::{
    AssistantTurn, ConversationMessage, InputModality, MessageRole, ToolCall, TurnId, UserInput,
};
use tokio::sync::oneshot;

impl ThreadSession {
    #[cfg(test)]
    pub(crate) async fn handle_user_input(
        &mut self,
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.handle_user_input_parts(
            turn_id,
            vec![UserInput::Text { text: prompt }],
            turn_context,
            interrupt,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn handle_user_input_parts(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.handle_user_input_parts_with_start_ack(turn_id, input, turn_context, interrupt, None)
            .await
    }

    pub(crate) async fn handle_user_input_parts_with_start_ack(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
        start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    ) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self
            .handle_user_input_inner(turn_id, input, turn_context, interrupt, start_tx)
            .await;
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    pub(crate) async fn handle_goal_continuation(
        &mut self,
        turn_id: TurnId,
        goal_id: String,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self
            .handle_goal_continuation_inner(turn_id, goal_id, interrupt)
            .await;
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    pub(crate) async fn handle_manual_compaction(&mut self) -> Result<ThreadOpResult> {
        self.set_status(ThreadRuntimeStatus::Running);
        let result = self.handle_manual_compaction_inner().await;
        if let Err(error) = &result {
            let _ = self.record_runtime_error_without_turn(error.to_string());
        }
        self.set_status(ThreadRuntimeStatus::Idle);
        result
    }

    async fn handle_manual_compaction_inner(&mut self) -> Result<ThreadOpResult> {
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

    async fn handle_goal_continuation_inner(
        &mut self,
        turn_id: TurnId,
        goal_id: String,
        interrupt: Option<RuntimeInterrupt>,
    ) -> Result<ThreadOpResult> {
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.record_turn_started(&snapshot, &turn_id, None)?;
        let Some(goal_runtime) = self.goal_runtime.as_ref().cloned() else {
            return Err(anyhow!("goal continuation requested without goal runtime"));
        };
        let Some(goal) = goal_runtime.get_goal(&snapshot.thread_id).await? else {
            return Err(anyhow!("goal continuation requested without active goal"));
        };
        if goal.goal_id != goal_id
            || goal.status != crate::app_server::protocol::ThreadGoalStatus::Active
        {
            return Err(anyhow!("goal continuation candidate is no longer active"));
        }
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::ThreadGoalContinuationStarted {
                goal_id: goal.goal_id.clone(),
            },
        )?;
        let context_message = self.context_manager.record_persistent_internal_context(
            "goal_continuation",
            crate::runtime::goal::prompts::continuation_prompt(&goal),
        );
        self.context_manager.sync_snapshot(&mut snapshot);
        self.rollout_store
            .append_items_blocking(&[RolloutItem::response_item_for_turn(
                turn_id.clone(),
                context_message,
            )])?;
        let effect = goal_runtime
            .apply(GoalRuntimeEvent::TurnStarted {
                thread_id: &snapshot.thread_id,
                turn_id: &turn_id,
                trigger: GoalTurnTrigger::GoalContinuation,
                token_usage: current_token_usage(&self.context_manager),
            })
            .await?;
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            Some(&turn_id),
            effect,
        )
        .await?;
        self.recorder.record(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::ThreadGoalTurnStarted {
                goal_id: goal.goal_id.clone(),
            },
        )?;

        let final_turn = if let Some(mut interrupt) = interrupt {
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = run_session_turn(
                    agent,
                    recorder,
                    rollout_store,
                    context_manager,
                    goal_runtime.as_deref(),
                    &mut snapshot,
                    runtime_turn_id,
                    None,
                    None,
                    TurnThinkingModeOverride::Inherit,
                    TurnMode::Default,
                    inbox,
                ) => result?,
                _ = &mut interrupt.interrupt_rx => {
                    self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                    return Err(ThreadRuntimeError::TurnInterrupted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                    }.into());
                }
            }
        } else {
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            run_session_turn(
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime.as_deref(),
                &mut snapshot,
                turn_id.clone(),
                None,
                None,
                TurnThinkingModeOverride::Inherit,
                TurnMode::Default,
                inbox,
            )
            .await?
        };

        let effect = goal_runtime
            .apply(GoalRuntimeEvent::TurnFinished {
                thread_id: &snapshot.thread_id,
                turn_id: &turn_id,
                turn_completed: true,
                token_usage: current_token_usage(&self.context_manager),
                assistant_had_activity: assistant_turn_has_activity(&final_turn),
            })
            .await?;
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            Some(&turn_id),
            effect,
        )
        .await?;
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

    async fn handle_user_input_inner(
        &mut self,
        turn_id: TurnId,
        input: Vec<UserInput>,
        turn_context: Option<ThreadTurnContext>,
        interrupt: Option<RuntimeInterrupt>,
        mut start_tx: Option<oneshot::Sender<Result<TurnId>>>,
    ) -> Result<ThreadOpResult> {
        let turn_cwd = turn_context
            .as_ref()
            .and_then(|context| context.cwd.clone());
        let turn_resolved_model = turn_context
            .as_ref()
            .and_then(|context| context.resolved_model.clone());
        let turn_thinking_mode = TurnThinkingModeOverride::from_turn_context(turn_context.as_ref());
        let turn_mode = turn_context
            .as_ref()
            .map(|context| context.turn_mode)
            .unwrap_or(TurnMode::Default);
        let effective_model = turn_resolved_model
            .as_ref()
            .unwrap_or(&self.base_config.model);
        if let Err(err) = multimodal::validate_turn_input_modalities(
            &input,
            &effective_model.capabilities.input_modalities,
        ) {
            send_start_ack_error(start_tx.take(), &err);
            return Err(err);
        }
        if let Err(err) = multimodal::validate_local_image_inputs(&input) {
            send_start_ack_error(start_tx.take(), &err);
            return Err(err);
        }

        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        self.record_turn_started(&snapshot, &turn_id, start_tx)?;
        if let Err(err) = self
            .ensure_agent_for_turn_model(turn_resolved_model.as_ref())
            .await
        {
            self.record_runtime_error(&snapshot, &turn_id, &err)?;
            return Err(err);
        }

        let final_turn = if let Some(mut interrupt) = interrupt {
            let interrupted_during_compaction = tokio::select! {
                result = self.compact_before_turn_if_needed(&turn_id, &mut snapshot) => {
                    if let Err(err) = result {
                        self.record_runtime_error(&snapshot, &turn_id, &err)?;
                        return Err(err);
                    }
                    false
                }
                _ = &mut interrupt.interrupt_rx => true,
            };
            if interrupted_during_compaction {
                self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                return Err(ThreadRuntimeError::TurnInterrupted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                }
                .into());
            }

            let runtime_turn_cwd = turn_cwd.clone();
            if let Err(err) = self
                .record_user_turn_start(
                    &turn_id,
                    &input,
                    turn_cwd,
                    turn_resolved_model.as_ref(),
                    turn_thinking_mode,
                    turn_mode,
                    &mut snapshot,
                )
                .await
            {
                self.record_runtime_error(&snapshot, &turn_id, &err)?;
                return Err(err);
            }
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = run_session_turn(agent, recorder, rollout_store, context_manager, goal_runtime.as_deref(), &mut snapshot, runtime_turn_id, runtime_turn_cwd, turn_resolved_model.as_ref(), turn_thinking_mode, turn_mode, inbox) => {
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
                    self.record_turn_interrupted(&mut snapshot, &turn_id, &interrupt.interrupted)?;
                    return Err(ThreadRuntimeError::TurnInterrupted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                    }.into());
                }
            }
        } else {
            if let Err(err) = self
                .compact_before_turn_if_needed(&turn_id, &mut snapshot)
                .await
            {
                self.record_runtime_error(&snapshot, &turn_id, &err)?;
                return Err(err);
            }
            let runtime_turn_cwd = turn_cwd.clone();
            if let Err(err) = self
                .record_user_turn_start(
                    &turn_id,
                    &input,
                    turn_cwd,
                    turn_resolved_model.as_ref(),
                    turn_thinking_mode,
                    turn_mode,
                    &mut snapshot,
                )
                .await
            {
                self.record_runtime_error(&snapshot, &turn_id, &err)?;
                return Err(err);
            }
            let inbox = self.inbox.clone();
            let Self {
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime,
                ..
            } = self;
            match run_session_turn(
                agent,
                recorder,
                rollout_store,
                context_manager,
                goal_runtime.as_deref(),
                &mut snapshot,
                turn_id.clone(),
                runtime_turn_cwd,
                turn_resolved_model.as_ref(),
                turn_thinking_mode,
                turn_mode,
                inbox,
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

        if !matches!(turn_mode, TurnMode::Plan) {
            if let Some(goal_runtime) = self.goal_runtime.as_ref() {
                let effect = goal_runtime
                    .apply(GoalRuntimeEvent::TurnFinished {
                        thread_id: &snapshot.thread_id,
                        turn_id: &turn_id,
                        turn_completed: true,
                        token_usage: current_token_usage(&self.context_manager),
                        assistant_had_activity: assistant_turn_has_activity(&final_turn),
                    })
                    .await?;
                apply_goal_effect(
                    Some(&self.agent),
                    &mut self.recorder,
                    &self.rollout_store,
                    &mut self.context_manager,
                    &mut snapshot,
                    Some(&turn_id),
                    effect,
                )
                .await?;
                record_goal_turn_started_marker(
                    goal_runtime,
                    &mut self.recorder,
                    &snapshot,
                    &turn_id,
                )
                .await?;
            }
        }

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

    async fn ensure_agent_for_turn_model(
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
        self.agent = (self.agent_factory)(config)?
            .with_subagent_control(self.subagent_control.clone())
            .with_goal_api(goal_api);
        Ok(())
    }

    fn record_turn_started(
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

    fn record_runtime_error(
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

    pub(crate) async fn handle_goal_runtime_effect(
        &mut self,
        effect: GoalRuntimeEffect,
    ) -> Result<bool> {
        let should_check_goal_continuation = !matches!(effect, GoalRuntimeEffect::None);
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        apply_goal_effect(
            Some(&self.agent),
            &mut self.recorder,
            &self.rollout_store,
            &mut self.context_manager,
            &mut snapshot,
            None,
            effect,
        )
        .await?;
        Ok(should_check_goal_continuation)
    }

    fn publish_snapshot(&self, snapshot: &ThreadSnapshot) -> Result<()> {
        let mut state = self
            .live_state
            .write()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?;
        state.snapshot = snapshot.clone();
        Ok(())
    }

    async fn record_user_turn_start(
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
        let context_messages = self.context_manager.apply_context_updates(prompt_context);
        let user_message = ConversationMessage::user_parts(input.to_vec());
        let prompt = user_message.content.clone();
        refresh_file_backed_contexts(
            &turn_config,
            &snapshot.workspace_root,
            &context_cwd,
            &prompt,
            &mut self.context_manager,
        );
        if !matches!(turn_mode, TurnMode::Plan) {
            if let Some(goal_runtime) = self.goal_runtime.as_ref() {
                if let Some(goal) = goal_runtime
                    .active_goal_snapshot(&snapshot.thread_id)
                    .await?
                {
                    let content = crate::runtime::goal::prompts::active_goal_snapshot_prompt(&goal);
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

    fn record_turn_interrupted(
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
        let tool_runtime = self
            .agent
            .tool_runtime(
                snapshot.thread_id.clone(),
                turn_id.clone(),
                snapshot.workspace_root.clone(),
                cwd,
                Some(self.recorder.exec_output_event_sink()),
                crate::runtime::agent_profile::AgentToolPolicy::all(),
                None,
            )
            .await?;
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
            thought_signature: None,
        };
        let mut outcome = tool_runtime
            .execute_with_lifecycle(call, &mut self.recorder, &snapshot, &turn_id)
            .await?;
        outcome.attach_approval_note(&approval_id, note.clone());
        if !outcome.approval_matches(&approval_id, &status) {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: self.thread_id.clone(),
                reason: outcome.result.content,
            }
            .into());
        }
        record_approval_decision_outcome(&mut self.recorder, &mut snapshot, &turn_id, outcome)?;

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

fn send_start_ack_error(start_tx: Option<oneshot::Sender<Result<TurnId>>>, err: &anyhow::Error) {
    if let Some(start_tx) = start_tx {
        let _ = start_tx.send(Err(anyhow!(err.to_string())));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TurnThinkingModeOverride {
    #[default]
    Inherit,
    Set(ThinkingMode),
    ClearDefault,
}

impl TurnThinkingModeOverride {
    fn from_turn_context(turn_context: Option<&ThreadTurnContext>) -> Self {
        let Some(turn_context) = turn_context else {
            return Self::default();
        };
        if let Some(thinking_mode) = turn_context.thinking_mode {
            return Self::Set(thinking_mode);
        }
        if turn_context.clear_thinking_mode {
            return Self::ClearDefault;
        }
        Self::Inherit
    }

    fn effective(self, agent_default: Option<ThinkingMode>) -> Option<ThinkingMode> {
        match self {
            Self::Inherit => agent_default,
            Self::Set(thinking_mode) => Some(thinking_mode),
            Self::ClearDefault => None,
        }
    }
}

async fn run_session_turn(
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
            tool_specs,
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
                    tool_specs,
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
    tool_specs: &[ToolSpec],
) -> Vec<ConversationMessage> {
    let mut prompt = context_manager.for_prompt(input_modalities);
    let Some(guidance) = subagent_tool_guidance_message(tool_specs) else {
        return prompt;
    };
    let insert_index = prompt
        .iter()
        .rposition(|message| matches!(message.role, MessageRole::User) && !message.injected)
        .unwrap_or(prompt.len());
    prompt.insert(insert_index, guidance);
    prompt
}

fn subagent_tool_guidance_message(tool_specs: &[ToolSpec]) -> Option<ConversationMessage> {
    let has_tool = |name: &str| tool_specs.iter().any(|spec| spec.name == name);
    let available = [
        "spawn_agent",
        "list_agents",
        "send_message",
        "wait_agent",
        "followup_task",
        "close_agent",
    ]
    .into_iter()
    .filter(|name| has_tool(name))
    .collect::<Vec<_>>();

    if available.is_empty() {
        return None;
    }

    let mut content = format!(
        "Subagent collaboration tools are available in this turn: {}.",
        available.join(", ")
    );
    if has_tool("spawn_agent") {
        content.push_str(" Use spawn_agent to start a native subagent thread for a focused task.");
    }
    if has_tool("wait_agent") {
        content.push_str(
            " Use wait_agent when you need to wait for subagent mailbox activity or completion messages.",
        );
    }
    if has_tool("send_message") {
        content.push_str(
            " Use send_message for direct inter-agent communication within the current agent tree.",
        );
    }
    if has_tool("list_agents") {
        content.push_str(" Use list_agents to inspect the current agent tree.");
    }

    Some(ConversationMessage::injected_system(content))
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

async fn record_goal_turn_started_marker(
    goal_runtime: &GoalRuntime,
    recorder: &mut ThreadEventRecorder,
    snapshot: &ThreadSnapshot,
    turn_id: &TurnId,
) -> Result<()> {
    if let Some(goal_id) = goal_runtime
        .active_goal_id_for_turn(&snapshot.thread_id, turn_id)
        .await
    {
        recorder.record(
            snapshot,
            Some(turn_id),
            RuntimeEventKind::ThreadGoalTurnStarted { goal_id },
        )?;
    }
    Ok(())
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

async fn compact_after_context_window_error(
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

fn agent_profile_context_for_turn(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> Option<AgentRuntimeProfileContext> {
    let lineage = snapshot.lineage.as_ref();
    let agent_type = effective_profile_agent_type_for_turn(snapshot, turn_mode);
    let agent_role = lineage.and_then(|lineage| lineage.agent_role.clone());
    if agent_type.is_none() && agent_role.is_none() {
        return None;
    }

    let (instructions, response_guidance) = match agent_type {
        Some(agent_type) => {
            let profile = profile_for_type(Some(agent_type));
            (Some(profile.instructions), Some(profile.response_guidance))
        }
        None => (None, None),
    };
    Some(AgentRuntimeProfileContext {
        agent_type,
        agent_role,
        instructions,
        response_guidance,
    })
}

fn agent_tool_policy(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> crate::runtime::agent_profile::AgentToolPolicy {
    match effective_profile_agent_type_for_turn(snapshot, turn_mode) {
        Some(agent_type) => profile_for_type(Some(agent_type)).tool_policy,
        None => crate::runtime::agent_profile::AgentToolPolicy::all(),
    }
}

#[cfg(test)]
fn effective_agent_type_for_turn(snapshot: &ThreadSnapshot, turn_mode: TurnMode) -> AgentType {
    effective_profile_agent_type_for_turn(snapshot, turn_mode).unwrap_or(AgentType::Worker)
}

fn effective_profile_agent_type_for_turn(
    snapshot: &ThreadSnapshot,
    turn_mode: TurnMode,
) -> Option<AgentType> {
    if matches!(turn_mode, TurnMode::Plan) {
        Some(AgentType::Planner)
    } else {
        snapshot
            .lineage
            .as_ref()
            .and_then(|lineage| lineage.agent_type)
    }
}

fn config_for_turn(
    config: &AgentConfig,
    turn_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: TurnThinkingModeOverride,
    effective_agent_type: Option<AgentType>,
) -> AgentConfig {
    let mut config = config.clone();
    if let Some(model) = turn_model {
        config.model = model.clone();
    }
    let profile_default = effective_agent_type
        .and_then(|agent_type| profile_for_type(Some(agent_type)).default_thinking_mode);
    let inherited_thinking_mode = profile_default.or(config.thinking_mode);
    config.thinking_mode = turn_thinking_mode.effective(inherited_thinking_mode);
    config
}

fn refresh_file_backed_contexts(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    cwd: &std::path::Path,
    prompt: &str,
    context_manager: &mut ContextManager,
) {
    refresh_project_doc_context(config, workspace_root, cwd, context_manager);
    refresh_skill_context(config, workspace_root, prompt, context_manager);
}

fn refresh_project_doc_context(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    cwd: &std::path::Path,
    context_manager: &mut ContextManager,
) {
    let docs = load_project_docs(
        workspace_root,
        cwd,
        &ProjectDocConfig {
            enabled: config.project_docs_enabled,
            max_bytes: config.project_docs_max_bytes,
            ..ProjectDocConfig::default()
        },
    );
    if let Some(rendered) = docs.render() {
        context_manager.upsert_ephemeral_internal_context(
            "01_project_docs",
            ConversationMessage::injected_user_context("01_project_docs", rendered),
        );
    } else {
        context_manager.clear_ephemeral_internal_context("01_project_docs");
    }
}

const AVAILABLE_SKILLS_INSTRUCTIONS: &str = "## Available skills\n\
A skill is a reusable procedural guide stored in a SKILL.md file. Each entry below lists its name, scope, description, and file path.\n\n\
### How to use skills\n\
- Trigger: if the user names a skill (with `$skill-name` or plain text) OR the task clearly matches a skill's description below, use that skill for this turn. If several match, use the minimal set that covers the request.\n\
- Loading: when you decide to use a skill, open its SKILL.md at the listed path with your file-reading tool and follow it. Listed paths may be outside the workspace but are readable when they are under a configured skill root. An explicitly invoked skill's body may already be included below; if so, use it directly. If reading a listed path fails, say so briefly and continue with the best alternative. Read only what you need.\n\
- Scope: do not carry skills across turns unless they are mentioned again.\n\
- Fallback: if a skill cannot be applied cleanly (missing files, unclear instructions), say so briefly and continue with the best alternative.\n\n\
### Skills\n";

fn refresh_skill_context(
    config: &AgentConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    context_manager: &mut ContextManager,
) {
    context_manager.clear_ephemeral_internal_context_prefix("03_skill:");
    if !config.skills_enabled {
        context_manager.clear_ephemeral_internal_context("02_available_skills");
        return;
    }

    let skill_config = SkillConfig {
        enabled: config.skills_enabled,
        max_metadata_chars: config.skills_metadata_max_chars,
    };
    let catalog = load_skills(workspace_root, &config.skills_user_roots, &skill_config);
    let rendered = render_available_skills(&catalog, config.skills_metadata_max_chars);
    if rendered.text.trim().is_empty() {
        context_manager.clear_ephemeral_internal_context("02_available_skills");
    } else {
        let mut content = String::from(AVAILABLE_SKILLS_INSTRUCTIONS);
        content.push_str(&rendered.text);
        if rendered.omitted > 0 {
            content.push_str(&format!(
                "\n{} additional skill(s) were omitted to fit the skills context budget.\n",
                rendered.omitted
            ));
        }
        if rendered.descriptions_shortened {
            content.push_str(
                "\nSome skill descriptions were shortened to fit the skills context budget; open the SKILL.md for the full text.\n",
            );
        }
        if rendered.truncated && rendered.omitted == 0 && !rendered.descriptions_shortened {
            content.push_str(
                "\nSome available-skills context was truncated to fit the skills context budget.\n",
            );
        }
        context_manager.upsert_ephemeral_internal_context(
            "02_available_skills",
            ConversationMessage::injected_user_context("02_available_skills", content),
        );
    }

    for skill in resolve_explicit_skill_mentions(prompt, &catalog) {
        let source = format!("03_skill:{}", skill.name);
        let content = match load_skill_body(&skill) {
            Ok(body) => format!(
                "# Skill: {}\n\nSource: {}\n\n{}",
                skill.name,
                skill.path.display(),
                body
            ),
            Err(err) => format!(
                "# Skill: {}\n\nSource: {}\n\nFailed to load skill body: {}",
                skill.name,
                skill.path.display(),
                err
            ),
        };
        context_manager.upsert_ephemeral_internal_context(
            source.clone(),
            ConversationMessage::injected_user_context(source, content),
        );
    }
}

fn record_compaction_checkpoint(
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

fn record_token_count_event(
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

fn current_token_usage(context_manager: &ContextManager) -> crate::types::TokenUsage {
    context_manager
        .token_info()
        .map(|info| info.total_token_usage)
        .unwrap_or_default()
}

fn assistant_turn_has_activity(turn: &AssistantTurn) -> bool {
    turn.text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
        || !turn.reasoning.is_empty()
        || !turn.tool_calls.is_empty()
}

async fn apply_goal_effect(
    agent: Option<&Agent>,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut ThreadSnapshot,
    turn_id: Option<&TurnId>,
    effect: GoalRuntimeEffect,
) -> Result<()> {
    match effect {
        GoalRuntimeEffect::None | GoalRuntimeEffect::ScheduleContinuation => Ok(()),
        GoalRuntimeEffect::EmitUpdated(goal) => {
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndGoalReport { goal, report } => {
            let report = finalize_goal_report(agent, rollout_store, report).await;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalReport { report },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitCleared(thread_id) => {
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalCleared { thread_id },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndInjectPersistentContext {
            goal,
            source,
            content,
        } => {
            let message = context_manager.record_persistent_internal_context(source, content);
            context_manager.sync_snapshot(snapshot);
            if let Some(turn_id) = turn_id {
                rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
                    turn_id.clone(),
                    message,
                )])?;
            }
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedInjectContextAndGoalReport {
            goal,
            source,
            content,
            report,
        } => {
            let message = context_manager.record_persistent_internal_context(source, content);
            context_manager.sync_snapshot(snapshot);
            if let Some(turn_id) = turn_id {
                rollout_store.append_items_blocking(&[RolloutItem::response_item_for_turn(
                    turn_id.clone(),
                    message,
                )])?;
            }
            let report = finalize_goal_report(agent, rollout_store, report).await;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalReport { report },
            )?;
            Ok(())
        }
        GoalRuntimeEffect::EmitUpdatedAndContinuationSuppressed { goal, reason } => {
            let goal_id = goal.goal_id.clone();
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalUpdated { goal },
            )?;
            recorder.record(
                snapshot,
                turn_id,
                RuntimeEventKind::ThreadGoalContinuationSuppressed { goal_id, reason },
            )?;
            Ok(())
        }
    }
}

async fn finalize_goal_report(
    agent: Option<&Agent>,
    rollout_store: &crate::state::rollout::RolloutStore,
    mut report: crate::app_server::protocol::ThreadGoalReport,
) -> crate::app_server::protocol::ThreadGoalReport {
    let rollout_items =
        crate::state::rollout::RolloutStore::read_items_blocking(rollout_store.path())
            .unwrap_or_default();
    let events = crate::state::rollout::events_from_rollout_items(&rollout_items);
    // This rollout is scoped to the current thread. RuntimeOverlay keeps only
    // active unresolved approvals, so resolved and interrupted approvals are not counted.
    report.pending_approvals_count =
        crate::runtime::thread_session::RuntimeOverlay::from_events(&events)
            .pending_approvals
            .len();
    if let Some(agent) = agent {
        if let Ok(summary) = sample_goal_report_summary(agent, &report).await {
            report.summary = summary;
            return report;
        }
    }
    if report.summary.trim().is_empty() {
        report.summary = fallback_goal_report_summary(&report);
    }
    report
}

async fn sample_goal_report_summary(
    agent: &Agent,
    report: &crate::app_server::protocol::ThreadGoalReport,
) -> Result<String> {
    let prompt = vec![ConversationMessage::user(
        crate::runtime::goal::prompts::goal_report_summary_prompt(report),
    )];
    let completion = agent
        .sample_assistant_turn(
            &prompt,
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: agent.config().thinking_mode,
                reasoning_capabilities: None,
            },
        )
        .await?;
    let summary = completion.turn.text.unwrap_or_default().trim().to_string();
    if summary.is_empty() {
        return Err(anyhow!("empty goal report summary"));
    }
    Ok(summary)
}

fn changed_files_for_goal_report(
    tool_name: &str,
    result: &crate::types::ToolResult,
) -> Vec<String> {
    if result.status != crate::types::ToolStatus::Success {
        return Vec::new();
    }
    let Some(meta) = result.meta.as_ref() else {
        return Vec::new();
    };
    let files = match tool_name {
        "apply_patch" => meta
            .get("changed_files")
            .and_then(serde_json::Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "write_file" => ["normalized_path", "requested_path", "path"]
            .iter()
            .find_map(|key| meta.get(*key).and_then(serde_json::Value::as_str))
            .map(|file| vec![file.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut deduped = Vec::new();
    for file in files {
        if !deduped.iter().any(|existing| existing == &file) {
            deduped.push(file);
        }
    }
    deduped
}

fn fallback_goal_report_summary(report: &crate::app_server::protocol::ThreadGoalReport) -> String {
    format!(
        "Goal finished with status {} after {} turn(s).",
        goal_report_status_label(report.final_status),
        report.turns_run
    )
}

fn goal_report_status_label(status: crate::app_server::protocol::ThreadGoalStatus) -> &'static str {
    match status {
        crate::app_server::protocol::ThreadGoalStatus::Active => "active",
        crate::app_server::protocol::ThreadGoalStatus::Paused => "paused",
        crate::app_server::protocol::ThreadGoalStatus::Blocked => "blocked",
        crate::app_server::protocol::ThreadGoalStatus::UsageLimited => "usage_limited",
        crate::app_server::protocol::ThreadGoalStatus::BudgetLimited => "budget_limited",
        crate::app_server::protocol::ThreadGoalStatus::Complete => "complete",
    }
}

fn record_assistant_turn(
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

fn record_tool_outcome(
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

fn record_approval_decision_outcome(
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

#[cfg(test)]
mod tests {
    use super::{
        agent_profile_context_for_turn, changed_files_for_goal_report,
        effective_agent_type_for_turn,
    };

    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};

    use crate::agent::Agent;
    use crate::config::{AgentConfig, ThinkingMode};
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::llm::{LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink, MockLlm};
    use crate::registry::{ToolContext, ToolRegistry};
    use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
    use crate::runtime::agent_profile::AgentType;
    use crate::runtime::thread_runtime::{
        AgentFactory, ThreadOpResult, ThreadRuntimeStatus, ThreadTurnContext,
    };
    use crate::runtime::thread_session::{RuntimeInterrupt, ThreadSession, ThreadSessionOptions};
    use crate::runtime::turn_mode::TurnMode;
    use crate::session::{ThreadLineage, ThreadSnapshot};
    use crate::state::rollout::{RolloutItem, RolloutStore};
    use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
    use crate::types::{
        AssistantTurn, ConversationContentPart, ConversationMessage, ImageDetail, InputModality,
        LlmCompletion, MessageRole, ReasoningBlock, ThreadId, TokenUsage, ToolCall, ToolResult,
        ToolStatus, TurnId, UserInput,
    };

    #[test]
    fn goal_report_changed_files_extracts_apply_patch_metadata() {
        let result = ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "apply_patch".into(),
            status: ToolStatus::Success,
            content: "applied".into(),
            meta: Some(json!({
                "changed_files": ["src/runtime/goal/runtime.rs", "src/runtime/goal/runtime.rs"]
            })),
            parts: Vec::new(),
        };

        assert_eq!(
            changed_files_for_goal_report("apply_patch", &result),
            vec!["src/runtime/goal/runtime.rs".to_string()]
        );
    }

    #[test]
    fn goal_report_changed_files_extracts_write_file_path_metadata_only_for_write_tool() {
        let result = ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            status: ToolStatus::Success,
            content: "wrote".into(),
            meta: Some(json!({
                "normalized_path": "/workspace/src/new.rs",
                "requested_path": "src/new.rs",
                "path": "/workspace/src/new.rs"
            })),
            parts: Vec::new(),
        };

        assert_eq!(
            changed_files_for_goal_report("write_file", &result),
            vec!["/workspace/src/new.rs".to_string()]
        );
        assert!(changed_files_for_goal_report("read_file", &result).is_empty());
    }

    #[test]
    fn goal_report_changed_files_ignores_failed_tool_results() {
        let result = ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            status: ToolStatus::Error,
            content: "failed".into(),
            meta: Some(json!({ "normalized_path": "/workspace/src/new.rs" })),
            parts: Vec::new(),
        };

        assert!(changed_files_for_goal_report("write_file", &result).is_empty());
    }

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

    #[test]
    fn effective_agent_type_uses_planner_for_plan_mode_root_turn() {
        let dir = tempdir().unwrap();
        let snapshot = ThreadSnapshot::new_thread(
            ThreadId::new("thread_plan_effective_root"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        assert_eq!(
            effective_agent_type_for_turn(&snapshot, TurnMode::Plan),
            AgentType::Planner
        );
    }

    #[test]
    fn effective_agent_type_preserves_lineage_for_default_subagent_turn() {
        let dir = tempdir().unwrap();
        let mut snapshot = ThreadSnapshot::new_thread(
            ThreadId::new("thread_explorer_child"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        snapshot.lineage = Some(ThreadLineage {
            parent_thread_id: ThreadId::new("parent"),
            root_thread_id: ThreadId::new("parent"),
            depth: 1,
            agent_path: "explore-auth".into(),
            agent_type: Some(AgentType::Explorer),
            agent_role: None,
            agent_nickname: None,
            forked_from_id: None,
        });

        assert_eq!(
            effective_agent_type_for_turn(&snapshot, TurnMode::Default),
            AgentType::Explorer
        );
    }

    #[test]
    fn effective_agent_type_plan_mode_overrides_worker_lineage_for_one_turn() {
        let dir = tempdir().unwrap();
        let mut snapshot = ThreadSnapshot::new_thread(
            ThreadId::new("thread_worker_child"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        snapshot.lineage = Some(ThreadLineage {
            parent_thread_id: ThreadId::new("parent"),
            root_thread_id: ThreadId::new("parent"),
            depth: 1,
            agent_path: "worker-child".into(),
            agent_type: Some(AgentType::Worker),
            agent_role: Some("implementation worker".into()),
            agent_nickname: None,
            forked_from_id: None,
        });

        assert_eq!(
            effective_agent_type_for_turn(&snapshot, TurnMode::Plan),
            AgentType::Planner
        );
        assert_eq!(
            snapshot.lineage.as_ref().unwrap().agent_type,
            Some(AgentType::Worker)
        );
    }

    #[test]
    fn profile_context_none_for_default_root_turn() {
        let dir = tempdir().unwrap();
        let snapshot = ThreadSnapshot::new_thread(
            ThreadId::new("thread_default_root_profile"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        assert!(agent_profile_context_for_turn(&snapshot, TurnMode::Default).is_none());
    }

    #[test]
    fn profile_context_uses_planner_for_plan_mode_root_turn() {
        let dir = tempdir().unwrap();
        let snapshot = ThreadSnapshot::new_thread(
            ThreadId::new("thread_plan_root_profile"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );

        let context = agent_profile_context_for_turn(&snapshot, TurnMode::Plan)
            .expect("planner profile context");

        assert_eq!(context.agent_type, Some(AgentType::Planner));
        assert_eq!(context.agent_role, None);
        assert!(context
            .instructions
            .as_deref()
            .unwrap()
            .contains("planner agent"));
        assert!(context
            .response_guidance
            .as_deref()
            .unwrap()
            .contains("ordered implementation steps"));
    }

    struct RecordingLlm {
        turns: AsyncMutex<VecDeque<AssistantTurn>>,
        prompt_lens: Arc<Mutex<Vec<usize>>>,
        prompt_contents: Option<Arc<Mutex<Vec<Vec<String>>>>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ObservedModelLlmCall {
        built_model: String,
        options: LlmRequestOptions,
    }

    struct RecordingModelLlm {
        built_model: String,
        calls: Arc<Mutex<Vec<ObservedModelLlmCall>>>,
    }

    struct StreamingScriptedLlm {
        events: Vec<LlmStreamEvent>,
        completion: LlmCompletion,
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
    impl LlmClient for StreamingScriptedLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            Ok(self.completion.clone())
        }

        async fn stream(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _options: &LlmRequestOptions,
            sink: &mut dyn LlmStreamSink,
        ) -> Result<LlmCompletion> {
            for event in self.events.clone() {
                sink.event(event).await?;
            }
            sink.event(LlmStreamEvent::Completed(self.completion.clone()))
                .await?;
            Ok(self.completion.clone())
        }
    }

    #[async_trait]
    impl LlmClient for RecordingLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
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

    #[async_trait]
    impl LlmClient for RecordingModelLlm {
        async fn complete(
            &self,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            options: &LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.calls.lock().unwrap().push(ObservedModelLlmCall {
                built_model: self.built_model.clone(),
                options: options.clone(),
            });
            Ok(AssistantTurn {
                text: Some(format!("{} done", self.built_model)),
                tool_calls: vec![],
                reasoning: vec![],
            }
            .into_completion())
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
            _tools: &[ToolSpec],
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
            reasoning: vec![],
        }
        .into_completion()
    }

    struct MetadataTool;

    struct BlockingTool {
        started: Arc<Notify>,
    }

    #[async_trait]
    impl ToolHandler for MetadataTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "metadata_tool",
                "Return content with runtime metadata",
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::read_only()
        }

        async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            let call = invocation.call;
            ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: "model-visible summary".to_string(),
                meta: Some(serde_json::json!({
                    "stdout": "runtime stdout that must not enter prompt",
                    "stderr": "runtime stderr that must not enter prompt",
                    "exec_session_id": "exec_secret",
                })),
                parts: vec![ConversationContentPart::LocalImage {
                    path: PathBuf::from("/tmp/tool-result.png"),
                    detail: Some(ImageDetail::High),
                }],
            })
        }
    }

    #[async_trait]
    impl ToolHandler for BlockingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                "blocking_tool",
                "Block until the test interrupts the turn",
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )
        }

        fn capabilities(&self) -> ToolCapabilities {
            ToolCapabilities::read_only()
        }

        async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
            self.started.notify_one();
            std::future::pending::<()>().await;
            ToolOutcome::success(
                invocation.call.id,
                invocation.call.name,
                crate::tools::ToolModelOutput::text("unreachable"),
            )
        }
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
            reasoning: vec![],
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
    async fn image_input_on_text_only_model_is_rejected_before_turn_start_recording() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_rejects_image_text_only");
        let turn_id = TurnId::new("turn_image_rejected");
        let mut config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        config.model.capabilities.input_modalities = vec![InputModality::Text];
        write_rollout_meta(&config, &thread_id);
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("should not run".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
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

        let result = session
            .handle_user_input_parts(
                turn_id.clone(),
                vec![UserInput::LocalImage {
                    path: dir.path().join("screen.png"),
                    detail: Some(ImageDetail::High),
                }],
                None,
                None,
            )
            .await;
        let err = match result {
            Ok(_) => panic!("expected image input rejection"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("does not support image input"));
        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert!(live_view
            .events
            .iter()
            .all(|event| !matches!(event.kind, RuntimeEventKind::TurnStarted)));
        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().all(|item| !matches!(
            item,
            RolloutItem::ResponseItem(response)
                if response.turn_id == turn_id && response.message.role == MessageRole::User
        )));
    }

    #[tokio::test]
    async fn missing_local_image_input_is_rejected_before_turn_start_recording() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_rejects_missing_image");
        let turn_id = TurnId::new("turn_missing_image_rejected");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("should not run".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
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

        let result = session
            .handle_user_input_parts(
                turn_id.clone(),
                vec![UserInput::LocalImage {
                    path: dir.path().join("missing.png"),
                    detail: Some(ImageDetail::High),
                }],
                None,
                None,
            )
            .await;
        let err = match result {
            Ok(_) => panic!("expected missing image rejection"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("could not read image"));
        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert!(live_view
            .events
            .iter()
            .all(|event| !matches!(event.kind, RuntimeEventKind::TurnStarted)));
        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().all(|item| !matches!(
            item,
            RolloutItem::ResponseItem(response)
                if response.turn_id == turn_id && response.message.role == MessageRole::User
        )));
    }

    #[tokio::test]
    async fn thread_session_records_reasoning_event_before_assistant_turn() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_records_reasoning_event");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let final_turn = AssistantTurn {
            text: Some("final answer".into()),
            tool_calls: vec![],
            reasoning: vec![ReasoningBlock {
                text: "provider reasoning".to_string(),
                signature: None,
                redacted: false,
            }],
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

        session
            .handle_user_input(turn_id.clone(), "think first".into(), None, None)
            .await
            .expect("run turn");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let kinds = live_view
            .events
            .iter()
            .map(|event| &event.kind)
            .collect::<Vec<_>>();

        assert!(matches!(kinds[0], RuntimeEventKind::TurnStarted));
        assert!(matches!(
            kinds[1],
            RuntimeEventKind::Reasoning { summary, content }
                if summary.is_empty() && content == &vec!["provider reasoning".to_string()]
        ));
        assert!(matches!(kinds[2], RuntimeEventKind::AssistantTurn { .. }));
        assert!(matches!(kinds[3], RuntimeEventKind::TurnCompleted));

        let rollout_paths =
            crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
        let rollout_items =
            RolloutStore::read_items_blocking(&rollout_paths.rollout_path).expect("read rollout");
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(RuntimeEvent {
                kind: RuntimeEventKind::Reasoning { content, .. },
                ..
            }) if content == &vec!["provider reasoning".to_string()]
        )));
    }

    #[tokio::test]
    async fn thread_session_streams_reasoning_and_assistant_deltas() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_thread_session_streaming_reasoning");
        let turn_id = TurnId::new("turn_streaming_reasoning");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let completion = LlmCompletion {
            turn: AssistantTurn {
                text: Some("hello world".to_string()),
                tool_calls: vec![],
                reasoning: vec![ReasoningBlock {
                    text: "think first".to_string(),
                    signature: None,
                    redacted: false,
                }],
            },
            token_usage: None,
        };
        let agent_factory: AgentFactory = Arc::new({
            let completion = completion.clone();
            move |config| {
                Ok(Agent::new(
                    config,
                    Box::new(StreamingScriptedLlm {
                        events: vec![
                            LlmStreamEvent::ReasoningDelta("think ".to_string()),
                            LlmStreamEvent::ReasoningDelta("first".to_string()),
                            LlmStreamEvent::AssistantTextDelta("hello ".to_string()),
                            LlmStreamEvent::AssistantTextDelta("world".to_string()),
                        ],
                        completion: completion.clone(),
                    }),
                    ToolRegistry::new(),
                ))
            }
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id.clone(), "stream please".into(), None, None)
            .await
            .expect("run turn");

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let kinds = live_view
            .events
            .iter()
            .map(|event| &event.kind)
            .collect::<Vec<_>>();

        assert!(matches!(kinds[0], RuntimeEventKind::TurnStarted));
        assert!(matches!(
            kinds[1],
            RuntimeEventKind::ReasoningDelta { delta } if delta == "think "
        ));
        assert!(matches!(
            kinds[2],
            RuntimeEventKind::ReasoningDelta { delta } if delta == "first"
        ));
        assert!(matches!(
            kinds[3],
            RuntimeEventKind::AssistantTextDelta { delta } if delta == "hello "
        ));
        assert!(matches!(
            kinds[4],
            RuntimeEventKind::AssistantTextDelta { delta } if delta == "world"
        ));
        assert!(matches!(kinds[5], RuntimeEventKind::Reasoning { .. }));
        assert!(matches!(kinds[6], RuntimeEventKind::AssistantTurn { .. }));
        assert!(matches!(kinds[7], RuntimeEventKind::TurnCompleted));

        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::ResponseItem(response)
                if response.turn_id == turn_id
                    && response.message.content == "hello world"
        )));
        assert!(!rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(RuntimeEvent {
                kind: RuntimeEventKind::ReasoningDelta { .. }
                    | RuntimeEventKind::AssistantTextDelta { .. },
                ..
            })
        )));
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
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("second turn complete".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
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
    async fn thread_session_restores_base_agent_after_turn_model_override() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_turn_model_override_restores_base");
        let base_model = ResolvedModelConfig::from_provider_profile(
            "openai",
            "gpt-4.1",
            None,
            ResolvedCredential::None,
            None,
        );
        let override_model = ResolvedModelConfig::from_provider_profile(
            "openai",
            "gpt-5",
            None,
            ResolvedCredential::None,
            None,
        );
        let config = AgentConfig {
            model: base_model.clone(),
            thinking_mode: Some(ThinkingMode::Low),
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);

        let built_models = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let built_models_for_factory = built_models.clone();
        let calls_for_factory = calls.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let built_model = config.model.identity.model_id.clone();
            built_models_for_factory
                .lock()
                .unwrap()
                .push(built_model.clone());
            Ok(Agent::new(
                config,
                Box::new(RecordingModelLlm {
                    built_model,
                    calls: calls_for_factory.clone(),
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

        session
            .handle_user_input(
                TurnId::new("turn_1"),
                "run with override model".into(),
                Some(ThreadTurnContext {
                    cwd: None,
                    resolved_model: Some(override_model.clone()),
                    thinking_mode: None,
                    clear_thinking_mode: false,
                    turn_mode: crate::runtime::turn_mode::TurnMode::Default,
                }),
                None,
            )
            .await
            .expect("first turn");
        session
            .handle_user_input(
                TurnId::new("turn_2"),
                "run with base model".into(),
                None,
                None,
            )
            .await
            .expect("second turn");

        assert_eq!(
            built_models.lock().unwrap().clone(),
            vec![
                "gpt-4.1".to_string(),
                "gpt-5".to_string(),
                "gpt-4.1".to_string()
            ]
        );

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].built_model, "gpt-5");
        assert_eq!(calls[0].options.model.as_deref(), Some("gpt-5"));
        assert_eq!(
            calls[0].options.reasoning_capabilities.as_ref(),
            Some(&override_model.capabilities.reasoning)
        );
        assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::Low));

        assert_eq!(calls[1].built_model, "gpt-4.1");
        assert_eq!(calls[1].options.model, None);
        assert_eq!(calls[1].options.reasoning_capabilities, None);
        assert_eq!(calls[1].options.thinking_mode, Some(ThinkingMode::Low));
    }

    #[tokio::test]
    async fn plan_mode_uses_planner_default_thinking_mode_for_root_turn() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_plan_mode_uses_planner_thinking");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            thinking_mode: None,
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_factory = calls.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let built_model = config.model.identity.model_id.clone();
            Ok(Agent::new(
                config,
                Box::new(RecordingModelLlm {
                    built_model,
                    calls: calls_for_factory.clone(),
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

        session
            .handle_user_input(
                TurnId::new("turn_plan_default"),
                "plan with default thinking".into(),
                Some(ThreadTurnContext {
                    cwd: None,
                    resolved_model: None,
                    thinking_mode: None,
                    clear_thinking_mode: false,
                    turn_mode: TurnMode::Plan,
                }),
                None,
            )
            .await
            .expect("plan turn");

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::Medium));
    }

    #[tokio::test]
    async fn plan_mode_explicit_thinking_override_wins_over_planner_default() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_plan_mode_thinking_override");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            thinking_mode: None,
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_factory = calls.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let built_model = config.model.identity.model_id.clone();
            Ok(Agent::new(
                config,
                Box::new(RecordingModelLlm {
                    built_model,
                    calls: calls_for_factory.clone(),
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

        session
            .handle_user_input(
                TurnId::new("turn_plan_high"),
                "plan with high thinking".into(),
                Some(ThreadTurnContext {
                    cwd: None,
                    resolved_model: None,
                    thinking_mode: Some(ThinkingMode::High),
                    clear_thinking_mode: false,
                    turn_mode: TurnMode::Plan,
                }),
                None,
            )
            .await
            .expect("plan turn");

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].options.thinking_mode, Some(ThinkingMode::High));
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
                                thought_signature: None,
                            }],
                            reasoning: vec![],
                        },
                        AssistantTurn {
                            text: Some("done".into()),
                            tool_calls: vec![],
                            reasoning: vec![],
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
    async fn thread_session_records_model_only_tool_message_in_prompt_context() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_model_only_tool_message");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let prompt_contents_for_llm = prompt_contents.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let mut registry = ToolRegistry::new();
            registry.register(MetadataTool);
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::with_prompt_contents(
                    vec![
                        AssistantTurn {
                            text: None,
                            tool_calls: vec![ToolCall {
                                id: "call_metadata".into(),
                                name: "metadata_tool".into(),
                                arguments: serde_json::json!({}),
                                thought_signature: None,
                            }],
                            reasoning: vec![],
                        },
                        AssistantTurn {
                            text: Some("done".into()),
                            tool_calls: vec![],
                            reasoning: vec![],
                        },
                    ],
                    prompt_lens_for_llm.clone(),
                    prompt_contents_for_llm.clone(),
                )),
                registry,
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config,
            agent_factory,
        ))
        .expect("create thread session");

        session
            .handle_user_input(turn_id, "use metadata tool".into(), None, None)
            .await
            .expect("run turn");

        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 5]);
        let second_prompt = prompts.last().expect("second prompt after tool");
        assert!(second_prompt
            .iter()
            .any(|content| content == "model-visible summary"));
        assert!(!second_prompt
            .iter()
            .any(|content| content.contains("runtime stdout")));
        assert!(!second_prompt
            .iter()
            .any(|content| content.contains("\"meta\"")));
        assert!(!second_prompt
            .iter()
            .any(|content| content.contains("exec_secret")));

        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        let tool_message = live_view
            .snapshot
            .conversation
            .iter()
            .find(|message| message.role == MessageRole::Tool)
            .expect("tool message in snapshot conversation");
        assert_eq!(
            tool_message.parts,
            vec![ConversationContentPart::LocalImage {
                path: PathBuf::from("/tmp/tool-result.png"),
                detail: Some(ImageDetail::High),
            }]
        );
    }

    #[tokio::test]
    async fn thread_session_interrupt_records_model_visible_tool_result() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_interrupt_tool_context");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        let prompt_lens = Arc::new(Mutex::new(Vec::new()));
        let prompt_contents = Arc::new(Mutex::new(Vec::new()));
        let prompt_lens_for_llm = prompt_lens.clone();
        let prompt_contents_for_llm = prompt_contents.clone();
        let tool_started = Arc::new(Notify::new());
        let tool_started_for_registry = tool_started.clone();
        let agent_factory: AgentFactory = Arc::new(move |config| {
            let mut registry = ToolRegistry::new();
            registry.register(BlockingTool {
                started: tool_started_for_registry.clone(),
            });
            Ok(Agent::new(
                config,
                Box::new(RecordingLlm::with_prompt_contents(
                    vec![
                        AssistantTurn {
                            text: None,
                            tool_calls: vec![ToolCall {
                                id: "call_blocking".into(),
                                name: "blocking_tool".into(),
                                arguments: serde_json::json!({}),
                                thought_signature: None,
                            }],
                            reasoning: vec![],
                        },
                        AssistantTurn {
                            text: Some("after interrupt".into()),
                            tool_calls: vec![],
                            reasoning: vec![],
                        },
                    ],
                    prompt_lens_for_llm.clone(),
                    prompt_contents_for_llm.clone(),
                )),
                registry,
            ))
        });
        let mut session = ThreadSession::new(ThreadSessionOptions::new(
            thread_id.clone(),
            config.clone(),
            agent_factory,
        ))
        .expect("create thread session");
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupted = Arc::new(Notify::new());
        let interrupted_for_waiter = interrupted.clone();
        let tool_started_for_waiter = tool_started.clone();

        let run_turn = session.handle_user_input(
            turn_id.clone(),
            "call blocking tool".into(),
            None,
            Some(RuntimeInterrupt {
                interrupt_rx,
                interrupted,
            }),
        );
        let send_interrupt = async move {
            tool_started_for_waiter.notified().await;
            interrupt_tx.send(()).expect("send interrupt");
            interrupted_for_waiter.notified().await;
        };
        let (result, _) = tokio::join!(run_turn, send_interrupt);
        assert!(result.is_err());

        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::ResponseItem(response)
                if response.turn_id == turn_id
                    && response.message.tool_call_id.as_deref() == Some("call_blocking")
                    && response.message.content == "interrupted"
        )));

        session
            .handle_user_input(TurnId::new("turn_2"), "continue".into(), None, None)
            .await
            .expect("second turn after interrupted tool");

        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(*prompt_lens.lock().unwrap(), vec![3, 6]);
        let second_prompt = prompts.last().expect("second prompt after interrupt");
        assert!(second_prompt.iter().any(|content| content == "interrupted"));
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
                        thought_signature: None,
                    }],
                    reasoning: vec![],
                });
            }
            turns.push(AssistantTurn {
                text: Some("done after tools".into()),
                tool_calls: vec![],
                reasoning: vec![],
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
                        reasoning: vec![],
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
                            thought_signature: None,
                        }],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
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
        assert_eq!(replay.len(), 7, "expected tool lifecycle plus turn events");
        assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
        assert!(matches!(
            replay[1].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(
            replay[2].kind,
            RuntimeEventKind::ToolInvocationStarted { .. }
        ));
        assert!(matches!(
            replay[3].kind,
            RuntimeEventKind::ToolInvocationFailed { .. }
        ));
        assert!(matches!(
            replay[4].kind,
            RuntimeEventKind::ToolResult { .. }
        ));
        assert!(matches!(
            replay[5].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(replay[6].kind, RuntimeEventKind::TurnCompleted));

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
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
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
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
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
                            reasoning: vec![],
                        },
                        AssistantTurn {
                            text: Some("done".into()),
                            tool_calls: vec![],
                            reasoning: vec![],
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
        session.context_manager.upsert_ephemeral_internal_context(
            "01_project_docs",
            ConversationMessage::injected_user_context(
                "01_project_docs",
                "prompt-only project docs must not be compacted",
            ),
        );

        session
            .handle_user_input(turn_id.clone(), "new user".into(), None, None)
            .await
            .expect("run turn");

        let prompts = prompt_contents.lock().unwrap();
        assert_eq!(*prompt_lens.lock().unwrap(), vec![2, 4]);
        assert!(prompts[0].join("\n").contains("old user"));
        assert!(prompts[0].join("\n").contains("old assistant"));
        assert!(!prompts[0].join("\n").contains("new user"));
        assert!(!prompts[0]
            .join("\n")
            .contains("prompt-only project docs must not be compacted"));
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
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![
                    AssistantTurn {
                        text: Some("summary from compaction".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
                    },
                    AssistantTurn {
                        text: Some("done".into()),
                        tool_calls: vec![],
                        reasoning: vec![],
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
    async fn thread_session_manual_compaction_writes_checkpoint_without_user_turn() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_manual_compaction");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        append_rollout_items(
            &config,
            &thread_id,
            &[
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("manual summary".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
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

        let result = session
            .handle_manual_compaction()
            .await
            .expect("manual compaction should succeed");

        assert!(matches!(result, ThreadOpResult::Ack));
        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(
            live_view
                .snapshot
                .latest_compaction
                .as_ref()
                .map(|summary| summary.summary.as_str()),
            Some("manual summary")
        );
        assert!(live_view.events.iter().any(|event| matches!(
            &event.kind,
            RuntimeEventKind::CompactionWritten { summary }
                if event.turn_id.is_none() && summary.summary == "manual summary"
        )));
        assert!(live_view.events.iter().any(|event| matches!(
            event,
            RuntimeEvent {
                turn_id: None,
                kind: RuntimeEventKind::TokenCount { .. },
                ..
            }
        )));

        let rollout_items = read_rollout_items(&config, &thread_id);
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::Compacted(compacted) if compacted.message == "manual summary"
        )));
        assert!(rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(RuntimeEvent {
                turn_id: None,
                kind: RuntimeEventKind::TokenCount { .. },
                ..
            })
        )));
        assert!(!rollout_items.iter().any(|item| matches!(
            item,
            RolloutItem::ResponseItem(response) if response.turn_id.as_str() == "manual_compaction"
        )));
        assert!(!live_view.events.iter().any(|event| event
            .turn_id
            .as_ref()
            .is_some_and(|turn_id| { turn_id.as_str() == "manual_compaction" })));
    }

    #[tokio::test]
    async fn thread_session_manual_compaction_failure_records_unscoped_runtime_error() {
        let dir = tempdir().unwrap();
        let thread_id = ThreadId::new("session_manual_compaction_failure");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        write_rollout_meta(&config, &thread_id);
        append_rollout_items(
            &config,
            &thread_id,
            &[
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::user("old user"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_0"),
                    ConversationMessage::assistant(Some("old assistant".to_string()), vec![]),
                ),
            ],
        );
        let agent_factory: AgentFactory = Arc::new(move |config| {
            Ok(Agent::new(
                config,
                Box::new(MockLlm::new(vec![AssistantTurn {
                    text: Some("".into()),
                    tool_calls: vec![],
                    reasoning: vec![],
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

        let error = match session.handle_manual_compaction().await {
            Ok(_) => panic!("empty compaction summary should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("empty compaction summary"));
        let live_view =
            ThreadSession::live_view_from_state(thread_id.clone(), &session.live_state_handle());
        assert_eq!(live_view.status, ThreadRuntimeStatus::Idle);
        assert!(live_view.events.iter().any(|event| matches!(
            event,
            RuntimeEvent {
                turn_id: None,
                kind: RuntimeEventKind::RuntimeError { message },
                ..
            } if message.contains("empty compaction summary")
        )));
        assert!(read_rollout_items(&config, &thread_id)
            .iter()
            .any(|item| matches!(
                item,
                RolloutItem::EventMsg(RuntimeEvent {
                    turn_id: None,
                    kind: RuntimeEventKind::RuntimeError { message },
                    ..
                }) if message.contains("empty compaction summary")
            )));
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
                        reasoning: vec![],
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
                    reasoning: vec![],
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
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "prompt-only project docs must not be compacted",
        )
        .expect("write project docs");
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
        assert!(prompts[0]
            .join("\n")
            .contains("prompt-only project docs must not be compacted"));
        assert!(prompts[1].join("\n").contains("new user"));
        assert!(!prompts[1]
            .join("\n")
            .contains("prompt-only project docs must not be compacted"));
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
