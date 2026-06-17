use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::llm::{LlmClient, LlmRequestOptions};
use super::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::config::ThinkingMode;
use crate::model::image_input::load_local_image_for_prompt;
use crate::tools::{ToolSpec, ToolSpecKind};
use crate::types::{
    AssistantTurn, ConversationContentPart, ConversationMessage, LlmCompletion, MessageRole,
    ReasoningBlock, ReasoningSignature, TokenUsage, ToolCall,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: i64 = 4096;

pub struct AnthropicLlm {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl AnthropicLlm {
    pub fn from_config(model: &ResolvedModelConfig) -> Result<Self> {
        let base_url = model
            .endpoint
            .base_url
            .clone()
            .context("ANTHROPIC_BASE_URL is required for the Anthropic adapter")?;
        let api_key = match &model.credential {
            ResolvedCredential::ApiKey(value) | ResolvedCredential::BearerToken(value) => {
                value.clone()
            }
            ResolvedCredential::None => bail!("ANTHROPIC_API_KEY is required for Anthropic"),
            ResolvedCredential::ChatGptOAuth { .. } => {
                bail!("ChatGPT OAuth cannot be used with Anthropic")
            }
        };

        Self::from_parts(model.identity.model_id.clone(), base_url, api_key)
    }

    pub fn from_parts(
        model: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self> {
        let model = model.into();
        let base_url = base_url.into();
        let api_key = api_key.into();
        if model.trim().is_empty() {
            bail!("model is required for the Anthropic adapter");
        }
        if base_url.trim().is_empty() {
            bail!("ANTHROPIC_BASE_URL is required for the Anthropic adapter");
        }
        if api_key.trim().is_empty() {
            bail!("ANTHROPIC_API_KEY is required for Anthropic");
        }

        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: messages_endpoint(&base_url),
            api_key,
            model,
        })
    }

    pub fn parse_response(value: Value) -> Result<LlmCompletion> {
        let response: AnthropicResponse =
            serde_json::from_value(value).context("Failed to parse Anthropic Messages response")?;
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut reasoning = Vec::new();
        for item in response.content {
            match item {
                AnthropicResponseContent::Thinking {
                    thinking,
                    signature,
                } => reasoning.push(ReasoningBlock {
                    text: thinking,
                    signature: signature.map(ReasoningSignature::AnthropicSignature),
                    redacted: false,
                }),
                AnthropicResponseContent::RedactedThinking { data } => {
                    reasoning.push(ReasoningBlock {
                        text: String::new(),
                        signature: Some(ReasoningSignature::AnthropicRedactedData(data)),
                        redacted: true,
                    });
                }
                AnthropicResponseContent::Text { text } => text_parts.push(text),
                AnthropicResponseContent::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                        thought_signature: None,
                    });
                }
                AnthropicResponseContent::Unknown => {}
            }
        }

        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join("\n"))
        };

        Ok(LlmCompletion {
            turn: AssistantTurn {
                text,
                tool_calls,
                reasoning,
            },
            token_usage: response.usage.map(TokenUsage::from),
        })
    }
}

#[async_trait]
impl LlmClient for AnthropicLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let model = options
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.model)
            .to_string();
        let request = build_anthropic_request(model, messages, tools, options)?;
        let response = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .await
            .context("Failed to send Anthropic Messages request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read Anthropic Messages response body")?;

        if !status.is_success() {
            bail!("Anthropic request failed with status {}: {}", status, body);
        }

        let value: Value =
            serde_json::from_str(&body).context("Failed to decode Anthropic JSON body")?;
        Self::parse_response(value)
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_anthropic_context_window_error(err)
    }
}

pub fn is_anthropic_context_window_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    ["context window", "prompt is too long", "too many tokens"]
        .iter()
        .any(|needle| message.contains(needle))
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: i64,
    messages: Vec<AnthropicRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicRequestTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
}

#[derive(Debug, Serialize)]
struct AnthropicRequestMessage {
    role: &'static str,
    content: Vec<AnthropicRequestContent>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicRequestContent {
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<AnthropicRequestContent>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Serialize)]
struct AnthropicRequestTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct AnthropicThinking {
    r#type: &'static str,
    budget_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseContent>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicResponseContent {
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

impl From<AnthropicUsage> for TokenUsage {
    fn from(usage: AnthropicUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cache_read_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: 0,
            total_tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        }
    }
}

fn messages_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/messages")
    }
}

