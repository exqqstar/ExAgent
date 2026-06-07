use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::llm::{LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink};
use super::openai_compatible::is_openai_context_window_error;
use super::reasoning::ReasoningCapabilities;
use super::resolved::{ResolvedCredential, ResolvedModelConfig};
use crate::config::ThinkingMode;
use crate::tools::{ToolSpec, ToolSpecKind};
use crate::types::{
    AssistantTurn, ConversationMessage, LlmCompletion, MessageRole, ReasoningBlock, TokenUsage,
    ToolCall,
};

const CHATGPT_CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const CHATGPT_ISSUER: &str = "https://auth.openai.com";
const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub struct ChatGptCodexLlm {
    client: reqwest::Client,
    endpoint: String,
    issuer: String,
    model: String,
    account_id: Option<String>,
    credential_id: Option<String>,
    reasoning_capabilities: ReasoningCapabilities,
    tokens: Mutex<ChatGptCodexTokens>,
    refresh_lock: Mutex<()>,
    token_refresh_sink: Option<Arc<dyn ChatGptCodexTokenRefreshSink>>,
}

#[derive(Debug, Clone)]
struct ChatGptCodexTokens {
    access_token: String,
    refresh_token: String,
    expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptCodexTokenUpdate {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: Option<u64>,
    pub account_id: Option<String>,
    pub credential_id: Option<String>,
}

#[async_trait]
pub trait ChatGptCodexTokenRefreshSink: Send + Sync {
    async fn save_chatgpt_codex_tokens(&self, update: ChatGptCodexTokenUpdate) -> Result<()>;
}

impl ChatGptCodexLlm {
    pub fn from_config(model: &ResolvedModelConfig) -> Result<Self> {
        Self::from_config_with_token_refresh_sink(model, None)
    }

    pub fn from_config_with_token_refresh_sink(
        model: &ResolvedModelConfig,
        token_refresh_sink: Option<Arc<dyn ChatGptCodexTokenRefreshSink>>,
    ) -> Result<Self> {
        Self::from_parts_with_token_refresh_sink(
            model.identity.model_id.clone(),
            CHATGPT_CODEX_ENDPOINT,
            CHATGPT_ISSUER,
            model.credential.clone(),
            token_refresh_sink,
        )
        .map(|mut llm| {
            llm.reasoning_capabilities = model.capabilities.reasoning.clone();
            llm
        })
    }

    pub fn from_parts(
        model: impl Into<String>,
        endpoint: impl Into<String>,
        issuer: impl Into<String>,
        credential: ResolvedCredential,
    ) -> Result<Self> {
        Self::from_parts_with_token_refresh_sink(model, endpoint, issuer, credential, None)
    }

