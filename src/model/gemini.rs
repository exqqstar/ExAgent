use std::collections::HashMap;

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

pub struct GeminiLlm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl GeminiLlm {
    pub fn from_config(model: &ResolvedModelConfig) -> Result<Self> {
        let base_url = model
            .endpoint
            .base_url
            .clone()
            .context("GOOGLE_BASE_URL is required for the Gemini adapter")?;
        let api_key = match &model.credential {
            ResolvedCredential::ApiKey(value) | ResolvedCredential::BearerToken(value) => {
                value.clone()
            }
            ResolvedCredential::None => bail!("GOOGLE_API_KEY is required for Gemini"),
            ResolvedCredential::ChatGptOAuth { .. } => {
                bail!("ChatGPT OAuth cannot be used with Gemini")
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
            bail!("model is required for the Gemini adapter");
        }
        if base_url.trim().is_empty() {
            bail!("GOOGLE_BASE_URL is required for the Gemini adapter");
        }
        if api_key.trim().is_empty() {
            bail!("GOOGLE_API_KEY is required for Gemini");
        }

        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
        })
    }

    pub fn parse_response(value: Value) -> Result<LlmCompletion> {
        let response: GeminiResponse =
            serde_json::from_value(value).context("Failed to parse Gemini response")?;
        let usage = response.usage_metadata.map(TokenUsage::from);
        let candidate = response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Gemini response had no candidates"))?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut reasoning = Vec::new();
        for (index, part) in candidate.content.parts.into_iter().enumerate() {
            let thought_signature = part
                .thought_signature
                .as_ref()
                .map(|signature| serde_json::json!(signature));
            if let Some(text) = part.text {
                if part.thought {
                    reasoning.push(ReasoningBlock {
                        text,
                        signature: part
                            .thought_signature
                            .map(ReasoningSignature::GeminiThoughtSignature),
                        redacted: false,
                    });
                } else {
                    if let Some(signature) = part.thought_signature {
                        text_parts.push(text.clone());
                        reasoning.push(ReasoningBlock {
                            text,
                            signature: Some(ReasoningSignature::GeminiThoughtSignature(signature)),
                            redacted: true,
                        });
                    } else {
                        text_parts.push(text);
                    }
                }
            }
            if let Some(function_call) = part.function_call {
                tool_calls.push(ToolCall {
                    id: format!("gemini_call_{}_{}", index, function_call.name),
                    name: function_call.name,
                    arguments: function_call.args,
                    thought_signature,
                });
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
            token_usage: usage,
        })
    }
}

#[async_trait]
impl LlmClient for GeminiLlm {
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
        let request = build_gemini_request(&model, messages, tools, options)?;
        let response = self
            .client
            .post(generate_content_endpoint(&self.base_url, &model))
            .header("x-goog-api-key", &self.api_key)
            .json(&request)
            .send()
            .await
            .context("Failed to send Gemini generateContent request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read Gemini response body")?;

        if !status.is_success() {
            bail!("Gemini request failed with status {}: {}", status, body);
        }

        let value: Value =
            serde_json::from_str(&body).context("Failed to decode Gemini JSON body")?;
        Self::parse_response(value)
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_gemini_context_window_error(err)
    }
}

pub fn is_gemini_context_window_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    ["context window", "input token count", "too many tokens"]
        .iter()
        .any(|needle| message.contains(needle))
}

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    role: &'static str,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
    #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData>,
    #[serde(rename = "fileData", skip_serializing_if = "Option::is_none")]
    file_data: Option<GeminiFileData>,
}

#[derive(Debug, Serialize)]
struct GeminiInlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct GeminiFileData {
    #[serde(rename = "fileUri")]
    file_uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeminiGenerationConfig {
    #[serde(rename = "thinkingConfig")]
    thinking_config: GeminiThinkingConfig,
}

#[derive(Debug, Serialize)]
struct GeminiThinkingConfig {
    #[serde(rename = "includeThoughts")]
    include_thoughts: bool,
    #[serde(rename = "thinkingLevel", skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
    #[serde(rename = "thinkingBudget", skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionResponse {
    name: String,
    response: Value,
}

#[derive(Debug, Serialize)]
struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiResponseContent,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(rename = "thoughtSignature")]
    thought_signature: Option<String>,
    #[serde(default)]
    thought: bool,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: i64,
    #[serde(rename = "cachedContentTokenCount", default)]
    cached_content_token_count: i64,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: i64,
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: i64,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: i64,
}

impl From<GeminiUsage> for TokenUsage {
    fn from(usage: GeminiUsage) -> Self {
        Self {
            input_tokens: usage.prompt_token_count,
            cached_input_tokens: usage.cached_content_token_count,
            output_tokens: usage.candidates_token_count,
            reasoning_output_tokens: usage.thoughts_token_count,
            total_tokens: usage.total_token_count,
        }
    }
}

fn generate_content_endpoint(base_url: &str, model: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with(":generateContent") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/models/{model}:generateContent")
    }
}

