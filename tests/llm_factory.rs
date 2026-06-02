use std::sync::Arc;

use async_trait::async_trait;
use exagent::llm::{LlmClient, LlmRequestOptions};
use exagent::model::factory::{DefaultLlmClientFactory, LlmClientFactory, SharedLlmFactory};
use exagent::provider::ProviderProtocol;
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::types::{AssistantTurn, ConversationMessage, LlmCompletion};

struct StaticLlm;

#[async_trait]
impl LlmClient for StaticLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        Ok(AssistantTurn {
            text: Some("ok".to_string()),
            tool_calls: Vec::new(),
        }
        .into_completion())
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        err.to_string().contains("static context")
    }
}

#[test]
fn default_factory_builds_openai_compatible_client() {
    let model = ResolvedModelConfig::from_provider_profile(
        "openai_compatible",
        "local-model",
        Some("http://127.0.0.1:11434/v1".to_string()),
        ResolvedCredential::None,
        None,
    );

    let client = DefaultLlmClientFactory.build(&model);

    assert!(client.is_ok());
}

#[test]
fn default_factory_rejects_unsupported_protocols() {
    let mut model = ResolvedModelConfig::from_provider_profile(
        "anthropic",
        "claude-sonnet",
        Some("https://api.anthropic.com/v1".to_string()),
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );
    model.protocol = ProviderProtocol::AnthropicMessages;

    let err = DefaultLlmClientFactory
        .build(&model)
        .err()
        .unwrap()
        .to_string();

    assert!(err.contains("not implemented"));
}

#[tokio::test]
async fn shared_factory_ignores_model_config_and_forwards_client_calls() {
    let factory = SharedLlmFactory::new(Arc::new(StaticLlm));
    let model = ResolvedModelConfig::default();

    let client = factory.build(&model).unwrap();
    let completion = client
        .complete(&[], &[], &LlmRequestOptions::default())
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
    assert!(client.is_context_window_error(&anyhow::anyhow!("static context exceeded")));
}