    pub fn from_parts_with_token_refresh_sink(
        model: impl Into<String>,
        endpoint: impl Into<String>,
        issuer: impl Into<String>,
        credential: ResolvedCredential,
        token_refresh_sink: Option<Arc<dyn ChatGptCodexTokenRefreshSink>>,
    ) -> Result<Self> {
        let model = model.into();
        if model.trim().is_empty() {
            bail!("model is required for the ChatGPT Codex adapter");
        }
        let ResolvedCredential::ChatGptOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
            account_id,
            credential_id,
        } = credential
        else {
            bail!("ChatGPT Codex adapter requires ChatGPT OAuth credentials");
        };

        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            issuer: issuer.into().trim_end_matches('/').to_string(),
            model,
            account_id,
            credential_id,
            reasoning_capabilities: ReasoningCapabilities::unsupported(),
            tokens: Mutex::new(ChatGptCodexTokens {
                access_token,
                refresh_token,
                expires_at_ms,
            }),
            refresh_lock: Mutex::new(()),
            token_refresh_sink,
        })
    }

    async fn post_request(&self, request: Value) -> Result<(reqwest::Response, String)> {
        let access_token = self.tokens.lock().await.access_token.clone();
        let mut builder = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&access_token)
            .header("originator", "exagent")
            .json(&request);
        if let Some(account_id) = self.account_id.as_deref() {
            builder = builder.header("ChatGPT-Account-ID", account_id);
        }
        let response = builder
            .send()
            .await
            .context("Failed to send ChatGPT Codex request")?;
        Ok((response, access_token))
    }

    async fn post_request_with_auth_retry(&self, request: Value) -> Result<reqwest::Response> {
        let (mut response, access_token) = self.post_request(request.clone()).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh_access_token_if_current(Some(&access_token))
                .await?;
            response = self.post_request(request).await?.0;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .context("Failed to read ChatGPT Codex error body")?;
            bail!(
                "ChatGPT Codex request failed with status {}: {}",
                status,
                body
            );
        }

        Ok(response)
    }

    async fn refresh_if_expired(&self) -> Result<()> {
        let expired = self
            .tokens
            .lock()
            .await
            .expires_at_ms
            .is_some_and(|expires_at_ms| expires_at_ms <= now_millis());
        if expired {
            self.refresh_access_token_if_current(None).await?;
        }
        Ok(())
    }

    async fn refresh_access_token_if_current(
        &self,
        current_access_token: Option<&str>,
    ) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let refresh_token = {
            let tokens = self.tokens.lock().await;
            if current_access_token.is_some_and(|access_token| tokens.access_token != access_token)
            {
                return Ok(());
            }
            if current_access_token.is_none()
                && tokens
                    .expires_at_ms
                    .is_some_and(|expires_at_ms| expires_at_ms > now_millis())
            {
                return Ok(());
            }
            tokens.refresh_token.clone()
        };
        let response = self
            .client
            .post(format!("{}/oauth/token", self.issuer))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", CHATGPT_CLIENT_ID),
            ])
            .send()
            .await
            .context("Failed to refresh ChatGPT access token")?;
        let token_response: RefreshTokenResponse = response
            .error_for_status()
            .context("ChatGPT access token refresh failed")?
            .json()
            .await
            .context("Failed to decode ChatGPT refresh response")?;

        let update = {
            let mut tokens = self.tokens.lock().await;
            tokens.access_token = token_response.access_token;
            tokens.refresh_token = token_response.refresh_token.unwrap_or(refresh_token);
            tokens.expires_at_ms = token_response
                .expires_in
                .map(|seconds| now_millis() + seconds * 1000);
            ChatGptCodexTokenUpdate {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens.refresh_token.clone(),
                expires_at_ms: tokens.expires_at_ms,
                account_id: self.account_id.clone(),
                credential_id: self.credential_id.clone(),
            }
        };
        if let Some(sink) = self.token_refresh_sink.as_ref() {
            sink.save_chatgpt_codex_tokens(update)
                .await
                .context("Failed to persist refreshed ChatGPT OAuth tokens")?;
        }
        Ok(())
    }
}

#[async_trait]
impl LlmClient for ChatGptCodexLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.refresh_if_expired().await?;
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
        let request = build_chatgpt_codex_responses_request(
            request_model,
            messages,
            tools,
            options,
            reasoning_capabilities,
            false,
        )?;

        let response = self.post_request_with_auth_retry(request).await?;
        let body = response
            .text()
            .await
            .context("Failed to read ChatGPT Codex response body")?;
        let value: Value =
            serde_json::from_str(&body).context("Failed to decode ChatGPT Codex JSON body")?;
        parse_chatgpt_codex_response(value)
    }

    async fn stream(
        &self,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        options: &LlmRequestOptions,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmCompletion> {
        self.refresh_if_expired().await?;
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
        let request = build_chatgpt_codex_responses_request(
            request_model,
            messages,
            tools,
            options,
            reasoning_capabilities,
            true,
        )?;
        let response = self.post_request_with_auth_retry(request).await?;
        stream_chatgpt_codex_response(response, sink).await
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        is_openai_context_window_error(err)
    }
}

fn build_chatgpt_codex_responses_request(
    request_model: String,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    options: &LlmRequestOptions,
    reasoning_capabilities: &ReasoningCapabilities,
    stream: bool,
) -> Result<Value> {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => instructions.push(message.content.clone()),
            MessageRole::User => input.push(json!({
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": message.content }],
            })),
            MessageRole::Assistant => {
                if !message.content.trim().is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": message.content }],
                    }));
                }
                for tool_call in &message.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": tool_call.id,
                        "name": tool_call.name,
                        "arguments": tool_call.arguments.to_string(),
                    }));
                }
            }
            MessageRole::Tool => input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.as_deref().unwrap_or_default(),
                "output": message.content,
            })),
        }
    }

    let request_tools = tools
        .iter()
        .map(|tool| match &tool.kind {
            ToolSpecKind::Function { input_schema } => json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": input_schema,
                "strict": false,
            }),
        })
        .collect::<Vec<_>>();
    let mut request = json!({
        "model": request_model,
        "instructions": non_empty_instructions(instructions.join("\n\n")),
        "input": input,
        "tools": request_tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "store": false,
        "stream": stream,
        "include": [],
    });
    if let Some(reasoning) = responses_reasoning(options.thinking_mode, reasoning_capabilities) {
        if reasoning.get("effort").and_then(Value::as_str) != Some("none") {
            request["include"] = json!(["reasoning.encrypted_content"]);
        }
        request["reasoning"] = reasoning;
    }
    Ok(request)
}

