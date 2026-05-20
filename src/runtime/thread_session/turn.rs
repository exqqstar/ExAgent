use std::path::PathBuf;

use anyhow::{anyhow, Result};

use super::{LiveEventSink, RuntimeInterrupt, ThreadEventRecorder, ThreadSession};
use crate::agent::Agent;
use crate::events::RuntimeEventKind;
use crate::runtime::context::{ContextManager, PromptContext, TurnPaths};
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::runtime::tool_call_runtime::{
    ApprovalUpdate, ExecSessionUpdate, ToolEffect, ToolExecutionOutcome,
};
use crate::session::{
    ApprovalStatus, ExecSessionRef, ExecSessionStatus, PendingApproval, SessionSnapshot,
};
use crate::state::rollout::RolloutItem;
use crate::types::{AssistantTurn, ConversationMessage, TurnId};

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
        let turn_cwd = turn_context.and_then(|context| context.cwd);

        // Apply model-visible runtime context before the user message so the
        // sampling prompt has stable background for this turn.
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        let context_cwd = turn_cwd.clone().unwrap_or_else(|| snapshot.cwd.clone());
        let prompt_context = PromptContext::for_turn(
            self.agent.config(),
            TurnPaths {
                workspace_root: snapshot.workspace_root.clone(),
                cwd: context_cwd,
            },
        );
        let turn_context = prompt_context.turn_context.clone();
        let context_messages = self.context_manager.apply_context_updates(prompt_context);
        let user_message = ConversationMessage::user(prompt);
        self.context_manager.record_items([user_message.clone()]);
        self.context_manager.sync_snapshot(&mut snapshot);
        let mut rollout_items = Vec::with_capacity(context_messages.len() + 2);
        rollout_items.push(RolloutItem::TurnContext(turn_context));
        rollout_items.extend(context_messages.into_iter().map(RolloutItem::ResponseItem));
        rollout_items.push(RolloutItem::ResponseItem(user_message));
        self.rollout_store.append_items_blocking(&rollout_items)?;
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::TurnStarted,
        )?;

        let Self {
            agent,
            recorder,
            rollout_store,
            context_manager,
            ..
        } = self;

        let final_turn = if let Some(interrupt) = interrupt {
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = run_session_turn(agent, recorder, rollout_store, context_manager, &mut snapshot, runtime_turn_id, turn_cwd) => {
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
                _ = interrupt.interrupt_rx => {
                    self.append_and_broadcast_snapshot(
                        &snapshot,
                        Some(&turn_id),
                        RuntimeEventKind::TurnInterrupted,
                    )?;
                    interrupt.interrupted.notify_one();
                    return Err(ThreadRuntimeError::TurnInterrupted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                    }.into());
                }
            }
        } else {
            match run_session_turn(
                agent,
                recorder,
                rollout_store,
                context_manager,
                &mut snapshot,
                turn_id.clone(),
                turn_cwd,
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
}

async fn run_session_turn(
    agent: &Agent,
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut SessionSnapshot,
    runtime_turn_id: TurnId,
    turn_cwd: Option<PathBuf>,
) -> Result<AssistantTurn> {
    snapshot.normalize_lineage();
    let cwd = turn_cwd.unwrap_or_else(|| snapshot.cwd.clone());
    let tool_runtime = agent.tool_runtime(
        snapshot.session_id.clone(),
        runtime_turn_id.clone(),
        snapshot.workspace_root.clone(),
        cwd,
    );

    for _ in 0..agent.max_turns() {
        let prompt = context_manager.for_prompt();
        let turn = agent
            .sample_assistant_turn(&prompt, &tool_runtime.schemas())
            .await?;
        record_assistant_turn(
            recorder,
            rollout_store,
            context_manager,
            snapshot,
            &runtime_turn_id,
            &turn,
        )?;

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

    Err(anyhow!(
        "Agent reached max turns ({}) without a final assistant turn",
        agent.max_turns()
    ))
}

fn record_assistant_turn(
    recorder: &mut ThreadEventRecorder,
    rollout_store: &crate::state::rollout::RolloutStore,
    context_manager: &mut ContextManager,
    snapshot: &mut SessionSnapshot,
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
    snapshot: &mut SessionSnapshot,
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
    snapshot: &mut SessionSnapshot,
    turn_id: &TurnId,
    effect: ToolEffect,
) -> Result<()> {
    match effect {
        ToolEffect::ExecSessionUpdate(update) => {
            apply_exec_session_update(snapshot, update);
            Ok(())
        }
        ToolEffect::ApprovalUpdate(update) => {
            apply_approval_update(recorder, snapshot, turn_id, update)
        }
    }
}

fn apply_exec_session_update(snapshot: &mut SessionSnapshot, update: ExecSessionUpdate) {
    let exec_session_id = match &update {
        ExecSessionUpdate::Running {
            exec_session_id, ..
        }
        | ExecSessionUpdate::NotRunning { exec_session_id } => exec_session_id.clone(),
    };
    snapshot
        .open_exec_sessions
        .retain(|entry| entry.exec_session_id != exec_session_id);

    if let ExecSessionUpdate::Running {
        exec_session_id,
        command,
        cwd,
    } = update
    {
        snapshot.open_exec_sessions.push(ExecSessionRef {
            exec_session_id,
            command,
            cwd,
            status: ExecSessionStatus::Running,
        });
    }
}

fn apply_approval_update(
    recorder: &mut ThreadEventRecorder,
    snapshot: &mut SessionSnapshot,
    turn_id: &TurnId,
    update: ApprovalUpdate,
) -> Result<()> {
    let approval_id = match &update {
        ApprovalUpdate::Requested { approval_id, .. }
        | ApprovalUpdate::Approved { approval_id }
        | ApprovalUpdate::Denied { approval_id } => approval_id.clone(),
    };
    snapshot
        .pending_approvals
        .retain(|entry| entry.approval_id != approval_id);

    match update {
        ApprovalUpdate::Requested {
            approval_id,
            tool_name,
            reason,
        } => {
            let event_id = recorder.reserve_event_id();
            snapshot.pending_approvals.push(PendingApproval {
                approval_id: approval_id.clone(),
                requested_event_id: event_id.clone(),
                tool_name: tool_name.clone(),
                reason: reason.clone(),
                status: ApprovalStatus::Pending,
            });
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
        ApprovalUpdate::Approved { approval_id } => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Approved,
                    note: None,
                },
            )?;
        }
        ApprovalUpdate::Denied { approval_id } => {
            recorder.record(
                snapshot,
                Some(turn_id),
                RuntimeEventKind::ApprovalDecision {
                    approval_id,
                    status: ApprovalStatus::Denied,
                    note: None,
                },
            )?;
        }
    }

    Ok(())
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
    use crate::llm::{LlmClient, MockLlm};
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult};
    use crate::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
    use crate::session::SessionSnapshot;
    use crate::types::{AssistantTurn, ConversationMessage, SessionId, ToolCall, TurnId};

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
        ) -> Result<AssistantTurn> {
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
                .ok_or_else(|| anyhow::anyhow!("RecordingLlm is out of scripted turns"))
        }
    }

    #[tokio::test]
    async fn thread_session_handles_user_input_and_records_turn_lifecycle() {
        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_thread_session_turn");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
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
        let thread_id = SessionId::new("session_thread_session_live_state");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
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
        let thread_id = SessionId::new("session_thread_session_committed_history");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
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
    async fn thread_session_sampling_prompt_includes_runtime_context() {
        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_thread_session_prompt_context");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().join("subdir"),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
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
        let thread_id = SessionId::new("session_streaming_capture");
        let turn_id = TurnId::new("turn_1");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };
        let snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        let paths = crate::transcript::session_paths(&config.workspace_root, &thread_id);
        crate::transcript::write_json(&paths.snapshot_path, &snapshot).unwrap();
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
}
