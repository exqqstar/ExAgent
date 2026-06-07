use std::collections::VecDeque;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::config::ThinkingMode;
use crate::tools::ToolSpec;
use crate::types::{AssistantTurn, ConversationMessage, LlmCompletion};

pub use super::anthropic::AnthropicLlm;
pub use super::chatgpt_codex::ChatGptCodexLlm;
pub use super::gemini::GeminiLlm;
pub use super::github_copilot::GitHubCopilotLlm;
pub use super::openai_compatible::OpenAiCompatibleLlm;
use super::reasoning::ReasoningCapabilities;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmRequestOptions {
    pub model: Option<String>,
    pub thinking_mode: Option<ThinkingMode>,
    pub reasoning_capabilities: Option<ReasoningCapabilities>,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion>;

    async fn stream(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmCompletion> {
        let completion = self.complete(messages, tools, options).await?;
        sink.event(LlmStreamEvent::Completed(completion.clone()))
            .await?;
        Ok(completion)
    }

    fn is_context_window_error(&self, _err: &anyhow::Error) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmStreamEvent {
    AssistantTextDelta(String),
    ReasoningDelta(String),
    Completed(LlmCompletion),
}

#[async_trait]
pub trait LlmStreamSink: Send {
    async fn event(&mut self, event: LlmStreamEvent) -> Result<()>;
}

pub fn is_context_window_error(err: &anyhow::Error) -> bool {
    super::openai_compatible::is_openai_context_window_error(err)
        || super::anthropic::is_anthropic_context_window_error(err)
        || super::gemini::is_gemini_context_window_error(err)
}

pub struct MockLlm {
    completions: Mutex<VecDeque<LlmCompletion>>,
}

impl MockLlm {
    pub fn new(turns: Vec<AssistantTurn>) -> Self {
        Self::new_completions(
            turns
                .into_iter()
                .map(AssistantTurn::into_completion)
                .collect(),
        )
    }

    pub fn new_completions(completions: Vec<LlmCompletion>) -> Self {
        Self {
            completions: Mutex::new(completions.into()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.completions
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("MockLlm is out of scripted turns"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TokenUsage;

    #[derive(Default)]
    struct RecordingSink {
        events: Vec<LlmStreamEvent>,
    }

    #[async_trait]
    impl LlmStreamSink for RecordingSink {
        async fn event(&mut self, event: LlmStreamEvent) -> Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_stream_wraps_complete_result_as_completed_event() {
        let completion = LlmCompletion {
            turn: AssistantTurn {
                text: Some("hello".to_string()),
                tool_calls: vec![],
                reasoning: vec![],
            },
            token_usage: Some(TokenUsage {
                input_tokens: 1,
                cached_input_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 3,
                total_tokens: 6,
            }),
        };
        let llm = MockLlm::new_completions(vec![completion.clone()]);
        let mut sink = RecordingSink::default();

        let returned = llm
            .stream(&[], &[], &LlmRequestOptions::default(), &mut sink)
            .await
            .expect("stream completion");

        assert_eq!(returned, completion);
        assert_eq!(sink.events, vec![LlmStreamEvent::Completed(completion)]);
    }
}