fn build_anthropic_request(
    model: String,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    options: &LlmRequestOptions,
) -> Result<AnthropicRequest> {
    let mut system = Vec::new();
    let mut request_messages = Vec::new();

    for message in messages {
        match message.role {
            MessageRole::System => system.push(message.content.clone()),
            MessageRole::User => request_messages.push(AnthropicRequestMessage {
                role: "user",
                content: anthropic_user_content(message),
            }),
            MessageRole::Assistant => {
                let mut content = Vec::new();
                content.extend(
                    message
                        .reasoning
                        .iter()
                        .filter_map(anthropic_reasoning_content),
                );
                if !message.content.is_empty() {
                    content.push(AnthropicRequestContent::Text {
                        text: message.content.clone(),
                    });
                }
                content.extend(message.tool_calls.iter().map(|tool_call| {
                    AnthropicRequestContent::ToolUse {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        input: tool_call.arguments.clone(),
                    }
                }));
                if content.is_empty() {
                    return Err(anyhow!(
                        "Assistant messages require reasoning, text, or tool calls"
                    ));
                }
                request_messages.push(AnthropicRequestMessage {
                    role: "assistant",
                    content,
                });
            }
            MessageRole::Tool => request_messages.push(AnthropicRequestMessage {
                role: "user",
                content: vec![AnthropicRequestContent::ToolResult {
                    tool_use_id: message
                        .tool_call_id
                        .clone()
                        .ok_or_else(|| anyhow!("Tool messages require tool_call_id"))?,
                    content: anthropic_tool_result_content(message),
                }],
            }),
        }
    }

    let thinking = anthropic_thinking(options.thinking_mode);
    let max_tokens = thinking
        .as_ref()
        .map(|thinking| DEFAULT_MAX_TOKENS.max(thinking.budget_tokens.saturating_add(1024)))
        .unwrap_or(DEFAULT_MAX_TOKENS);

    Ok(AnthropicRequest {
        model,
        max_tokens,
        messages: request_messages,
        system: (!system.is_empty()).then(|| system.join("\n\n")),
        tools: build_anthropic_tools(tools)?,
        thinking,
    })
}

fn anthropic_user_content(message: &ConversationMessage) -> Vec<AnthropicRequestContent> {
    message
        .effective_parts()
        .into_iter()
        .filter_map(|part| match part {
            ConversationContentPart::Text { text } => {
                (!text.is_empty()).then_some(AnthropicRequestContent::Text { text })
            }
            ConversationContentPart::ImageUrl { url, .. } => Some(AnthropicRequestContent::Image {
                source: anthropic_image_source_from_url(url),
            }),
            ConversationContentPart::LocalImage { path, detail } => {
                match load_local_image_for_prompt(&path, detail.unwrap_or_default()) {
                    Ok(encoded) => Some(AnthropicRequestContent::Image {
                        source: AnthropicImageSource::Base64 {
                            media_type: encoded.mime,
                            data: encoded.base64_data,
                        },
                    }),
                    Err(err) => Some(AnthropicRequestContent::Text {
                        text: format!("image unavailable: {err}"),
                    }),
                }
            }
        })
        .collect()
}

fn anthropic_tool_result_content(message: &ConversationMessage) -> Vec<AnthropicRequestContent> {
    let mut content = Vec::new();
    if !message.content.is_empty() {
        content.push(AnthropicRequestContent::Text {
            text: message.content.clone(),
        });
    }
    content.extend(message.parts.iter().filter_map(|part| match part {
        ConversationContentPart::Text { text } => {
            (!text.is_empty()).then(|| AnthropicRequestContent::Text { text: text.clone() })
        }
        ConversationContentPart::ImageUrl { url, .. } => Some(AnthropicRequestContent::Image {
            source: anthropic_image_source_from_url(url.clone()),
        }),
        ConversationContentPart::LocalImage { path, detail } => {
            match load_local_image_for_prompt(path, detail.unwrap_or_default()) {
                Ok(encoded) => Some(AnthropicRequestContent::Image {
                    source: AnthropicImageSource::Base64 {
                        media_type: encoded.mime,
                        data: encoded.base64_data,
                    },
                }),
                Err(err) => Some(AnthropicRequestContent::Text {
                    text: format!("image unavailable: {err}"),
                }),
            }
        }
    }));
    if content.is_empty() {
        content.push(AnthropicRequestContent::Text {
            text: "No content returned.".to_string(),
        });
    }
    content
}

