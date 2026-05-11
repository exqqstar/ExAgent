use anyhow::{anyhow, Result};
use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::llm::{LlmClient, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::runtime::{RuntimeExecution, RuntimeOp, RuntimeOpExecutor, TurnContext, UserInput};
use exagent::session::AgentRole;
use exagent::tools::write_file::WriteFileTool;
use exagent::types::{
    AssistantTurn, ConversationMessage, MessageRole, SessionId, ToolCall, TurnId,
};
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[tokio::test]
async fn agent_runs_until_assistant_returns_no_tool_calls() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("writing".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "out.txt", "content": "hello"}),
            }],
        },
        AssistantTurn {
            text: Some("done".into()),
            tool_calls: vec![],
        },
    ]);

    let mut registry = ToolRegistry::new();
    registry.register(WriteFileTool);

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), registry);
    let final_turn = agent.run("create a file").await.unwrap();

    assert_eq!(final_turn.text.as_deref(), Some("done"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "hello"
    );
    let sessions_dir = dir.path().join(".exagent/sessions");
    let session_dirs = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(session_dirs.len(), 1);
    let session_dir = session_dirs[0].path();
    assert!(session_dir.join("snapshot.json").exists());
    assert!(session_dir.join("events.jsonl").exists());
}

#[tokio::test]
async fn agent_feeds_tool_errors_back_into_next_turn() {
    let dir = tempdir().unwrap();
    let llm = InspectingLlm::default();

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let final_turn = agent.run("do something").await.unwrap();

    assert_eq!(final_turn.text.as_deref(), Some("handled tool error"));
}

#[tokio::test]
async fn agent_executes_runtime_user_input_op_with_existing_session_id() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![AssistantTurn {
        text: Some("runtime done".into()),
        tool_calls: vec![],
    }]);
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let session_id = SessionId::new("session_runtime_1");
    let turn_id = TurnId::new("turn_runtime_1");

    let result = agent
        .execute_op(
            &session_id,
            RuntimeOp::UserInput {
                turn_id: turn_id.clone(),
                input: vec![UserInput {
                    content: "execute through runtime op".into(),
                }],
                context: TurnContext {
                    model: "runtime-model".into(),
                    workspace_root: dir.path().to_path_buf(),
                    cwd: dir.path().to_path_buf(),
                    policy_mode: exagent::policy::PolicyMode::Off,
                    agent_role: AgentRole::Primary,
                    instructions: vec![],
                },
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        RuntimeExecution {
            session_id: session_id.clone(),
            turn_id: Some(turn_id),
            status: "completed".into(),
        }
    );

    let snapshot = exagent::transcript::read_session_snapshot(dir.path(), &session_id).unwrap();
    assert_eq!(snapshot.session_id, session_id);
    assert_eq!(
        snapshot.conversation[0].content,
        "execute through runtime op"
    );
    assert!(snapshot
        .conversation
        .iter()
        .any(|message| message.content == "runtime done"));
}

#[tokio::test]
async fn agent_runtime_user_input_op_resumes_existing_session_state() {
    let dir = tempdir().unwrap();
    let llm = MockLlm::new(vec![
        AssistantTurn {
            text: Some("first done".into()),
            tool_calls: vec![],
        },
        AssistantTurn {
            text: Some("second done".into()),
            tool_calls: vec![],
        },
    ]);
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    let agent = Agent::new(config, Box::new(llm), ToolRegistry::new());
    let session_id = SessionId::new("session_runtime_resume");
    let context = TurnContext {
        model: "runtime-model".into(),
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        policy_mode: exagent::policy::PolicyMode::Off,
        agent_role: AgentRole::Primary,
        instructions: vec![],
    };

    agent
        .execute_op(
            &session_id,
            RuntimeOp::UserInput {
                turn_id: TurnId::new("turn_runtime_1"),
                input: vec![UserInput {
                    content: "first input".into(),
                }],
                context: context.clone(),
            },
        )
        .await
        .unwrap();
    agent
        .execute_op(
            &session_id,
            RuntimeOp::UserInput {
                turn_id: TurnId::new("turn_runtime_2"),
                input: vec![UserInput {
                    content: "second input".into(),
                }],
                context,
            },
        )
        .await
        .unwrap();

    let snapshot = exagent::transcript::read_session_snapshot(dir.path(), &session_id).unwrap();
    let contents = snapshot
        .conversation
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert!(contents.contains(&"first input"));
    assert!(contents.contains(&"first done"));
    assert!(contents.contains(&"second input"));
    assert!(contents.contains(&"second done"));
}

#[derive(Default)]
struct InspectingLlm {
    calls: Mutex<usize>,
}

#[async_trait]
impl LlmClient for InspectingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        let mut calls = self.calls.lock().await;
        *calls += 1;

        match *calls {
            1 => {
                assert_eq!(messages.len(), 1);
                assert!(matches!(messages[0].role, MessageRole::User));
                Ok(AssistantTurn {
                    text: Some("trying tool".into()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "missing_tool".into(),
                        arguments: json!({}),
                    }],
                })
            }
            2 => {
                let assistant_message = messages
                    .iter()
                    .find(|message| matches!(message.role, MessageRole::Assistant))
                    .ok_or_else(|| anyhow!("expected assistant message"))?;
                assert_eq!(assistant_message.tool_calls.len(), 1);
                assert_eq!(assistant_message.tool_calls[0].id, "call_1");

                let tool_message = messages
                    .iter()
                    .find(|message| matches!(message.role, MessageRole::Tool))
                    .ok_or_else(|| anyhow!("expected tool message"))?;
                assert_eq!(tool_message.tool_call_id.as_deref(), Some("call_1"));
                assert!(tool_message.content.contains("\"status\":\"error\""));
                assert!(tool_message
                    .content
                    .contains("\"tool_name\":\"missing_tool\""));

                Ok(AssistantTurn {
                    text: Some("handled tool error".into()),
                    tool_calls: vec![],
                })
            }
            _ => Err(anyhow!("unexpected extra llm call")),
        }
    }
}
