use anyhow::Result;

use super::{RuntimeInterrupt, ThreadSession};
use crate::events::RuntimeEventKind;
use crate::runtime::thread_runtime::{
    ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext,
};
use crate::types::TurnId;

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
        self.append_and_broadcast(Some(&turn_id), RuntimeEventKind::TurnStarted)?;
        let broadcasted_event_count = self.persisted_event_count(&self.thread_id)?;

        let turn_cwd = turn_context.and_then(|context| context.cwd);
        let output = if let Some(interrupt) = interrupt {
            let runtime_turn_id = turn_id.clone();
            tokio::select! {
                result = self.agent.run_live_turn(
                    &mut self.snapshot,
                    &prompt,
                    runtime_turn_id,
                    turn_cwd,
                    &mut self.next_event_index,
                ) => {
                    match result {
                        Ok(output) => output,
                        Err(err) => {
                            self.broadcast_events_since(broadcasted_event_count)?;
                            let message = err.to_string();
                            self.append_and_broadcast(
                                Some(&turn_id),
                                RuntimeEventKind::RuntimeError { message },
                            )?;
                            return Err(err);
                        }
                    }
                }
                _ = interrupt.interrupt_rx => {
                    self.append_and_broadcast(
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
            let result = self
                .agent
                .run_live_turn(
                    &mut self.snapshot,
                    &prompt,
                    turn_id.clone(),
                    turn_cwd,
                    &mut self.next_event_index,
                )
                .await;
            match result {
                Ok(output) => output,
                Err(err) => {
                    self.broadcast_events_since(broadcasted_event_count)?;
                    let message = err.to_string();
                    self.append_and_broadcast(
                        Some(&turn_id),
                        RuntimeEventKind::RuntimeError { message },
                    )?;
                    return Err(err);
                }
            }
        };

        for event in &output.events {
            let _ = self.event_tx.send(event.clone());
        }
        self.append_and_broadcast(Some(&turn_id), RuntimeEventKind::TurnCompleted)?;

        Ok(ThreadOpResult::UserInput { turn_id, output })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tempfile::tempdir;

    use crate::agent::Agent;
    use crate::config::AgentConfig;
    use crate::events::RuntimeEventKind;
    use crate::llm::MockLlm;
    use crate::registry::ToolRegistry;
    use crate::runtime::thread_runtime::{AgentFactory, ThreadOpResult};
    use crate::runtime::thread_session::{ThreadSession, ThreadSessionOptions};
    use crate::session::SessionSnapshot;
    use crate::types::{AssistantTurn, SessionId, TurnId};

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

        let ThreadOpResult::UserInput { output, .. } = result else {
            panic!("expected user input result");
        };
        assert_eq!(
            output.final_turn.text.as_deref(),
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
            output: first_output,
            ..
        } = first
        else {
            panic!("expected first user input result");
        };
        let ThreadOpResult::UserInput {
            output: second_output,
            ..
        } = second
        else {
            panic!("expected second user input result");
        };
        assert_eq!(
            first_output.final_turn.text.as_deref(),
            Some("first turn complete")
        );
        assert_eq!(
            second_output.final_turn.text.as_deref(),
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
}
