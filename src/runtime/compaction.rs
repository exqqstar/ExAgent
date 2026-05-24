use anyhow::{anyhow, Result};

use crate::runtime::agent::Agent;
use crate::types::ConversationMessage;

const DEFAULT_COMPACT_PROMPT: &str = "\
Summarize the conversation so far for a coding agent runtime. \
Preserve user goals, architectural decisions, files changed, commands run, \
open questions, and constraints. Omit irrelevant chatter.";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompactionResult {
    pub(crate) summary: String,
    pub(crate) replacement_history: Vec<ConversationMessage>,
}

pub(crate) async fn compact_history(
    agent: &Agent,
    history: &[ConversationMessage],
) -> Result<CompactionResult> {
    let prompt = build_compaction_prompt(history)?;
    let completion = agent.sample_assistant_turn(&prompt, &[]).await?;
    let summary = completion.turn.text.unwrap_or_default().trim().to_string();
    if summary.is_empty() {
        return Err(anyhow!("empty compaction summary"));
    }

    let replacement_history = vec![ConversationMessage::injected_system(format!(
        "Conversation summary so far:\n{}",
        summary
    ))];

    Ok(CompactionResult {
        summary,
        replacement_history,
    })
}

fn build_compaction_prompt(history: &[ConversationMessage]) -> Result<Vec<ConversationMessage>> {
    Ok(vec![
        ConversationMessage::system(DEFAULT_COMPACT_PROMPT),
        ConversationMessage::user(serde_json::to_string_pretty(history)?),
    ])
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use async_trait::async_trait;

    use super::*;
    use crate::config::AgentConfig;
    use crate::llm::LlmClient;
    use crate::registry::ToolRegistry;
    use crate::runtime::agent::Agent;
    use crate::types::{AssistantTurn, ConversationMessage, LlmCompletion};

    #[derive(Default)]
    struct RecordingLlm {
        prompts: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
        text: Option<String>,
    }

    impl RecordingLlm {
        fn new(text: impl Into<String>) -> Self {
            Self {
                prompts: Arc::new(Mutex::new(Vec::new())),
                text: Some(text.into()),
            }
        }

        fn with_empty_text() -> Self {
            Self {
                prompts: Arc::new(Mutex::new(Vec::new())),
                text: Some(String::new()),
            }
        }
    }

    #[async_trait]
    impl LlmClient for RecordingLlm {
        async fn complete(
            &self,
            messages: &[ConversationMessage],
            _tools: &[serde_json::Value],
        ) -> Result<LlmCompletion> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(messages.to_vec());
            Ok(AssistantTurn {
                text: self.text.clone(),
                tool_calls: vec![],
            }
            .into_completion())
        }
    }

    fn test_agent(llm: RecordingLlm) -> (Agent, Arc<Mutex<Vec<Vec<ConversationMessage>>>>) {
        let prompts = llm.prompts.clone();
        let agent = Agent::new(
            AgentConfig::default(),
            Box::new(llm),
            ToolRegistry::default(),
        );
        (agent, prompts)
    }

    #[tokio::test]
    async fn compaction_prompt_includes_prior_conversation() {
        let (agent, prompts) = test_agent(RecordingLlm::new("summary"));
        let history = vec![
            ConversationMessage::user("keep this user goal"),
            ConversationMessage::assistant(Some("keep this decision".to_string()), vec![]),
        ];

        compact_history(&agent, &history).await.expect("compact");

        let prompts = prompts.lock().expect("prompts");
        let prompt_text = prompts[0]
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt_text.contains("keep this user goal"));
        assert!(prompt_text.contains("keep this decision"));
    }

    #[tokio::test]
    async fn compaction_output_creates_summary_replacement_history() {
        let (agent, _prompts) = test_agent(RecordingLlm::new("important summary"));
        let history = vec![ConversationMessage::user("old prompt")];

        let result = compact_history(&agent, &history).await.expect("compact");

        assert_eq!(result.summary, "important summary");
        assert_eq!(result.replacement_history.len(), 1);
        assert!(result.replacement_history[0].injected);
        assert!(result.replacement_history[0]
            .content
            .contains("important summary"));
    }

    #[tokio::test]
    async fn compaction_replacement_history_contains_only_summary() {
        let (agent, _prompts) = test_agent(RecordingLlm::new("condensed"));
        let history = vec![
            ConversationMessage::user("old prompt"),
            ConversationMessage::assistant(Some("old answer".to_string()), vec![]),
        ];

        let result = compact_history(&agent, &history).await.expect("compact");

        assert_eq!(result.replacement_history.len(), 1);
        assert!(result.replacement_history[0].content.contains("condensed"));
        assert!(!result.replacement_history[0].content.contains("old prompt"));
    }

    #[tokio::test]
    async fn compaction_errors_when_summary_is_empty() {
        let (agent, _prompts) = test_agent(RecordingLlm::with_empty_text());
        let history = vec![ConversationMessage::user("old prompt")];

        let error = compact_history(&agent, &history)
            .await
            .expect_err("empty summary");

        assert!(error.to_string().contains("empty compaction summary"));
    }
}
