use std::collections::VecDeque;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::types::{AssistantTurn, ConversationMessage, MessageRole, ToolCall};

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<AssistantTurn>;
}

pub struct MockLlm {
    turns: Mutex<VecDeque<AssistantTurn>>,
}

impl MockLlm {
    pub fn new(turns: Vec<AssistantTurn>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        self.turns
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("MockLlm is out of scripted turns"))
    }
}

pub struct OpenAiCompatibleLlm {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatibleLlm {
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("OPENAI_BASE_URL")
            .context("OPENAI_BASE_URL is required for the OpenAI-compatible adapter")?;
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY is required for the OpenAI-compatible adapter")?;
        let model = std::env::var("OPENAI_MODEL")
            .context("OPENAI_MODEL is required for the OpenAI-compatible adapter")?;

        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: chat_completions_endpoint(&base_url),
            api_key,
            model,
        })
    }

    pub fn parse_response(value: Value) -> Result<AssistantTurn> {
        let response: ChatCompletionResponse =
            serde_json::from_value(value).context("Failed to parse chat completion response")?;
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

        Ok(AssistantTurn {
            text: choice.message.content,
            tool_calls,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatibleLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<AssistantTurn> {
        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: build_request_messages(messages)?,
            tools: build_request_tools(tools)?,
        };

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
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
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatRequestMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatRequestTool>,
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
