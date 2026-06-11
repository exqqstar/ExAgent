use anyhow::{anyhow, Result};

use crate::llm::LlmRequestOptions;
use crate::runtime::agent::Agent;
use crate::types::ConversationMessage;

const DEFAULT_COMPACT_PROMPT: &str = "\
Write a concise summary for a coding agent that will continue this work. \
Omit irrelevant chatter and preserve only actionable context. \
Use these sections:\n\
- Current goal\n\
- Important constraints\n\
- Decisions made\n\
- Files changed\n\
- Commands and tests run\n\
- Open issues\n\
- Next suggested step";

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
        ConversationMessage::user(serde_json::to_string_pretty(
            &sanitize_history_for_compaction(history),
        )?),
    ])
}

fn sanitize_history_for_compaction(history: &[ConversationMessage]) -> Vec<ConversationMessage> {
    history
        .iter()
        .cloned()
        .map(|mut message| {
            message.reasoning.clear();
            for tool_call in &mut message.tool_calls {
                tool_call.thought_signature = None;
            }
            message
        })
        .collect()
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
    use crate::tools::ToolSpec;
    use crate::types::{
        AssistantTurn, ConversationMessage, LlmCompletion, ReasoningBlock, ReasoningSignature,
        ToolCall,
    };

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
            _tools: &[ToolSpec],
            _options: &crate::llm::LlmRequestOptions,
        ) -> Result<LlmCompletion> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(messages.to_vec());
            Ok(AssistantTurn {
                text: self.text.clone(),
                tool_calls: vec![],
                reasoning: vec![],
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
    async fn compaction_prompt_requests_structured_coding_agent_summary() {
        let (agent, prompts) = test_agent(RecordingLlm::new("summary"));
        let history = vec![ConversationMessage::user("old prompt")];

        compact_history(&agent, &history).await.expect("compact");

        let prompts = prompts.lock().expect("prompts");
        let system_prompt = &prompts[0][0].content;
        for section in [
            "Current goal",
            "Important constraints",
            "Decisions made",
            "Files changed",
            "Commands and tests run",
            "Open issues",
            "Next suggested step",
        ] {
            assert!(
                system_prompt.contains(section),
                "prompt should request section: {section}"
            );
        }
    }

    #[tokio::test]
    async fn compaction_prompt_omits_reasoning_metadata() {
        let (agent, prompts) = test_agent(RecordingLlm::new("summary"));
        let history = vec![ConversationMessage::assistant_with_reasoning(
            Some("keep visible answer".to_string()),
            vec![ReasoningBlock {
                text: "private reasoning text".to_string(),
                signature: Some(ReasoningSignature::GeminiThoughtSignature(
                    "private-reasoning-signature".to_string(),
                )),
                redacted: false,
            }],
            vec![ToolCall {
                id: "tool-call-1".to_string(),
                name: "visible_tool".to_string(),
                arguments: serde_json::json!({"visible": true}),
                thought_signature: Some(serde_json::json!("private-tool-signature")),
            }],
        )];

        compact_history(&agent, &history).await.expect("compact");

        let prompts = prompts.lock().expect("prompts");
        let prompt_text = prompts[0]
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt_text.contains("keep visible answer"));
        assert!(prompt_text.contains("visible_tool"));
        assert!(!prompt_text.contains("private reasoning text"));
        assert!(!prompt_text.contains("private-reasoning-signature"));
        assert!(!prompt_text.contains("private-tool-signature"));
        assert!(!prompt_text.contains("thought_signature"));
        assert!(!prompt_text.contains("\"reasoning\""));
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
