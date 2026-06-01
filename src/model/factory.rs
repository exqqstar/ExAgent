use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;

use crate::llm::{LlmClient, LlmRequestOptions, OpenAiCompatibleLlm};
use crate::provider::ProviderProtocol;
use crate::resolved::ResolvedModelConfig;
use crate::types::{ConversationMessage, LlmCompletion};

pub trait LlmClientFactory: Send + Sync {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>>;
}

pub struct DefaultLlmClientFactory;

impl LlmClientFactory for DefaultLlmClientFactory {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>> {
        match model.protocol {
            ProviderProtocol::OpenAiChatCompletions => {
                Ok(Box::new(OpenAiCompatibleLlm::from_config(model)?))
            }
            ProviderProtocol::AnthropicMessages => {
                bail!("Anthropic Messages adapter is not implemented")
            }
            ProviderProtocol::GeminiGenerateContent => {
                bail!("Gemini Generate Content adapter is not implemented")
            }
            ProviderProtocol::CopilotOAuth => {
                bail!("GitHub Copilot OAuth runtime adapter is not implemented")
            }
        }
    }
}

pub struct SharedLlmFactory {
    llm: Arc<dyn LlmClient>,
}

impl SharedLlmFactory {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

impl LlmClientFactory for SharedLlmFactory {
    fn build(&self, _model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(SharedLlmClient {
            llm: self.llm.clone(),
        }))
    }
}

struct SharedLlmClient {
    llm: Arc<dyn LlmClient>,
}

#[async_trait]
impl LlmClient for SharedLlmClient {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(messages, tools, options).await
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        self.llm.is_context_window_error(err)
    }
}