async fn stream_chatgpt_codex_response(
    response: reqwest::Response,
    sink: &mut dyn LlmStreamSink,
) -> Result<LlmCompletion> {
    let mut parser = ChatGptCodexStreamParser::default();
    let mut bytes = response.bytes_stream();
    let mut buffer = String::new();
    let mut pending_utf8 = Vec::new();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.context("Failed to read ChatGPT Codex stream chunk")?;
        append_utf8_chunk(&mut pending_utf8, &chunk, &mut buffer)?;

        while let Some((frame_end, delimiter_len)) = next_sse_frame_boundary(&buffer) {
            let frame = buffer[..frame_end].to_string();
            buffer.drain(..frame_end + delimiter_len);
            parser.process_frame(&frame, sink).await?;
            if parser.done {
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
            bail!("ChatGPT Codex stream ended with incomplete UTF-8 sequence");
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

#[derive(Default)]
struct ChatGptCodexStreamParser {
    assistant_text: String,
    reasoning_text: String,
    completed: Option<LlmCompletion>,
    done: bool,
}

impl ChatGptCodexStreamParser {
    async fn process_frame(&mut self, frame: &str, sink: &mut dyn LlmStreamSink) -> Result<()> {
        for data in sse_data_lines(frame) {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                self.done = true;
                continue;
            }

            let value: Value =
                serde_json::from_str(data).context("Failed to decode ChatGPT Codex stream JSON")?;
            match value.get("type").and_then(Value::as_str) {
                Some("response.output_text.delta" | "response.text.delta") => {
                    if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                        if !delta.is_empty() {
                            self.assistant_text.push_str(delta);
                            sink.event(LlmStreamEvent::AssistantTextDelta(delta.to_string()))
                                .await?;
                        }
                    }
                }
                Some(
                    "response.reasoning.delta"
                    | "response.reasoning_text.delta"
                    | "response.reasoning_summary_text.delta",
                ) => {
                    if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                        if !delta.is_empty() {
                            self.reasoning_text.push_str(delta);
                            sink.event(LlmStreamEvent::ReasoningDelta(delta.to_string()))
                                .await?;
                        }
                    }
                }
                Some("response.completed") => {
                    let response = value
                        .get("response")
                        .cloned()
                        .context("ChatGPT Codex completed stream event missing response")?;
                    self.completed = Some(parse_chatgpt_codex_response(response)?);
                    self.done = true;
                }
                Some("response.failed" | "error") => {
                    bail!(
                        "ChatGPT Codex stream failed: {}",
                        chatgpt_codex_stream_error_message(&value)
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<LlmCompletion> {
        if let Some(mut completion) = self.completed {
            let missing_completed_text = completion
                .turn
                .text
                .as_deref()
                .map(str::is_empty)
                .unwrap_or(true);
            if missing_completed_text && !self.assistant_text.is_empty() {
                completion.turn.text = Some(self.assistant_text);
            }
            if completion.turn.reasoning.is_empty() && !self.reasoning_text.is_empty() {
                completion
                    .turn
                    .reasoning
                    .push(reasoning_block(self.reasoning_text));
            }
            return Ok(completion);
        }

        let text = (!self.assistant_text.is_empty()).then_some(self.assistant_text);
        let reasoning = if self.reasoning_text.is_empty() {
            Vec::new()
        } else {
            vec![reasoning_block(self.reasoning_text)]
        };
        Ok(LlmCompletion {
            turn: AssistantTurn {
                text,
                tool_calls: Vec::new(),
                reasoning,
            },
            token_usage: None,
        })
    }
}

fn chatgpt_codex_stream_error_message(value: &Value) -> String {
    value
        .pointer("/response/error/message")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown stream error")
        .to_string()
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
                bail!("ChatGPT Codex stream chunk was not valid UTF-8");
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

fn sse_data_lines(frame: &str) -> Vec<String> {
    frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.trim_start().to_string())
        .collect()
}

fn non_empty_instructions(instructions: String) -> String {
    if instructions.trim().is_empty() {
        "You are ExAgent.".to_string()
    } else {
        instructions
    }
}

fn responses_reasoning(
    thinking_mode: Option<ThinkingMode>,
    capabilities: &ReasoningCapabilities,
) -> Option<Value> {
    let mode = capabilities.effective_mode(thinking_mode)?;
    let effort = capabilities.provider_mode_value(mode)?;
    if mode == ThinkingMode::Off {
        return Some(json!({ "effort": effort }));
    }
    Some(json!({ "effort": effort, "summary": "auto" }))
}

fn parse_chatgpt_codex_response(value: Value) -> Result<LlmCompletion> {
    let output = value
        .get("output")
        .and_then(Value::as_array)
        .context("ChatGPT Codex response missing output")?;
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                reasoning.extend(parse_responses_reasoning_item(item));
            }
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("output_text" | "text")
                        ) {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(|raw| serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw })))
                    .unwrap_or_else(|| json!({}));
                if !name.is_empty() {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                        thought_signature: None,
                    });
                }
            }
            _ => {}
        }
    }
    let token_usage = parse_responses_usage(value.get("usage"));
    Ok(LlmCompletion {
        turn: AssistantTurn {
            text: (!text_parts.is_empty()).then(|| text_parts.join("")),
            tool_calls,
            reasoning,
        },
        token_usage,
    })
}

