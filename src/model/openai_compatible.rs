use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::llm::{LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink};
use super::openai_reasoning::{
    apply_openai_reasoning, apply_reasoning_replay, extract_openai_reasoning_blocks,
};
use super::reasoning::{ReasoningCapabilities, ReasoningProtocol};
use super::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::config::ThinkingMode;
use crate::model::image_input::load_local_image_for_prompt;
use crate::tools::{ToolSpec, ToolSpecKind};
use crate::types::{
    AssistantTurn, ConversationContentPart, ConversationMessage, ImageDetail, LlmCompletion,
    MessageRole, TokenUsage, ToolCall,
};

pub struct OpenAiCompatibleLlm {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    model: String,
    reasoning_capabilities: ReasoningCapabilities,
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
            ResolvedCredential::ChatGptOAuth { .. } => {
                bail!("ChatGPT OAuth requires the ChatGPT Codex adapter")
            }
        };

        let mut llm = Self::from_parts(model.identity.model_id.clone(), base_url, api_key)?;
        llm.reasoning_capabilities = model.capabilities.reasoning.clone();
        Ok(llm)
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
            reasoning_capabilities: default_openai_compatible_reasoning_capabilities(),
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

        let reasoning = extract_openai_reasoning_blocks(&choice.message);
        let message: ChatResponseMessage = serde_json::from_value(choice.message)
            .context("Failed to parse chat completion response message")?;

        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tool_call| {
                let arguments = tool_call.function.arguments.into_value(&tool_call.id)?;
                Ok(ToolCall {
                    id: tool_call.id,
                    name: tool_call.function.name,
                    arguments,
                    thought_signature: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(LlmCompletion {
            turn: AssistantTurn {
                text: message.content,
                tool_calls,
                reasoning,
            },
            token_usage,
        })
    }

    fn build_request_value(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
        stream: bool,
    ) -> Result<Value> {
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
        build_openai_chat_completion_request_value(
            request_model,
            messages,
            tools,
            options,
            reasoning_capabilities,
            stream,
        )
    }

    async fn post_request(&self, request: Value) -> Result<reqwest::Response> {
        let mut request_builder = self.client.post(&self.endpoint).json(&request);
        if let Some(api_key) = self.api_key.as_deref() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder
            .send()
            .await
            .context("Failed to send chat completion request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .context("Failed to read chat completion error body")?;
            bail!(
                "OpenAI-compatible request failed with status {}: {}",
                status,
                body
            );
        }

        Ok(response)
    }
}

pub(crate) fn build_openai_chat_completion_request_value(
    request_model: String,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    options: &LlmRequestOptions,
    reasoning_capabilities: &ReasoningCapabilities,
    stream: bool,
) -> Result<Value> {
    let request = build_chat_completion_request(
        request_model,
        messages,
        tools,
        options,
        reasoning_capabilities,
    )?;
    let mut request =
        serde_json::to_value(request).context("Failed to encode chat completion request")?;
    apply_openai_reasoning(&mut request, reasoning_capabilities, options.thinking_mode);
    if stream {
        request["stream"] = Value::Bool(true);
        request["stream_options"] = json!({ "include_usage": true });
    }
    log_debug_request_shape(&request);
    Ok(request)
}

fn log_debug_request_shape(request: &Value) {
    if std::env::var_os("EXAGENT_DEBUG_LLM_REQUEST").is_none() {
        return;
    }

    let tool_names = request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    tool.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let has_subagent_guidance = request
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| {
                        content.contains("Subagent collaboration tools are available")
                    })
            })
        });
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");

    eprintln!(
        "exagent debug llm request model={} subagent_guidance={} tools={}",
        model,
        has_subagent_guidance,
        tool_names.join(",")
    );
}

#[async_trait]
impl LlmClient for OpenAiCompatibleLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let request = self.build_request_value(messages, tools, options, false)?;
        let response = self.post_request(request).await?;
        let body = response
            .text()
            .await
            .context("Failed to read chat completion response body")?;

        let value: Value =
            serde_json::from_str(&body).context("Failed to decode chat completion JSON body")?;
        Self::parse_response(value)
    }

    async fn stream(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmCompletion> {
        let request = self.build_request_value(messages, tools, options, true)?;
        let response = self.post_request(request).await?;
        stream_chat_completion_response(response, sink).await
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_openai_context_window_error(err)
    }
}

