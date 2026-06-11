use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::llm::{LlmClient, LlmRequestOptions};
use super::openai_compatible::{
    build_openai_chat_completion_request_value, is_openai_context_window_error, OpenAiCompatibleLlm,
};
use super::reasoning::ReasoningCapabilities;
use super::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::tools::ToolSpec;
use crate::types::{ConversationMessage, LlmCompletion};

pub const COPILOT_INTEGRATION_ID: &str = "vscode-chat";
pub const COPILOT_EDITOR_VERSION: &str = "vscode/1.104.1";
pub const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.35.0";
pub const COPILOT_USER_AGENT: &str = "ExAgent";
pub const COPILOT_OPENAI_INTENT: &str = "conversation-edits";

pub struct GitHubCopilotLlm {
    client: reqwest::Client,
    endpoint: String,
    token: String,
    model: String,
    reasoning_capabilities: ReasoningCapabilities,
}

impl GitHubCopilotLlm {
    pub fn from_config(model: &ResolvedModelConfig) -> Result<Self> {
        let base_url = model
            .endpoint
            .base_url
            .clone()
            .context("GitHub Copilot base URL is required")?;
        let token = match &model.credential {
            ResolvedCredential::ApiKey(value) | ResolvedCredential::BearerToken(value) => {
                value.clone()
            }
            ResolvedCredential::None => bail!("GitHub Copilot OAuth token is required"),
            ResolvedCredential::ChatGptOAuth { .. } => {
                bail!("ChatGPT OAuth cannot be used with GitHub Copilot")
            }
        };
        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: chat_completions_endpoint(&base_url),
            token,
            model: model.identity.model_id.clone(),
            reasoning_capabilities: model.capabilities.reasoning.clone(),
        })
    }
}

#[async_trait]
impl LlmClient for GitHubCopilotLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let request_model = options
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.model)
            .to_string();
        let reasoning_capabilities = options
            .reasoning_capabilities
            .as_ref()
            .unwrap_or(&self.reasoning_capabilities);
        let request = build_openai_chat_completion_request_value(
            request_model,
            messages,
            tools,
            options,
            reasoning_capabilities,
            false,
        )?;
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .copilot_headers()
            .header("x-initiator", "user")
            .json(&request)
            .send()
            .await
            .context("Failed to send GitHub Copilot request")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .context("Failed to read GitHub Copilot error body")?;
            bail!(
                "GitHub Copilot request failed with status {}: {}",
                status,
                body
            );
        }
        let body = response
            .text()
            .await
            .context("Failed to read GitHub Copilot response body")?;
        let value: Value =
            serde_json::from_str(&body).context("Failed to decode GitHub Copilot JSON body")?;
        OpenAiCompatibleLlm::parse_response(value)
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_openai_context_window_error(err)
    }
}

pub trait CopilotRequestBuilderExt {
    fn copilot_headers(self) -> Self;
}

impl CopilotRequestBuilderExt for reqwest::RequestBuilder {
    fn copilot_headers(self) -> Self {
        self.header("User-Agent", COPILOT_USER_AGENT)
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
            .header("Editor-Version", COPILOT_EDITOR_VERSION)
            .header("Editor-Plugin-Version", COPILOT_EDITOR_PLUGIN_VERSION)
            .header("Openai-Intent", COPILOT_OPENAI_INTENT)
            .header("x-github-api-version", "2025-04-01")
    }
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

pub fn models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/models") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/models")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use axum::{http::HeaderMap, routing::post, Json, Router};
    use serde_json::json;

    use crate::llm::{LlmClient, LlmRequestOptions};
    use crate::resolved::{ResolvedCredential, ResolvedModelConfig};
    use crate::types::ConversationMessage;

    #[tokio::test]
    async fn sends_copilot_oauth_headers_to_chat_completions_endpoint() {
        let saw_headers = Arc::new(AtomicBool::new(false));
        let app_saw_headers = saw_headers.clone();
        let app = Router::new().route(
            "/chat/completions",
            post(move |headers: HeaderMap| {
                let app_saw_headers = app_saw_headers.clone();
                async move {
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|header| header.to_str().ok()),
                        Some("Bearer copilot-token-1")
                    );
                    assert_eq!(
                        headers
                            .get("Openai-Intent")
                            .and_then(|header| header.to_str().ok()),
                        Some("conversation-edits")
                    );
                    assert_eq!(
                        headers
                            .get("Copilot-Integration-Id")
                            .and_then(|header| header.to_str().ok()),
                        Some("vscode-chat")
                    );
                    assert_eq!(
                        headers
                            .get("Editor-Version")
                            .and_then(|header| header.to_str().ok()),
                        Some("vscode/1.104.1")
                    );
                    app_saw_headers.store(true, Ordering::SeqCst);
                    Json(json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "copilot ok"
                            }
                        }]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let config = ResolvedModelConfig::from_provider_profile(
            "github_copilot",
            "gpt-5.5",
            Some(format!("http://{addr}")),
            ResolvedCredential::BearerToken("copilot-token-1".to_string()),
            None,
        );
        let llm = super::GitHubCopilotLlm::from_config(&config).unwrap();
        let completion = llm
            .complete(
                &[ConversationMessage::user("hello")],
                &[],
                &LlmRequestOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("copilot ok"));
        assert!(saw_headers.load(Ordering::SeqCst));
    }
}
