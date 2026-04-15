use anyhow::{anyhow, Result};
use async_trait::async_trait;
use exagent::agent::Agent;
use exagent::config::AgentConfig;
use exagent::llm::{LlmClient, MockLlm};
use exagent::registry::ToolRegistry;
use exagent::tools::write_file::WriteFileTool;
use exagent::types::{AssistantTurn, ConversationMessage, MessageRole, ToolCall};
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
    let runs_dir = dir.path().join(".exagent/runs");
    assert!(runs_dir.exists());
    assert_eq!(std::fs::read_dir(runs_dir).unwrap().count(), 1);
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
async fn agent_creates_a_new_transcript_for_each_run() {
    let dir = tempdir().unwrap();

    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let agent_one = Agent::new(
        config.clone(),
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("first".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new(),
    );
    let agent_two = Agent::new(
        config,
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("second".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new(),
    );

    agent_one.run("first prompt").await.unwrap();
    agent_two.run("second prompt").await.unwrap();

    let runs_dir = dir.path().join(".exagent/runs");
    assert_eq!(std::fs::read_dir(runs_dir).unwrap().count(), 2);
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