async fn stream_chat_completion_response(
    response: reqwest::Response,
    sink: &mut dyn LlmStreamSink,
) -> Result<LlmCompletion> {
    let mut parser = ChatCompletionStreamParser::default();
    let mut bytes = response.bytes_stream();
    let mut buffer = String::new();
    let mut pending_utf8 = Vec::new();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.context("Failed to read chat completion stream chunk")?;
        append_utf8_chunk(&mut pending_utf8, &chunk, &mut buffer)?;

        while let Some((frame_end, delimiter_len)) = next_sse_frame_boundary(&buffer) {
            let frame = buffer[..frame_end].to_string();
            buffer.drain(..frame_end + delimiter_len);
            if parser.process_frame(&frame, sink).await? {
                let completion = parser.finish()?;
                sink.event(LlmStreamEvent::Completed(completion.clone()))
                    .await?;
                return Ok(completion);
            }
        }
    }

    if !pending_utf8.is_empty() {
        append_utf8_chunk(&mut pending_utf8, &[], &mut buffer)?;
        if !pending_utf8.is_empty() {
            bail!("Chat completion stream ended with incomplete UTF-8 sequence");
        }
    }

    if !buffer.trim().is_empty() {
        parser.process_frame(&buffer, sink).await?;
    }

    let completion = parser.finish()?;
    sink.event(LlmStreamEvent::Completed(completion.clone()))
        .await?;
    Ok(completion)
}

fn append_utf8_chunk(pending: &mut Vec<u8>, chunk: &[u8], buffer: &mut String) -> Result<()> {
    pending.extend_from_slice(chunk);
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                buffer.push_str(text);
                pending.clear();
                return Ok(());
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let text = std::str::from_utf8(&pending[..valid_up_to])
                        .context("Validated UTF-8 prefix unexpectedly failed to decode")?;
                    buffer.push_str(text);
                    pending.drain(..valid_up_to);
                    continue;
                }
                if error.error_len().is_none() {
                    return Ok(());
                }
                bail!("Chat completion stream chunk was not valid UTF-8");
            }
        }
    }
}