fn parse_responses_reasoning_item(item: &Value) -> Vec<ReasoningBlock> {
    let mut blocks = Vec::new();
    collect_reasoning_text_parts(
        &mut blocks,
        item.get("summary").and_then(Value::as_array),
        &["summary_text"],
    );
    collect_reasoning_text_parts(
        &mut blocks,
        item.get("content").and_then(Value::as_array),
        &["reasoning_text"],
    );
    blocks
}

fn collect_reasoning_text_parts(
    blocks: &mut Vec<ReasoningBlock>,
    parts: Option<&Vec<Value>>,
    allowed_types: &[&str],
) {
    let Some(parts) = parts else {
        return;
    };
    for part in parts {
        let part_type = part.get("type").and_then(Value::as_str);
        if !matches!(part_type, Some(kind) if allowed_types.contains(&kind)) {
            continue;
        }
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        blocks.push(reasoning_block(text.to_string()));
    }
}

fn reasoning_block(text: String) -> ReasoningBlock {
    ReasoningBlock {
        text,
        signature: None,
        redacted: false,
    }
}

fn parse_responses_usage(value: Option<&Value>) -> Option<TokenUsage> {
    let value = value?;
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached_input_tokens = value
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let reasoning_output_tokens = value
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(input_tokens + output_tokens),
    })
}

