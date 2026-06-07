use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::llm::{
    AnthropicLlm, GeminiLlm, GitHubCopilotLlm, LlmClient, LlmRequestOptions, OpenAiCompatibleLlm,
};
use crate::model::chatgpt_codex::{ChatGptCodexLlm, ChatGptCodexTokenRefreshSink};
use crate::provider::ProviderProtocol;
use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::tools::ToolSpec;
use crate::types::{ConversationMessage, LlmCompletion};

pub trait LlmClientFactory: Send + Sync {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>>;
}

#[derive(Default)]
pub struct DefaultLlmClientFactory {
    chatgpt_token_refresh_sink: Option<Arc<dyn ChatGptCodexTokenRefreshSink>>,
}

impl DefaultLlmClientFactory {
    pub fn with_chatgpt_token_refresh_sink(
        chatgpt_token_refresh_sink: Arc<dyn ChatGptCodexTokenRefreshSink>,
    ) -> Self {
        Self {
            chatgpt_token_refresh_sink: Some(chatgpt_token_refresh_sink),
        }
    }
}

impl LlmClientFactory for DefaultLlmClientFactory {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>> {
        match model.protocol {
            ProviderProtocol::OpenAiChatCompletions => match &model.credential {
                ResolvedCredential::ChatGptOAuth { .. } => Ok(Box::new(
                    ChatGptCodexLlm::from_config_with_token_refresh_sink(
                        model,
                        self.chatgpt_token_refresh_sink.clone(),
                    )?,
                )),
                _ => Ok(Box::new(OpenAiCompatibleLlm::from_config(model)?)),
            },
            ProviderProtocol::AnthropicMessages => Ok(Box::new(AnthropicLlm::from_config(model)?)),
            ProviderProtocol::GeminiGenerateContent => Ok(Box::new(GeminiLlm::from_config(model)?)),
            ProviderProtocol::CopilotOAuth => Ok(Box::new(GitHubCopilotLlm::from_config(model)?)),
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
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(messages, tools, options).await
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        self.llm.is_context_window_error(err)
    }
}