fn next_sse_frame_boundary(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf < lf => Some((crlf, 4)),
        (Some(lf), _) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

#[derive(Default)]
struct ChatCompletionStreamParser {
    assistant_text: String,
    reasoning_text: String,
    reasoning_field: Option<String>,
    token_usage: Option<TokenUsage>,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    done: bool,
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatCompletionStreamParser {
    async fn process_frame(&mut self, frame: &str, sink: &mut dyn LlmStreamSink) -> Result<bool> {
        for data in sse_data_lines(frame) {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                self.done = true;
                return Ok(true);
            }

            let chunk: ChatCompletionStreamChunk = serde_json::from_str(data)
                .context("Failed to decode chat completion stream JSON")?;
            if let Some(usage) = chunk.usage {
                self.token_usage = Some(TokenUsage::from(usage));
            }

            for choice in chunk.choices {
                let ChatCompletionStreamChoice {
                    delta,
                    finish_reason,
                    usage,
                } = choice;
                if let Some(usage) = usage {
                    self.token_usage = Some(TokenUsage::from(usage));
                }
                self.apply_delta(delta, sink).await?;
                if finish_reason.is_some() {
                    self.done = true;
                }
            }
        }

        Ok(self.done)
    }

    async fn apply_delta(
        &mut self,
        delta: ChatCompletionDelta,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<()> {
        for (field, reasoning) in delta.reasoning_fields() {
            if self.reasoning_field.is_none() {
                self.reasoning_field = Some(field.to_string());
            }
            self.reasoning_text.push_str(&reasoning);
            sink.event(LlmStreamEvent::ReasoningDelta(reasoning))
                .await?;
        }

        if let Some(content) = delta.content.filter(|value| !value.is_empty()) {
            self.assistant_text.push_str(&content);
            sink.event(LlmStreamEvent::AssistantTextDelta(content))
                .await?;
        }

        for tool_call in delta.tool_calls.unwrap_or_default() {
            let partial = self.tool_calls.entry(tool_call.index).or_default();
            if let Some(id) = tool_call.id {
                partial.id = Some(id);
            }
            if let Some(function) = tool_call.function {
                if let Some(name) = function.name {
                    partial.name = Some(name);
                }
                if let Some(arguments) = function.arguments {
                    partial.arguments.push_str(&arguments);
                }
            }
        }

        Ok(())
    }

    fn finish(self) -> Result<LlmCompletion> {
        let text = if self.assistant_text.is_empty() {
            None
        } else {
            Some(self.assistant_text)
        };
        let reasoning = if self.reasoning_text.is_empty() {
            Vec::new()
        } else {
            vec![crate::types::ReasoningBlock {
                text: self.reasoning_text,
                signature: Some(crate::types::ReasoningSignature::OpenAiField {
                    field: self
                        .reasoning_field
                        .unwrap_or_else(|| "reasoning_content".to_string()),
                }),
                redacted: false,
            }]
        };
        let tool_calls = self
            .tool_calls
            .into_iter()
            .map(|(index, partial)| {
                let id = partial.id.unwrap_or_else(|| format!("call_stream_{index}"));
                let name = partial.name.with_context(|| {
                    format!("Streamed tool call {id} did not include function name")
                })?;
                let arguments = if partial.arguments.trim().is_empty() {
                    Value::Object(Map::new())
                } else {
                    serde_json::from_str(&partial.arguments).with_context(|| {
                        format!("Streamed tool call {id} returned invalid JSON arguments")
                    })?
                };
                Ok(ToolCall {
                    id,
                    name,
                    arguments,
                    thought_signature: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(LlmCompletion {
            turn: AssistantTurn {
                text,
                tool_calls,
                reasoning,
            },
            token_usage: self.token_usage,
        })
    }
}

fn sse_data_lines(frame: &str) -> Vec<String> {
    frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.trim_start().to_string())
        .collect()
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamChunk {
    #[serde(default)]
    choices: Vec<ChatCompletionStreamChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamChoice {
    #[serde(default)]
    delta: ChatCompletionDelta,
    finish_reason: Option<Value>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_text: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatCompletionToolCallDelta>>,
}

impl ChatCompletionDelta {
    fn reasoning_fields(&self) -> Vec<(&'static str, String)> {
        [
            ("reasoning_content", self.reasoning_content.as_ref()),
            ("reasoning", self.reasoning.as_ref()),
            ("reasoning_text", self.reasoning_text.as_ref()),
        ]
        .into_iter()
        .filter_map(|(field, text)| {
            text.filter(|value| !value.is_empty())
                .map(|value| (field, value.clone()))
        })
        .collect()
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<ChatCompletionToolCallFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCallFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
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
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatRequestTool>,
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
    message: Value,
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
    arguments: ChatResponseToolCallArguments,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChatResponseToolCallArguments {
    String(String),
    Value(Value),
}

impl ChatResponseToolCallArguments {
    fn into_value(self, tool_call_id: &str) -> Result<Value> {
        match self {
            Self::String(arguments) => serde_json::from_str(&arguments).with_context(|| {
                format!("Tool call {tool_call_id} returned invalid JSON arguments")
            }),
            Self::Value(arguments) => Ok(arguments),
        }
    }
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

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn default_openai_compatible_reasoning_capabilities() -> ReasoningCapabilities {
    ReasoningCapabilities {
        protocol: ReasoningProtocol::OpenAiReasoningEffort,
        supported_modes: vec![ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::High],
        default_mode: None,
        mode_map: BTreeMap::new(),
        requires_assistant_reasoning_content: false,
    }
}

fn build_chat_completion_request(
    model: String,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    options: &LlmRequestOptions,
    reasoning_capabilities: &ReasoningCapabilities,
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
        messages: build_request_messages(
            messages,
            reasoning_capabilities.requires_assistant_reasoning_content,
        )?,
        tools: build_request_tools(tools)?,
    })
}

fn build_request_tools(tools: &[ToolSpec]) -> Result<Vec<ChatRequestTool>> {
    tools
        .iter()
        .map(|tool| match &tool.kind {
            ToolSpecKind::Function { input_schema } => Ok(ChatRequestTool {
                kind: "function",
                function: ChatRequestFunction {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: input_schema.clone(),
                },
            }),
        })
        .collect()
}

fn build_request_messages(
    messages: &[ConversationMessage],
    requires_empty_reasoning_content: bool,
) -> Result<Vec<Value>> {
    let mut request_messages = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => request_messages.push(json!({
                "role": "system",
                "content": message.content,
            })),
            MessageRole::User => request_messages.push(json!({
                "role": "user",
                "content": build_user_content(message),
            })),
            MessageRole::Assistant => {
                let mut assistant = Map::new();
                assistant.insert("role".to_string(), json!("assistant"));
                if !message.content.is_empty() {
                    assistant.insert("content".to_string(), json!(message.content));
                }
                if !message.tool_calls.is_empty() {
                    assistant.insert(
                        "tool_calls".to_string(),
                        serde_json::to_value(
                            message
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
                                .collect::<Vec<_>>(),
                        )
                        .context("Failed to encode assistant tool calls")?,
                    );
                }
                let mut value = Value::Object(assistant);
                apply_reasoning_replay(
                    &mut value,
                    &message.reasoning,
                    requires_empty_reasoning_content,
                );
                request_messages.push(value);
            }
            MessageRole::Tool => {
                let tool_call_id = message
                    .tool_call_id
                    .clone()
                    .ok_or_else(|| anyhow!("Tool messages require tool_call_id"))?;
                request_messages.push(json!({
                    "role": "tool",
                    "content": message.content,
                    "tool_call_id": tool_call_id,
                }));
                if let Some(image_message) = tool_result_image_user_message(&tool_call_id, message)
                {
                    request_messages.push(json!({
                        "role": "user",
                        "content": build_user_content(&image_message),
                    }));
                }
            }
        }
    }
    Ok(request_messages)
}

fn build_user_content(message: &ConversationMessage) -> Value {
    let parts = message.effective_parts();
    if parts
        .iter()
        .all(|part| matches!(part, ConversationContentPart::Text { .. }))
    {
        return json!(message.content);
    }

    let content = parts
        .into_iter()
        .filter_map(|part| match part {
            ConversationContentPart::Text { text } => {
                (!text.is_empty()).then(|| json!({ "type": "text", "text": text }))
            }
            ConversationContentPart::ImageUrl { url, detail } => {
                Some(openai_image_url_part(url, detail))
            }
            ConversationContentPart::LocalImage { path, detail } => {
                match load_local_image_for_prompt(&path, detail.unwrap_or_default()) {
                    Ok(encoded) => Some(openai_image_url_part(encoded.data_url, detail)),
                    Err(err) => Some(json!({
                        "type": "text",
                        "text": format!("image unavailable: {err}")
                    })),
                }
            }
        })
        .collect::<Vec<_>>();

    Value::Array(content)
}

fn tool_result_image_user_message(
    tool_call_id: &str,
    message: &ConversationMessage,
) -> Option<ConversationMessage> {
    let mut image_parts = message
        .parts
        .iter()
        .filter(|part| part.is_image())
        .cloned()
        .collect::<Vec<_>>();
    if image_parts.is_empty() {
        return None;
    }
    let marker = format!("[image from tool result {tool_call_id}]");
    let mut parts = vec![ConversationContentPart::Text {
        text: marker.clone(),
    }];
    parts.append(&mut image_parts);
    Some(ConversationMessage {
        role: MessageRole::User,
        content: marker,
        parts,
        tool_call_id: None,
        tool_calls: vec![],
        reasoning: vec![],
        injected: true,
        internal_source: Some("tool_result_image".to_string()),
    })
}

fn openai_image_url_part(url: String, detail: Option<ImageDetail>) -> Value {
    let mut image_url = Map::new();
    image_url.insert("url".to_string(), json!(url));
    if let Some(detail) = detail.and_then(openai_image_detail) {
        image_url.insert("detail".to_string(), json!(detail));
    }
    json!({
        "type": "image_url",
        "image_url": Value::Object(image_url),
    })
}

fn openai_image_detail(detail: ImageDetail) -> Option<&'static str> {
    match detail {
        ImageDetail::Auto | ImageDetail::Original => Some("auto"),
        ImageDetail::Low => Some("low"),
        ImageDetail::High => Some("high"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThinkingMode;
    use crate::tools::ToolSpec;
    use crate::types::{ConversationContentPart, ImageDetail, UserInput};

    #[test]
    fn chat_completion_request_keeps_reasoning_fields_out_of_base_dto() {
        let request = build_chat_completion_request(
            "gpt-thinking".to_string(),
            &[],
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: Some(ThinkingMode::High),
                reasoning_capabilities: None,
            },
            &default_openai_compatible_reasoning_capabilities(),
        )
        .unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("reasoning_effort").is_none());
        assert!(value.get("thinking").is_none());
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn chat_completion_request_uses_per_turn_model_when_set() {
        let options = LlmRequestOptions {
            model: Some("override-model".to_string()),
            thinking_mode: None,
            reasoning_capabilities: None,
        };

        let request = build_chat_completion_request(
            "base-model".to_string(),
            &[],
            &[],
            &options,
            &default_openai_compatible_reasoning_capabilities(),
        )
        .unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["model"], "override-model");
    }

    #[test]
    fn chat_completion_request_serializes_function_tools_from_typed_specs() {
        let tools = vec![ToolSpec::function(
            "read_file",
            "Read a file from the workspace",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        )];

        let request = build_chat_completion_request(
            "gpt-tools".to_string(),
            &[],
            &tools,
            &LlmRequestOptions::default(),
            &default_openai_compatible_reasoning_capabilities(),
        )
        .unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["function"]["name"], "read_file");
        assert_eq!(
            value["tools"][0]["function"]["description"],
            "Read a file from the workspace"
        );
        assert_eq!(
            value["tools"][0]["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[test]
    fn chat_completion_request_serializes_user_image_url_parts() {
        let message = ConversationMessage::user_parts(vec![
            UserInput::Text {
                text: "describe".to_string(),
            },
            UserInput::ImageUrl {
                url: "data:image/png;base64,AAA".to_string(),
                detail: Some(ImageDetail::High),
            },
        ]);

        let messages = build_request_messages(&[message], false).unwrap();

        assert_eq!(messages[0]["role"], "user");
        assert_eq!(
            messages[0]["content"],
            json!([
                { "type": "text", "text": "describe" },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "data:image/png;base64,AAA",
                        "detail": "high"
                    }
                }
            ])
        );
    }

    #[test]
    fn chat_completion_request_degrades_missing_local_image_to_text_part() {
        let message = ConversationMessage::user_parts(vec![UserInput::LocalImage {
            path: std::path::PathBuf::from("/tmp/definitely-missing-exagent-image.png"),
            detail: Some(ImageDetail::High),
        }]);

        let messages = build_request_messages(&[message], false).unwrap();

        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().expect("content parts");
        assert!(content.iter().any(|part| {
            part["type"] == "text"
                && part["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("image unavailable"))
        }));
    }

    #[test]
    fn chat_completion_request_injects_tool_result_image_parts() {
        let message = ConversationMessage::tool_with_parts(
            "call_1",
            "Viewed image",
            vec![ConversationContentPart::ImageUrl {
                url: "data:image/png;base64,AAA".to_string(),
                detail: Some(ImageDetail::High),
            }],
        );

        let messages = build_request_messages(&[message], false).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0],
            json!({
                "role": "tool",
                "content": "Viewed image",
                "tool_call_id": "call_1"
            })
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(
            messages[1]["content"],
            json!([
                { "type": "text", "text": "[image from tool result call_1]" },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "data:image/png;base64,AAA",
                        "detail": "high"
                    }
                }
            ])
        );
    }
}