fn build_gemini_request(
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    options: &LlmRequestOptions,
) -> Result<GeminiRequest> {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for message in messages {
        match message.role {
            MessageRole::System => {
                system_parts.push(GeminiPart::text(message.content.clone(), None))
            }
            MessageRole::User => contents.push(GeminiContent {
                role: "user",
                parts: gemini_user_parts(message),
            }),
            MessageRole::Assistant => {
                let mut parts = Vec::new();
                let visible_signed_text_blocks = message
                    .reasoning
                    .iter()
                    .filter(|block| block.redacted && !block.text.is_empty())
                    .collect::<Vec<_>>();
                if !visible_signed_text_blocks.is_empty() {
                    push_gemini_visible_text_parts(
                        &mut parts,
                        &message.content,
                        &visible_signed_text_blocks,
                    );
                } else if !message.content.is_empty() {
                    parts.push(GeminiPart::text(
                        message.content.clone(),
                        message
                            .reasoning
                            .iter()
                            .find_map(gemini_visible_text_signature),
                    ));
                }
                for block in &message.reasoning {
                    if !block.redacted && !block.text.is_empty() {
                        parts.push(GeminiPart::thought_text(
                            block.text.clone(),
                            gemini_thought_signature(block),
                        ));
                    }
                }
                for tool_call in &message.tool_calls {
                    tool_names_by_id.insert(tool_call.id.clone(), tool_call.name.clone());
                    parts.push(GeminiPart::function_call(
                        tool_call.name.clone(),
                        tool_call.arguments.clone(),
                        tool_call
                            .thought_signature
                            .as_ref()
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    ));
                }
                if parts.is_empty() {
                    return Err(anyhow!("Assistant messages require text or tool calls"));
                }
                contents.push(GeminiContent {
                    role: "model",
                    parts,
                });
            }
            MessageRole::Tool => {
                let tool_call_id = message
                    .tool_call_id
                    .clone()
                    .ok_or_else(|| anyhow!("Tool messages require tool_call_id"))?;
                let name = tool_names_by_id
                    .get(&tool_call_id)
                    .cloned()
                    .unwrap_or(tool_call_id);
                contents.push(GeminiContent {
                    role: "user",
                    parts: vec![GeminiPart::function_response(name, message.content.clone())],
                });
            }
        }
    }

    Ok(GeminiRequest {
        contents,
        system_instruction: (!system_parts.is_empty()).then_some(GeminiSystemInstruction {
            parts: system_parts,
        }),
        tools: build_gemini_tools(tools)?,
        generation_config: gemini_generation_config(model, options.thinking_mode),
    })
}

fn push_gemini_visible_text_parts(
    parts: &mut Vec<GeminiPart>,
    content: &str,
    signed_blocks: &[&ReasoningBlock],
) {
    let mut remaining = content;
    for block in signed_blocks {
        if let Some(index) = remaining.find(&block.text) {
            push_gemini_unsigned_text_part(parts, &remaining[..index]);
            parts.push(GeminiPart::text(
                block.text.clone(),
                gemini_thought_signature(block),
            ));
            remaining = &remaining[index + block.text.len()..];
            remaining = remaining.strip_prefix('\n').unwrap_or(remaining);
        } else {
            parts.push(GeminiPart::text(
                block.text.clone(),
                gemini_thought_signature(block),
            ));
        }
    }
    push_gemini_unsigned_text_part(parts, remaining);
}

fn push_gemini_unsigned_text_part(parts: &mut Vec<GeminiPart>, text: &str) {
    let text = text.trim_matches('\n');
    if !text.is_empty() {
        parts.push(GeminiPart::text(text.to_string(), None));
    }
}

fn gemini_user_parts(message: &ConversationMessage) -> Vec<GeminiPart> {
    message
        .effective_parts()
        .into_iter()
        .filter_map(|part| match part {
            ConversationContentPart::Text { text } => {
                (!text.is_empty()).then(|| GeminiPart::text(text, None))
            }
            ConversationContentPart::ImageUrl { url, .. } => Some(gemini_image_url_part(url)),
            ConversationContentPart::LocalImage { path, detail } => {
                match load_local_image_for_prompt(&path, detail.unwrap_or_default()) {
                    Ok(encoded) => Some(GeminiPart::inline_data(encoded.mime, encoded.base64_data)),
                    Err(err) => Some(GeminiPart::text(format!("image unavailable: {err}"), None)),
                }
            }
        })
        .collect()
}

