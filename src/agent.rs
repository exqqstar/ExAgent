use anyhow::{anyhow, Result};

use crate::config::AgentConfig;
use crate::llm::LlmClient;
use crate::registry::{ToolContext, ToolRegistry};
use crate::types::{AssistantTurn, ConversationMessage};

pub struct Agent {
    config: AgentConfig,
    llm: Box<dyn LlmClient>,
    registry: ToolRegistry,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Box<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self {
            config,
            llm,
            registry,
        }
    }

    pub async fn run(&self, user_prompt: &str) -> Result<AssistantTurn> {
        let mut messages = vec![ConversationMessage::user(user_prompt)];

        let ctx = ToolContext {
            config: self.config.clone(),
        };
        let transcript_path = self.config.workspace_root.join(".exagent/transcript.jsonl");

        for _ in 0..self.config.max_turns {
            let turn = self
                .llm
                .complete(&messages, &self.registry.schemas())
                .await?;
            crate::transcript::append_json_line(&transcript_path, &turn)?;

            if turn.text.is_some() || !turn.tool_calls.is_empty() {
                messages.push(ConversationMessage::assistant(
                    turn.text.clone(),
                    turn.tool_calls.clone(),
                ));
            }

            if turn.tool_calls.is_empty() {
                return Ok(turn);
            }

            for call in turn.tool_calls.clone() {
                let result = self.registry.execute(call, Some(&ctx)).await;
                crate::transcript::append_json_line(&transcript_path, &result)?;
                messages.push(ConversationMessage::tool(
                    result.tool_call_id.clone(),
                    serde_json::to_string(&result)?,
                ));
            }
        }

        Err(anyhow!(
            "Agent reached max turns ({}) without a final assistant turn",
            self.config.max_turns
        ))
    }
}
