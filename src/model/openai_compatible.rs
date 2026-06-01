use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::llm::{LlmClient, LlmRequestOptions};
use super::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::config::ThinkingMode;
use crate::types::{
    AssistantTurn, ConversationMessage, LlmCompletion, MessageRole, TokenUsage, ToolCall,
};

pub struct OpenAiCompatibleLlm {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiCompatibleLlm {
    pub fn from_config(model: &ResolvedModelConfig) -> Result<Self> {
        let base_url = model
            .endpoint
            .base_url
            .clone()
            .context("OPENAI_BASE_URL is required for the OpenAI-compatible adapter")?;
        let api_key = match &model.credential {
            ResolvedCredential::ApiKey(value) | ResolvedCredential::BearerToken(value) => {
                Some(value.clone())
            }
            ResolvedCredential::None => None,
        };

        Self::from_parts(model.identity.model_id.clone(), base_url, api_key)
    }

    pub fn from_parts(
        model: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<impl Into<String>>,
    ) -> Result<Self> {
        let model = model.into();
        let base_url = base_url.into();
        let api_key = api_key
            .map(Into::into)
            .filter(|value: &String| !value.trim().is_empty());
        if model.trim().is_empty() {
            bail!("model is required for the OpenAI-compatible adapter");
        }
        if base_url.trim().is_empty() {
            bail!("OPENAI_BASE_URL is required for the OpenAI-compatible adapter");
        }

        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: chat_completions_endpoint(&base_url),
            api_key,
            model,
        })
    }

    pub fn parse_response(value: Value) -> Result<LlmCompletion> {
        let response: ChatCompletionResponse =
            serde_json::from_value(value).context("Failed to parse chat completion response")?;
        let token_usage = response.usage.map(TokenUsage::from);
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Chat completion response had no choices"))?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tool_call| {
                let arguments: Value = serde_json::from_str(&tool_call.function.arguments)
                    .with_context(|| {
                        format!("Tool call {} returned invalid JSON arguments", tool_call.id)
                    })?;
                Ok(ToolCall {
                    id: tool_call.id,
                    name: tool_call.function.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(LlmCompletion {
            turn: AssistantTurn {
                text: choice.message.content,
                tool_calls,
            },
            token_usage,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatibleLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let request_model = options
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.model)
            .to_string();
        let request = build_chat_completion_request(request_model, messages, tools, options)?;

        let mut request_builder = self.client.post(&self.endpoint).json(&request);
        if let Some(api_key) = self.api_key.as_deref() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder
            .send()
            .await
            .context("Failed to send chat completion request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read chat completion response body")?;

        if !status.is_success() {
            bail!(
                "OpenAI-compatible request failed with status {}: {}",
                status,
                body
            );
        }

        let value: Value =
            serde_json::from_str(&body).context("Failed to decode chat completion JSON body")?;
        Self::parse_response(value)
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_openai_context_window_error(err)
    }
}

pub fn is_openai_context_window_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    [
        "context_length_exceeded",
        "maximum context length",
        "context window",
        "too many tokens",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatRequestMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatRequestTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum ChatRequestMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatRequestToolCall>,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Debug, Serialize)]
struct ChatRequestTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ChatRequestFunction,
}

#[derive(Debug, Serialize)]
struct ChatRequestFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct ChatRequestToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: ChatRequestToolCallFunction,
}