fn gemini_image_url_part(url: String) -> GeminiPart {
    if let Some((mime_type, data)) = parse_data_url(&url) {
        GeminiPart::inline_data(mime_type, data)
    } else {
        GeminiPart::file_data(url, None)
    }
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (metadata, data) = rest.split_once(',')?;
    if !metadata.ends_with(";base64") {
        return None;
    }
    let mime_type = metadata.trim_end_matches(";base64");
    if mime_type.is_empty() || data.is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

impl GeminiPart {
    fn text(text: String, thought_signature: Option<String>) -> Self {
        Self {
            text: Some(text),
            function_call: None,
            function_response: None,
            thought_signature,
            thought: None,
            inline_data: None,
            file_data: None,
        }
    }

    fn thought_text(text: String, thought_signature: Option<String>) -> Self {
        Self {
            text: Some(text),
            function_call: None,
            function_response: None,
            thought_signature,
            thought: Some(true),
            inline_data: None,
            file_data: None,
        }
    }

    fn function_call(name: String, args: Value, thought_signature: Option<String>) -> Self {
        Self {
            text: None,
            function_call: Some(GeminiFunctionCall { name, args }),
            function_response: None,
            thought_signature,
            thought: None,
            inline_data: None,
            file_data: None,
        }
    }

    fn function_response(name: String, content: String) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: Some(GeminiFunctionResponse {
                name,
                response: serde_json::json!({ "content": content }),
            }),
            thought_signature: None,
            thought: None,
            inline_data: None,
            file_data: None,
        }
    }

    fn inline_data(mime_type: String, data: String) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: None,
            thought_signature: None,
            thought: None,
            inline_data: Some(GeminiInlineData { mime_type, data }),
            file_data: None,
        }
    }

    fn file_data(file_uri: String, mime_type: Option<String>) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: None,
            thought_signature: None,
            thought: None,
            inline_data: None,
            file_data: Some(GeminiFileData {
                file_uri,
                mime_type,
            }),
        }
    }
}

fn gemini_generation_config(
    model: &str,
    thinking_mode: Option<ThinkingMode>,
) -> Option<GeminiGenerationConfig> {
    if model.starts_with("gemini-2.5") {
        let thinking_budget = match thinking_mode {
            Some(ThinkingMode::Off) if model.starts_with("gemini-2.5-pro") => return None,
            Some(ThinkingMode::Off) => 0,
            Some(ThinkingMode::Minimal | ThinkingMode::Low) => 1_024,
            Some(ThinkingMode::Medium) => 8_192,
            Some(ThinkingMode::High | ThinkingMode::XHigh) => 16_384,
            None | Some(ThinkingMode::Auto) => return None,
        };

        return Some(GeminiGenerationConfig {
            thinking_config: GeminiThinkingConfig {
                include_thoughts: true,
                thinking_level: None,
                thinking_budget: Some(thinking_budget),
            },
        });
    }

    let thinking_level = match thinking_mode {
        Some(ThinkingMode::Off | ThinkingMode::Minimal) => "minimal",
        Some(ThinkingMode::Low) => "low",
        Some(ThinkingMode::Medium) => "medium",
        Some(ThinkingMode::High | ThinkingMode::XHigh) => "high",
        None | Some(ThinkingMode::Auto) => return None,
    };

    Some(GeminiGenerationConfig {
        thinking_config: GeminiThinkingConfig {
            include_thoughts: true,
            thinking_level: Some(thinking_level),
            thinking_budget: None,
        },
    })
}

fn gemini_thought_signature(block: &ReasoningBlock) -> Option<String> {
    match &block.signature {
        Some(ReasoningSignature::GeminiThoughtSignature(signature)) => Some(signature.clone()),
        _ => None,
    }
}

fn gemini_visible_text_signature(block: &ReasoningBlock) -> Option<String> {
    block
        .redacted
        .then(|| gemini_thought_signature(block))
        .flatten()
}

fn build_gemini_tools(tools: &[ToolSpec]) -> Result<Vec<GeminiTool>> {
    if tools.is_empty() {
        return Ok(Vec::new());
    }
    let function_declarations = tools
        .iter()
        .map(|tool| match &tool.kind {
            ToolSpecKind::Function { input_schema } => Ok(GeminiFunctionDeclaration {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: input_schema.clone(),
            }),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![GeminiTool {
        function_declarations,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::types::{ImageDetail, UserInput};

    #[test]
    fn request_serializes_user_image_url_parts() {
        let message = ConversationMessage::user_parts(vec![
            UserInput::Text {
                text: "describe".to_string(),
            },
            UserInput::ImageUrl {
                url: "data:image/png;base64,AAA".to_string(),
                detail: Some(ImageDetail::High),
            },
        ]);

        let request = build_gemini_request(
            "gemini-3-pro-preview",
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(
            value["contents"][0]["parts"],
            json!([
                { "text": "describe" },
                { "inlineData": { "mimeType": "image/png", "data": "AAA" } }
            ])
        );
    }

    #[test]
    fn request_degrades_missing_local_image_to_text_part() {
        let message = ConversationMessage::user_parts(vec![UserInput::LocalImage {
            path: std::path::PathBuf::from("/tmp/definitely-missing-exagent-image.png"),
            detail: Some(ImageDetail::High),
        }]);

        let request = build_gemini_request(
            "gemini-3-pro-preview",
            &[message],
            &[],
            &LlmRequestOptions::default(),
        )
        .unwrap();
        let value = serde_json::to_value(request).unwrap();
        let parts = value["contents"][0]["parts"].as_array().expect("parts");

        assert!(parts.iter().any(|part| {
            part["text"]
                .as_str()
                .is_some_and(|text| text.contains("image unavailable"))
        }));
    }
}