#[derive(Debug, Clone, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use anyhow::Result;
    use async_trait::async_trait;
    use axum::{
        extract::Form,
        http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use crate::config::ThinkingMode;
    use crate::llm::{LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink};
    use crate::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};
    use crate::resolved::ResolvedCredential;
    use crate::types::ConversationMessage;

    #[derive(Default)]
    struct RecordingTokenRefreshSink {
        updates: Mutex<Vec<super::ChatGptCodexTokenUpdate>>,
    }

    #[async_trait]
    impl super::ChatGptCodexTokenRefreshSink for RecordingTokenRefreshSink {
        async fn save_chatgpt_codex_tokens(
            &self,
            update: super::ChatGptCodexTokenUpdate,
        ) -> Result<()> {
            self.updates.lock().await.push(update);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingStreamSink {
        events: Vec<LlmStreamEvent>,
    }

    #[async_trait]
    impl LlmStreamSink for RecordingStreamSink {
        async fn event(&mut self, event: LlmStreamEvent) -> Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    fn openai_reasoning_capabilities() -> ReasoningCapabilities {
        ReasoningCapabilities {
            protocol: ReasoningProtocol::OpenAiReasoningEffort,
            supported_modes: vec![
                ThinkingMode::Off,
                ThinkingMode::Low,
                ThinkingMode::Medium,
                ThinkingMode::High,
                ThinkingMode::XHigh,
            ],
            default_mode: Some(ThinkingMode::Medium),
            mode_map: BTreeMap::new(),
            requires_assistant_reasoning_content: false,
        }
    }

    #[test]
    fn responses_request_uses_default_reasoning_summary_for_auto_mode() {
        let request = super::build_chatgpt_codex_responses_request(
            "gpt-5.5".to_string(),
            &[ConversationMessage::user("hello")],
            &[],
            &LlmRequestOptions {
                thinking_mode: Some(ThinkingMode::Auto),
                ..LlmRequestOptions::default()
            },
            &openai_reasoning_capabilities(),
            true,
        )
        .unwrap();

        assert_eq!(
            request["reasoning"],
            json!({ "effort": "medium", "summary": "auto" })
        );
        assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn responses_request_can_turn_reasoning_off_without_summary_include() {
        let request = super::build_chatgpt_codex_responses_request(
            "gpt-5.5".to_string(),
            &[ConversationMessage::user("hello")],
            &[],
            &LlmRequestOptions {
                thinking_mode: Some(ThinkingMode::Off),
                ..LlmRequestOptions::default()
            },
            &openai_reasoning_capabilities(),
            true,
        )
        .unwrap();

        assert_eq!(request["reasoning"], json!({ "effort": "none" }));
        assert_eq!(request["include"], json!([]));
    }

    #[tokio::test]
    async fn sends_chatgpt_account_header_to_codex_endpoint() {
        let saw_headers = Arc::new(AtomicUsize::new(0));
        let app_saw_headers = saw_headers.clone();
        let app = Router::new().route(
            "/backend-api/codex/responses",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let app_saw_headers = app_saw_headers.clone();
                async move {
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|header| header.to_str().ok()),
                        Some("Bearer access-token-1")
                    );
                    assert_eq!(
                        headers
                            .get("ChatGPT-Account-ID")
                            .and_then(|header| header.to_str().ok()),
                        Some("acct_123")
                    );
                    assert!(body.get("input").is_some());
                    assert!(body.get("messages").is_none());
                    assert_eq!(body.get("stream").and_then(Value::as_bool), Some(false));
                    app_saw_headers.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "output": [{
                            "type": "message",
                            "role": "assistant",
                            "content": [{
                                "type": "output_text",
                                "text": "ok"
                            }]
                        }],
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let llm = super::ChatGptCodexLlm::from_parts(
            "gpt-5.5",
            format!("http://{addr}/backend-api/codex/responses"),
            "http://127.0.0.1/unused",
            ResolvedCredential::ChatGptOAuth {
                access_token: "access-token-1".to_string(),
                refresh_token: "refresh-token-1".to_string(),
                expires_at_ms: None,
                account_id: Some("acct_123".to_string()),
                credential_id: Some("chatgpt-1".to_string()),
            },
        )
        .unwrap();

        let completion = llm
            .complete(
                &[ConversationMessage::user("hello")],
                &[],
                &LlmRequestOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("ok"));
        assert_eq!(saw_headers.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn streams_with_required_codex_stream_flag_and_parses_completed_response() {
        let saw_request = Arc::new(AtomicUsize::new(0));
        let app_saw_request = saw_request.clone();
        let app = Router::new().route(
            "/backend-api/codex/responses",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let app_saw_request = app_saw_request.clone();
                async move {
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|header| header.to_str().ok()),
                        Some("Bearer access-token-1")
                    );
                    assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
                    app_saw_request.fetch_add(1, Ordering::SeqCst);
                    let stream = concat!(
                        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thinking\"}\n\n",
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"thinking\"}]},{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
                    );
                    ([(CONTENT_TYPE, "text/event-stream")], stream)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let llm = super::ChatGptCodexLlm::from_parts(
            "gpt-5.5",
            format!("http://{addr}/backend-api/codex/responses"),
            "http://127.0.0.1/unused",
            ResolvedCredential::ChatGptOAuth {
                access_token: "access-token-1".to_string(),
                refresh_token: "refresh-token-1".to_string(),
                expires_at_ms: None,
                account_id: Some("acct_123".to_string()),
                credential_id: Some("chatgpt-1".to_string()),
            },
        )
        .unwrap();
        let mut sink = RecordingStreamSink::default();

        let completion = llm
            .stream(
                &[ConversationMessage::user("hello")],
                &[],
                &LlmRequestOptions::default(),
                &mut sink,
            )
            .await
            .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("ok"));
        assert_eq!(completion.turn.reasoning[0].text, "thinking");
        assert_eq!(completion.token_usage.unwrap().total_tokens, 2);
        assert_eq!(saw_request.load(Ordering::SeqCst), 1);
        assert!(sink
            .events
            .contains(&LlmStreamEvent::ReasoningDelta("thinking".to_string())));
        assert!(sink
            .events
            .contains(&LlmStreamEvent::AssistantTextDelta("ok".to_string())));
        assert!(matches!(
            sink.events.last(),
            Some(LlmStreamEvent::Completed(completion)) if completion.turn.text.as_deref() == Some("ok")
        ));
    }

    #[tokio::test]
    async fn stream_finish_keeps_text_deltas_when_completed_output_is_empty() {
        let mut parser = super::ChatGptCodexStreamParser::default();
        let mut sink = RecordingStreamSink::default();

        parser
            .process_frame(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
                &mut sink,
            )
            .await
            .unwrap();
        parser
            .process_frame(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
                &mut sink,
            )
            .await
            .unwrap();

        let completion = parser.finish().unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("hello"));
        assert_eq!(completion.token_usage.unwrap().total_tokens, 3);
    }

    #[tokio::test]
    async fn refreshes_and_retries_once_after_unauthorized_response() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app_attempts = attempts.clone();
        let app = Router::new()
            .route(
                "/backend-api/codex/responses",
                post(move |headers: HeaderMap| {
                    let app_attempts = app_attempts.clone();
                    async move {
                        let attempt = app_attempts.fetch_add(1, Ordering::SeqCst);
                        let auth = headers
                            .get("authorization")
                            .and_then(|header| header.to_str().ok());
                        if attempt == 0 {
                            assert_eq!(auth, Some("Bearer stale-access-token"));
                            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "expired"})));
                        }
                        assert_eq!(auth, Some("Bearer refreshed-access-token"));
                        (
                            StatusCode::OK,
                            Json(json!({
                                "output": [{
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [{
                                        "type": "output_text",
                                        "text": "retried"
                                    }]
                                }]
                            })),
                        )
                    }
                }),
            )
            .route(
                "/oauth/token",
                post(|Form(form): Form<HashMap<String, String>>| async move {
                    assert_eq!(
                        form.get("grant_type").map(String::as_str),
                        Some("refresh_token")
                    );
                    assert_eq!(
                        form.get("refresh_token").map(String::as_str),
                        Some("refresh-token-1")
                    );
                    Json(json!({
                        "access_token": "refreshed-access-token",
                        "refresh_token": "refresh-token-2",
                        "expires_in": 3600
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let sink = Arc::new(RecordingTokenRefreshSink::default());
        let llm = super::ChatGptCodexLlm::from_parts_with_token_refresh_sink(
            "gpt-5.5",
            format!("http://{addr}/backend-api/codex/responses"),
            format!("http://{addr}"),
            ResolvedCredential::ChatGptOAuth {
                access_token: "stale-access-token".to_string(),
                refresh_token: "refresh-token-1".to_string(),
                expires_at_ms: None,
                account_id: Some("acct_123".to_string()),
                credential_id: Some("chatgpt-1".to_string()),
            },
            Some(sink.clone()),
        )
        .unwrap();

        let completion = llm
            .complete(
                &[ConversationMessage::user("hello")],
                &[],
                &LlmRequestOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("retried"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let updates = sink.updates.lock().await;
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].access_token, "refreshed-access-token");
        assert_eq!(updates[0].refresh_token, "refresh-token-2");
        assert_eq!(updates[0].credential_id.as_deref(), Some("chatgpt-1"));
    }

    #[test]
    fn parses_responses_reasoning_summary_output() {
        let completion = super::parse_chatgpt_codex_response(json!({
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [
                        { "type": "summary_text", "text": "Checked the constraints." }
                    ],
                    "content": [
                        { "type": "reasoning_text", "text": "Visible reasoning text." }
                    ],
                    "encrypted_content": "opaque"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "done" }]
                }
            ]
        }))
        .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("done"));
        assert_eq!(
            completion
                .turn
                .reasoning
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Checked the constraints.", "Visible reasoning text."]
        );
    }

    #[test]
    fn parses_responses_function_call_output() {
        let completion = super::parse_chatgpt_codex_response(json!({
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "checking" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"query\":\"docs\"}"
                }
            ],
            "usage": {
                "input_tokens": 3,
                "input_tokens_details": { "cached_tokens": 1 },
                "output_tokens": 5,
                "output_tokens_details": { "reasoning_tokens": 2 },
                "total_tokens": 8
            }
        }))
        .unwrap();

        assert_eq!(completion.turn.text.as_deref(), Some("checking"));
        assert_eq!(completion.turn.tool_calls.len(), 1);
        assert_eq!(completion.turn.tool_calls[0].id, "call_1");
        assert_eq!(completion.turn.tool_calls[0].name, "lookup");
        assert_eq!(completion.turn.tool_calls[0].arguments["query"], "docs");
        let usage = completion.token_usage.unwrap();
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.cached_input_tokens, 1);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.reasoning_output_tokens, 2);
        assert_eq!(usage.total_tokens, 8);
    }
}
