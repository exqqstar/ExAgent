use anyhow::Result;

use super::{RuntimeInterrupt, ThreadSession};
use crate::events::RuntimeEventKind;
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::types::{ConversationMessage, TurnId};
// LiveEventSink not imported directly; the recorder field implements it and is
// passed by trait object to Agent::run_live_turn.

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
        // Push user message into the live snapshot first so the TurnStarted
        // event records a snapshot that already contains the user input.
        let mut snapshot = self
            .live_state
            .read()
            .map_err(|_| anyhow::anyhow!("thread session live state rwlock poisoned"))?
            .snapshot
            .clone();
        snapshot
            .conversation
            .push(ConversationMessage::user(prompt));
        self.append_and_broadcast_snapshot(
            &snapshot,
            Some(&turn_id),
            RuntimeEventKind::TurnStarted,
        )?;

        let turn_cwd = turn_context.and_then(|context| context.cwd);
        let Self {
            agent, recorder, ..
        } = self;

        let final_turn = if let Some(interrupt) = interrupt {
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = agent.run_live_turn(&mut snapshot, runtime_turn_id, turn_cwd, recorder) => {
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
            match agent
                .run_live_turn(&mut snapshot, turn_id.clone(), turn_cwd, recorder)
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

        Ok(ThreadOpResult::UserInput { turn_id, final_turn })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use anyhow::Result;
    use tempfile::tempdir;

    use crate::agent::Agent;
    use crate::config::AgentConfig;
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::llm::MockLlm;
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult};
    use crate::runtime::thread_session::{LiveEventSink, ThreadSession, ThreadSessionOptions};
    use crate::session::SessionSnapshot;
    use crate::types::{AssistantTurn, ConversationMessage, EventId, SessionId, ToolCall, TurnId};

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
        assert_eq!(
            final_turn.text.as_deref(),
            Some("session turn complete")
        );

        let replay = crate::transcript::read_session_events(&config.workspace_root, &thread_id)
            .expect("read events");
        assert!(matches!(replay[0].kind, RuntimeEventKind::TurnStarted));
        assert!(matches!(
            replay[1].kind,
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(replay[2].kind, RuntimeEventKind::TurnCompleted));
        assert_eq!(replay[0].turn_id.as_ref(), Some(&turn_id));
        assert_eq!(replay[2].turn_id.as_ref(), Some(&turn_id));
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

        let snapshot = crate::transcript::read_session_snapshot(&config.workspace_root, &thread_id)
            .expect("read snapshot");
        let contents = snapshot
            .conversation
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            contents,
            vec![
                "first input",
                "first turn complete",
                "second input",
                "second turn complete"
            ]
        );
    }

    /// Verifies the F2 streaming contract: Agent calls the sink once per
    /// produced event (not batched at end of turn), and the snapshot passed
    /// to each record() call already reflects the message that produced that
    /// event. This proves a mid-turn reader of the live state would observe
    /// monotonically growing conversation history paired with event records.
    #[tokio::test]
    async fn agent_streams_events_through_live_event_sink_paired_with_snapshot() {
        struct CapturingSink {
            event_kinds: Vec<RuntimeEventKind>,
            conversation_lens_at_record: Vec<usize>,
            thread_id: SessionId,
            next_id: usize,
        }

        impl LiveEventSink for CapturingSink {
            fn reserve_event_id(&mut self) -> EventId {
                let event_id = EventId::new(format!("evt_capture_{}", self.next_id));
                self.next_id += 1;
                event_id
            }

            fn record_reserved(
                &mut self,
                snapshot: &SessionSnapshot,
                turn_id: Option<&TurnId>,
                event_id: EventId,
                kind: RuntimeEventKind,
            ) -> Result<RuntimeEvent> {
                self.conversation_lens_at_record
                    .push(snapshot.conversation.len());
                self.event_kinds.push(kind.clone());
                let event = RuntimeEvent {
                    event_id,
                    session_id: self.thread_id.clone(),
                    turn_id: turn_id.cloned(),
                    kind,
                };
                Ok(event)
            }
        }

        let dir = tempdir().unwrap();
        let thread_id = SessionId::new("session_streaming_capture");
        let config = AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        };

        // Two-step turn: a tool call (which fails because no tool is
        // registered, giving a deterministic ToolResult event) followed by a
        // final assistant turn.
        let llm = MockLlm::new(vec![
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
        ]);

        let agent = Agent::new(config.clone(), Box::new(llm), ToolRegistry::new());
        let mut snapshot = SessionSnapshot::new_thread(
            thread_id.clone(),
            config.workspace_root.clone(),
            config.cwd.clone(),
        );
        snapshot
            .conversation
            .push(ConversationMessage::user("hi".to_string()));
        let starting_len = snapshot.conversation.len();

        let mut sink = CapturingSink {
            event_kinds: Vec::new(),
            conversation_lens_at_record: Vec::new(),
            thread_id: thread_id.clone(),
            next_id: 1,
        };

        let final_turn = agent
            .run_live_turn(&mut snapshot, TurnId::new("turn_1"), None, &mut sink)
            .await
            .expect("agent should complete the two-step turn");

        assert_eq!(final_turn.text.as_deref(), Some("done"));
        // Three sink.record() calls -- AssistantTurn (tool call),
        // ToolResult (no-tool error), AssistantTurn (final). Proves events
        // are recorded one at a time, not batched at end of turn.
        assert_eq!(sink.event_kinds.len(), 3, "expected three streamed events");
        assert!(matches!(
            sink.event_kinds[0],
            RuntimeEventKind::AssistantTurn { .. }
        ));
        assert!(matches!(
            sink.event_kinds[1],
            RuntimeEventKind::ToolResult { .. }
        ));
        assert!(matches!(
            sink.event_kinds[2],
            RuntimeEventKind::AssistantTurn { .. }
        ));
        // Each record() saw a snapshot that already included that step's
        // conversation push, and the conversation grew monotonically.
        assert!(sink
            .conversation_lens_at_record
            .windows(2)
            .all(|window| window[1] >= window[0]));
        assert!(sink.conversation_lens_at_record[0] > starting_len);
    }
}