#[derive(Debug, Serialize)]
struct ChatRequestToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatResponseToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseToolCall {
    id: String,
    function: ChatResponseToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct ChatResponseToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    #[serde(default)]
    prompt_tokens_details: Option<ChatPromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<ChatCompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatPromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

impl From<ChatUsage> for TokenUsage {
    fn from(usage: ChatUsage) -> Self {
        Self {
            input_tokens: usage.prompt_tokens,
            cached_input_tokens: usage
                .prompt_tokens_details
                .map(|details| details.cached_tokens)
                .unwrap_or_default(),
            output_tokens: usage.completion_tokens,
            reasoning_output_tokens: usage
                .completion_tokens_details
                .map(|details| details.reasoning_tokens)
                .unwrap_or_default(),
            total_tokens: usage.total_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct InternalToolSchema {
    name: String,
    description: String,
    input_schema: Value,
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn build_chat_completion_request(
    model: String,
    messages: &[ConversationMessage],
    tools: &[Value],
    options: &LlmRequestOptions,
) -> Result<ChatCompletionRequest> {
    let model = options
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&model)
        .to_string();
    Ok(ChatCompletionRequest {
        model,
        messages: build_request_messages(messages)?,
        tools: build_request_tools(tools)?,
        reasoning_effort: options
            .thinking_mode
            .and_then(reasoning_effort_for_thinking_mode)
            .map(str::to_string),
    })
}

fn reasoning_effort_for_thinking_mode(mode: ThinkingMode) -> Option<&'static str> {
    match mode {
        ThinkingMode::Auto => None,
        ThinkingMode::Low => Some("low"),
        ThinkingMode::Medium => Some("medium"),
        ThinkingMode::High => Some("high"),
    }
}

fn build_request_tools(tools: &[Value]) -> Result<Vec<ChatRequestTool>> {
    tools
        .iter()
        .cloned()
        .map(|tool| {
            let tool: InternalToolSchema =
                serde_json::from_value(tool).context("Invalid internal tool schema")?;
            Ok(ChatRequestTool {
                kind: "function",
                function: ChatRequestFunction {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.input_schema,
                },
            })
        })
        .collect()
}

fn build_request_messages(messages: &[ConversationMessage]) -> Result<Vec<ChatRequestMessage>> {
    messages
        .iter()
        .map(|message| match message.role {
            MessageRole::System => Ok(ChatRequestMessage::System {
                content: message.content.clone(),
            }),
            MessageRole::User => Ok(ChatRequestMessage::User {
                content: message.content.clone(),
            }),
            MessageRole::Assistant => Ok(ChatRequestMessage::Assistant {
                content: if message.content.is_empty() {
                    None
                } else {
                    Some(message.content.clone())
                },
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|tool_call| ChatRequestToolCall {
                        id: tool_call.id.clone(),
                        kind: "function",
                        function: ChatRequestToolCallFunction {
                            name: tool_call.name.clone(),
                            arguments: tool_call.arguments.to_string(),
                        },
                    })
                    .collect(),
            }),
            MessageRole::Tool => Ok(ChatRequestMessage::Tool {
                content: message.content.clone(),
                tool_call_id: message
                    .tool_call_id
                    .clone()
                    .ok_or_else(|| anyhow!("Tool messages require tool_call_id"))?,
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThinkingMode;

    #[test]
    fn chat_completion_request_serializes_reasoning_effort_when_thinking_mode_is_set() {
        let request = build_chat_completion_request(
            "gpt-thinking".to_string(),
            &[],
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: Some(ThinkingMode::High),
            },
        )
        .unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["reasoning_effort"], "high");
    }

    #[test]
    fn chat_completion_request_omits_reasoning_effort_when_thinking_mode_is_auto_or_unset() {
        let auto = build_chat_completion_request(
            "gpt-thinking".to_string(),
            &[],
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: Some(ThinkingMode::Auto),
            },
        )
        .unwrap();
        let unset = build_chat_completion_request(
            "gpt-thinking".to_string(),
            &[],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();

        assert!(serde_json::to_value(auto)
            .unwrap()
            .get("reasoning_effort")
            .is_none());
        assert!(serde_json::to_value(unset)
            .unwrap()
            .get("reasoning_effort")
            .is_none());
    }

    #[test]
    fn chat_completion_request_uses_per_turn_model_when_set() {
        let options = LlmRequestOptions {
            model: Some("override-model".to_string()),
            thinking_mode: None,
        };

        let request =
            build_chat_completion_request("base-model".to_string(), &[], &[], &options).unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["model"], "override-model");
    }
}