fn anthropic_image_source_from_url(url: String) -> AnthropicImageSource {
    if let Some((media_type, data)) = parse_data_url(&url) {
        AnthropicImageSource::Base64 { media_type, data }
    } else {
        AnthropicImageSource::Url { url }
    }
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (metadata, data) = rest.split_once(',')?;
    if !metadata.ends_with(";base64") {
        return None;
    }
    let media_type = metadata.trim_end_matches(";base64");
    if media_type.is_empty() || data.is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

fn anthropic_reasoning_content(block: &ReasoningBlock) -> Option<AnthropicRequestContent> {
    match &block.signature {
        Some(ReasoningSignature::AnthropicSignature(signature)) => {
            Some(AnthropicRequestContent::Thinking {
                thinking: block.text.clone(),
                signature: Some(signature.clone()),
            })
        }
        Some(ReasoningSignature::AnthropicRedactedData(data)) => {
            Some(AnthropicRequestContent::RedactedThinking { data: data.clone() })
        }
        _ => None,
    }
}

fn anthropic_thinking(thinking_mode: Option<ThinkingMode>) -> Option<AnthropicThinking> {
    let budget_tokens = match thinking_mode {
        Some(ThinkingMode::Minimal | ThinkingMode::Low) => 4000,
        Some(ThinkingMode::Medium) => 8000,
        Some(ThinkingMode::High) => 16000,
        Some(ThinkingMode::XHigh) => 32000,
        None | Some(ThinkingMode::Auto | ThinkingMode::Off) => return None,
    };

    Some(AnthropicThinking {
        r#type: "enabled",
        budget_tokens,
    })
}

fn build_anthropic_tools(tools: &[ToolSpec]) -> Result<Vec<AnthropicRequestTool>> {
    tools
        .iter()
        .map(|tool| match &tool.kind {
            // Only input_schema is sent. `tool.output_schema`/`tool.strict` are
            // internal contracts; the Anthropic tools wire has no output_schema
            // field, so they are intentionally not serialized here (ADR-0042).
            ToolSpecKind::Function { input_schema } => Ok(AnthropicRequestTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: input_schema.clone(),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::types::{ConversationContentPart, ImageDetail, UserInput};

    #[test]
    fn request_serializes_user_image_url_parts() {
        let message = ConversationMessage::user_parts(vec![
            UserInput::Text {
                text: "describe".to_string(),
            },
            UserInput::ImageUrl {
                url: "https://example.com/cat.png".to_string(),
                detail: Some(ImageDetail::High),
            },
        ]);

        let request = build_anthropic_request(
            "claude-sonnet-4-5".to_string(),
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(
            value["messages"][0]["content"],
            json!([
                { "type": "text", "text": "describe" },
                {
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": "https://example.com/cat.png"
                    }
                }
            ])
        );
    }

    #[test]
    fn request_degrades_missing_local_image_to_text_part() {
        let message = ConversationMessage::user_parts(vec![UserInput::LocalImage {
            path: std::path::PathBuf::from("/tmp/definitely-missing-exagent-image.png"),
            detail: Some(ImageDetail::High),
        }]);

        let request = build_anthropic_request(
            "claude-sonnet-4-5".to_string(),
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();
        let content = value["messages"][0]["content"]
            .as_array()
            .expect("content parts");

        assert!(content.iter().any(|part| {
            part["type"] == "text"
                && part["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("image unavailable"))
        }));
    }

    #[test]
    fn request_serializes_tool_result_image_parts() {
        let message = ConversationMessage::tool_with_parts(
            "toolu_1",
            "Viewed image",
            vec![ConversationContentPart::ImageUrl {
                url: "https://example.com/tool.png".to_string(),
                detail: Some(ImageDetail::High),
            }],
        );

        let request = build_anthropic_request(
            "claude-sonnet-4-5".to_string(),
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(
            value["messages"][0]["content"],
            json!([{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": [
                    { "type": "text", "text": "Viewed image" },
                    {
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": "https://example.com/tool.png"
                        }
                    }
                ]
            }])
        );
    }

    #[test]
    fn request_serializes_empty_tool_result_as_text_block() {
        let message = ConversationMessage::tool("toolu_empty", "");

        let request = build_anthropic_request(
            "claude-sonnet-4-5".to_string(),
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(
            value["messages"][0]["content"],
            json!([{
                "type": "tool_result",
                "tool_use_id": "toolu_empty",
                "content": [
                    { "type": "text", "text": "No content returned." }
                ]
            }])
        );
    }
}
