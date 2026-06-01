use std::collections::VecDeque;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::config::ThinkingMode;
use crate::types::{AssistantTurn, ConversationMessage, LlmCompletion};

pub use super::openai_compatible::OpenAiCompatibleLlm;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmRequestOptions {
    pub model: Option<String>,
    pub thinking_mode: Option<ThinkingMode>,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion>;

    fn is_context_window_error(&self, _err: &anyhow::Error) -> bool {
        false
    }
}

pub fn is_context_window_error(err: &anyhow::Error) -> bool {
    super::openai_compatible::is_openai_context_window_error(err)
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
        _tools: &[serde_json::Value],
        _options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.completions
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("MockLlm is out of scripted turns"))
    }
}
