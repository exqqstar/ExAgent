use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use axum::{
    body::{Body, Bytes},
    http::header::{AUTHORIZATION, CONTENT_TYPE},
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use exagent::config::ThinkingMode;
use exagent::llm::{
    is_context_window_error, LlmClient, LlmRequestOptions, LlmStreamEvent, LlmStreamSink,
    OpenAiCompatibleLlm,
};
use exagent::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::types::{
    ConversationMessage, ReasoningBlock, ReasoningSignature, TokenUsage, ToolCall,
};
use serde_json::{json, Value};
use std::convert::Infallible;

#[derive(Default)]
struct RecordingStreamSink {
    events: Vec<LlmStreamEvent>,
}

#[async_trait::async_trait]
impl LlmStreamSink for RecordingStreamSink {
    async fn event(&mut self, event: LlmStreamEvent) -> anyhow::Result<()> {
        self.events.push(event);
        Ok(())
    }
}

#[test]
fn openai_client_requires_model_in_from_parts() {
    let build = OpenAiCompatibleLlm::from_parts("", "https://api.openai.com/v1", None::<String>);
    assert!(build.is_err());
}

#[test]
fn openai_client_accepts_resolved_model_config() {
    let model = ResolvedModelConfig::from_provider_profile(
        "openai",
        "configured-model",
        Some("https://api.openai.com/v1".to_string()),
        ResolvedCredential::BearerToken("test-token".to_string()),
        None,
    );

    let build = OpenAiCompatibleLlm::from_config(&model);
    assert!(build.is_ok());
}

#[test]
fn openai_response_parses_assistant_text_and_tool_calls() {
    let completion = OpenAiCompatibleLlm::parse_response(json!({
        "choices": [{
            "message": {
                "content": "I will read the file",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"Cargo.toml\"}"
                    }
                }]
            }
        }]
    }))
    .unwrap();
    let turn = completion.turn;

    assert_eq!(turn.text.as_deref(), Some("I will read the file"));
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_1");
    assert_eq!(turn.tool_calls[0].name, "read_file");
    assert_eq!(turn.tool_calls[0].arguments["path"], "Cargo.toml");
    assert_eq!(completion.token_usage, None);
}

#[test]
fn openai_response_accepts_object_tool_call_arguments() {
    let completion = OpenAiCompatibleLlm::parse_response(json!({
        "choices": [{
            "message": {
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": { "path": "Cargo.toml" }
                    }
                }]
            }
        }]
    }))
    .unwrap();

    assert_eq!(
        completion.turn.tool_calls[0].arguments["path"],
        "Cargo.toml"
    );
}

#[test]
fn openai_response_rejects_invalid_string_tool_call_arguments() {
    let error = OpenAiCompatibleLlm::parse_response(json!({
        "choices": [{
            "message": {
                "tool_calls": [{
                    "id": "call_bad",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{not json"
                    }
                }]
            }
        }]
    }))
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("Tool call call_bad returned invalid JSON arguments"),
        "{error:#}"
    );
}

#[test]
fn openai_response_preserves_reasoning_content_and_details() {
    let details = json!([{ "type": "encrypted", "data": "opaque-provider-state" }]);
    let completion = OpenAiCompatibleLlm::parse_response(json!({
        "choices": [{
            "message": {
                "content": "I will use a tool",
                "reasoning_content": "private chain",
                "reasoning_details": details
            }
        }]
    }))
    .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("I will use a tool"));
    assert_eq!(
        completion.turn.reasoning,
        vec![
            ReasoningBlock {
                text: "private chain".to_string(),
                signature: Some(ReasoningSignature::OpenAiField {
                    field: "reasoning_content".to_string(),
                }),
                redacted: false,
            },
            ReasoningBlock {
                text: String::new(),
                signature: Some(ReasoningSignature::ReasoningDetails(details)),
                redacted: true,
            },
        ]
    );
}

#[test]
fn openai_response_parses_token_usage_when_present() {
    let completion = OpenAiCompatibleLlm::parse_response(json!({
        "choices": [{
            "message": {
                "content": "done"
            }
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
            "prompt_tokens_details": {
                "cached_tokens": 2
            },
            "completion_tokens_details": {
                "reasoning_tokens": 1
            }
        }
    }))
    .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("done"));
    assert_eq!(
        completion.token_usage,
        Some(TokenUsage {
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 15,
        })
    );
}

#[test]
fn openai_context_window_errors_are_classified_by_message() {
    let exceeded = anyhow::anyhow!(
        "{}",
        r#"OpenAI-compatible request failed with status 400 Bad Request: {"error":{"code":"context_length_exceeded","message":"too many tokens"}}"#
    );
    let maximum = anyhow::anyhow!("maximum context length is 128000 tokens");
    let ordinary = anyhow::anyhow!("OpenAI-compatible request failed with status 401");

    assert!(is_context_window_error(&exceeded));
    assert!(is_context_window_error(&maximum));
    assert!(!is_context_window_error(&ordinary));
}

#[tokio::test]
async fn openai_compatible_client_can_call_gateway_without_api_key() {
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let app_saw_authorization = saw_authorization.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap| {
            let app_saw_authorization = app_saw_authorization.clone();
            async move {
                if headers.contains_key(AUTHORIZATION) {
                    app_saw_authorization.store(true, Ordering::SeqCst);
                }
                Json(json!({
                    "choices": [{
                        "message": {
                            "content": "ok"
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

    let llm =
        OpenAiCompatibleLlm::from_parts("local-model", format!("http://{addr}/v1"), None::<String>)
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
    assert!(!saw_authorization.load(Ordering::SeqCst));
}

#[tokio::test]
async fn openai_compatible_client_sends_resolved_deepseek_reasoning_fields() {
    let captured_body = Arc::new(Mutex::new(None::<Value>));
    let app_captured_body = captured_body.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let app_captured_body = app_captured_body.clone();
            async move {
                *app_captured_body.lock().unwrap() = Some(body);
                Json(json!({
                    "choices": [{
                        "message": {
                            "content": "ok"
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

    let model = ResolvedModelConfig::from_provider_profile(
        "deepseek",
        "deepseek-v4-flash",
        Some(format!("http://{addr}")),
        ResolvedCredential::ApiKey("test-token".to_string()),
        None,
    );
    let llm = OpenAiCompatibleLlm::from_config(&model).unwrap();

    let completion = llm
        .complete(
            &[ConversationMessage::user("hello")],
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: Some(ThinkingMode::High),
                reasoning_capabilities: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
    let body = captured_body
        .lock()
        .unwrap()
        .clone()
        .expect("request body should be captured");
    assert_eq!(body["thinking"], json!({ "type": "enabled" }));
    assert_eq!(body["reasoning_effort"], "high");
}

#[tokio::test]
async fn openai_compatible_client_streams_deepseek_reasoning_and_text() {
    let captured_body = Arc::new(Mutex::new(None::<Value>));
    let app_captured_body = captured_body.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let app_captured_body = app_captured_body.clone();
            async move {
                *app_captured_body.lock().unwrap() = Some(body);
                let body = concat!(
                    "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"},\"finish_reason\":null}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"first\"},\"finish_reason\":null}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"world\"},\"finish_reason\":null}]}\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],",
                    "\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15,",
                    "\"prompt_tokens_details\":{\"cached_tokens\":2},",
                    "\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\r\n\r\n",
                    "data: [DONE]\r\n\r\n"
                );
                ([(CONTENT_TYPE, "text/event-stream")], body).into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let model = ResolvedModelConfig::from_provider_profile(
        "deepseek",
        "deepseek-v4-flash",
        Some(format!("http://{addr}")),
        ResolvedCredential::ApiKey("test-token".to_string()),
        None,
    );
    let llm = OpenAiCompatibleLlm::from_config(&model).unwrap();
    let mut sink = RecordingStreamSink::default();

    let completion = llm
        .stream(
            &[ConversationMessage::user("hello")],
            &[],
            &LlmRequestOptions {
                model: None,
                thinking_mode: Some(ThinkingMode::High),
                reasoning_capabilities: None,
            },
            &mut sink,
        )
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("hello world"));
    assert_eq!(completion.turn.reasoning.len(), 1);
    assert_eq!(completion.turn.reasoning[0].text, "think first");
    assert_eq!(
        completion.token_usage,
        Some(TokenUsage {
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 3,
            total_tokens: 15,
        })
    );
    assert_eq!(
        sink.events,
        vec![
            LlmStreamEvent::ReasoningDelta("think ".to_string()),
            LlmStreamEvent::ReasoningDelta("first".to_string()),
            LlmStreamEvent::AssistantTextDelta("hello ".to_string()),
            LlmStreamEvent::AssistantTextDelta("world".to_string()),
            LlmStreamEvent::Completed(completion),
        ]
    );

    let body = captured_body
        .lock()
        .unwrap()
        .clone()
        .expect("request body should be captured");
    assert_eq!(body["stream"], true);
    assert_eq!(body["thinking"], json!({ "type": "enabled" }));
}

#[tokio::test]
async fn openai_compatible_client_streams_split_utf8_chunks() {
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(_body): Json<Value>| async move {
            let frame =
                "data: {\"choices\":[{\"delta\":{\"content\":\"你好🙂\"},\"finish_reason\":null}]}\n\n";
            let split = frame.find('🙂').expect("emoji should be present") + 1;
            let chunks = vec![
                frame.as_bytes()[..split].to_vec(),
                frame.as_bytes()[split..].to_vec(),
                b"data: [DONE]\n\n".to_vec(),
            ];
            let stream = async_stream::stream! {
                for chunk in chunks {
                    yield Ok::<Bytes, Infallible>(Bytes::from(chunk));
                }
            };
            ([(CONTENT_TYPE, "text/event-stream")], Body::from_stream(stream)).into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm =
        OpenAiCompatibleLlm::from_parts("local-model", format!("http://{addr}"), None::<String>)
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

    assert_eq!(completion.turn.text.as_deref(), Some("你好🙂"));
    assert_eq!(
        sink.events,
        vec![
            LlmStreamEvent::AssistantTextDelta("你好🙂".to_string()),
            LlmStreamEvent::Completed(completion),
        ]
    );
}

#[tokio::test]
async fn openai_compatible_client_streams_alternate_reasoning_fields() {
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(_body): Json<Value>| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"reasoning\":\"think \"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_text\":\"more\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n"
            );
            ([(CONTENT_TYPE, "text/event-stream")], body).into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm =
        OpenAiCompatibleLlm::from_parts("local-model", format!("http://{addr}"), None::<String>)
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

    assert_eq!(completion.turn.text.as_deref(), Some("done"));
    assert_eq!(completion.turn.reasoning.len(), 1);
    assert_eq!(completion.turn.reasoning[0].text, "think more");
    assert_eq!(
        completion.turn.reasoning[0].signature,
        Some(ReasoningSignature::OpenAiField {
            field: "reasoning".to_string(),
        })
    );
    assert_eq!(
        sink.events,
        vec![
            LlmStreamEvent::ReasoningDelta("think ".to_string()),
            LlmStreamEvent::ReasoningDelta("more".to_string()),
            LlmStreamEvent::AssistantTextDelta("done".to_string()),
            LlmStreamEvent::Completed(completion),
        ]
    );
}

#[tokio::test]
async fn openai_compatible_client_streams_tool_call_arguments() {
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(_body): Json<Value>| async move {
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
                "{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}",
                "]},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
                "{\"index\":0,\"function\":{\"arguments\":\"\\\"Cargo.toml\\\"}\"}}",
                "]},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            ([(CONTENT_TYPE, "text/event-stream")], body).into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm =
        OpenAiCompatibleLlm::from_parts("local-model", format!("http://{addr}"), None::<String>)
            .unwrap();
    let mut sink = RecordingStreamSink::default();

    let completion = llm
        .stream(
            &[ConversationMessage::user("read file")],
            &[],
            &LlmRequestOptions::default(),
            &mut sink,
        )
        .await
        .unwrap();

    assert_eq!(completion.turn.text, None);
    assert_eq!(completion.turn.tool_calls.len(), 1);
    assert_eq!(completion.turn.tool_calls[0].id, "call_1");
    assert_eq!(completion.turn.tool_calls[0].name, "read_file");
    assert_eq!(
        completion.turn.tool_calls[0].arguments["path"],
        "Cargo.toml"
    );
    assert_eq!(sink.events, vec![LlmStreamEvent::Completed(completion)]);
}

#[tokio::test]
async fn openai_compatible_client_sends_resolved_deepseek_off_reasoning_fields() {
    let body =
        capture_openai_compatible_request_body("deepseek", "deepseek-v4-flash", ThinkingMode::Off)
            .await;

    assert_eq!(body["thinking"], json!({ "type": "disabled" }));
    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_client_omits_reasoning_effort_for_non_reasoning_model_off() {
    let body = capture_openai_compatible_request_body("openai", "gpt-4.1", ThinkingMode::Off).await;

    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_client_omits_reasoning_effort_for_non_reasoning_snapshot_model_high() {
    let body =
        capture_openai_compatible_request_body("openai", "gpt-4.1-2025-04-14", ThinkingMode::High)
            .await;

    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_client_omits_reasoning_effort_for_unknown_model_off() {
    let body =
        capture_openai_compatible_request_body("openai", "gpt-future-reasoner", ThinkingMode::Off)
            .await;

    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_from_parts_omits_reasoning_effort_for_off() {
    let body = capture_from_parts_request_body("local-model", ThinkingMode::Off).await;

    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_client_sends_reasoning_effort_none_for_known_current_model_off() {
    let body = capture_openai_compatible_request_body("openai", "gpt-5.5", ThinkingMode::Off).await;

    assert_eq!(body["reasoning_effort"], "none");
}

#[tokio::test]
async fn openai_client_uses_per_turn_unsupported_reasoning_capabilities() {
    let body = capture_openai_compatible_request_body_with_options(
        "openai",
        "gpt-5.5",
        LlmRequestOptions {
            model: Some("gpt-4.1".to_string()),
            thinking_mode: Some(ThinkingMode::Off),
            reasoning_capabilities: Some(ReasoningCapabilities::unsupported()),
        },
    )
    .await;

    assert_eq!(body["model"], "gpt-4.1");
    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_client_uses_per_turn_off_reasoning_capabilities() {
    let body = capture_openai_compatible_request_body_with_options(
        "openai",
        "gpt-4.1",
        LlmRequestOptions {
            model: Some("gpt-5.5".to_string()),
            thinking_mode: Some(ThinkingMode::Off),
            reasoning_capabilities: Some(ReasoningCapabilities {
                protocol: ReasoningProtocol::OpenAiReasoningEffort,
                supported_modes: vec![ThinkingMode::Off],
                default_mode: None,
                mode_map: Default::default(),
                requires_assistant_reasoning_content: false,
            }),
        },
    )
    .await;

    assert_eq!(body["model"], "gpt-5.5");
    assert_eq!(body["reasoning_effort"], "none");
}

#[tokio::test]
async fn openai_compatible_client_sends_resolved_glm_thinking_object_fields() {
    let high = capture_openai_compatible_request_body("glm", "glm-5.1", ThinkingMode::High).await;
    let off = capture_openai_compatible_request_body("glm", "glm-5.1", ThinkingMode::Off).await;

    assert_eq!(high["thinking"], json!({ "type": "enabled" }));
    assert_eq!(off["thinking"], json!({ "type": "disabled" }));
    assert!(high.get("enable_thinking").is_none());
    assert!(off.get("enable_thinking").is_none());
}

#[tokio::test]
async fn openai_compatible_client_sends_resolved_kimi_thinking_object_fields() {
    let high =
        capture_openai_compatible_request_body("kimi", "kimi-k2.6", ThinkingMode::High).await;
    let off = capture_openai_compatible_request_body("kimi", "kimi-k2.6", ThinkingMode::Off).await;

    assert_eq!(high["thinking"], json!({ "type": "enabled" }));
    assert_eq!(off["thinking"], json!({ "type": "disabled" }));
    assert!(high.get("chat_template_args").is_none());
    assert!(off.get("chat_template_args").is_none());
    assert!(high.get("reasoning_effort").is_none());
    assert!(off.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_compatible_client_replays_saved_assistant_reasoning() {
    let messages = vec![
        ConversationMessage::user("hello"),
        ConversationMessage::assistant_with_reasoning(
            Some("I need a file".to_string()),
            vec![
                ReasoningBlock {
                    text: "saved private chain".to_string(),
                    signature: Some(ReasoningSignature::OpenAiField {
                        field: "reasoning_content".to_string(),
                    }),
                    redacted: false,
                },
                ReasoningBlock {
                    text: String::new(),
                    signature: Some(ReasoningSignature::ReasoningDetails(json!([
                        { "type": "encrypted", "data": "opaque" }
                    ]))),
                    redacted: true,
                },
            ],
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "Cargo.toml" }),
                thought_signature: None,
            }],
        ),
    ];
    let body = capture_from_parts_request_body_with_messages(
        "local-model",
        messages,
        LlmRequestOptions::default(),
    )
    .await;

    let assistant = &body["messages"][1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "I need a file");
    assert_eq!(assistant["reasoning_content"], "saved private chain");
    assert_eq!(
        assistant["reasoning_details"],
        json!([{ "type": "encrypted", "data": "opaque" }])
    );
    assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
}

#[tokio::test]
async fn openai_compatible_client_replays_required_empty_assistant_reasoning_content() {
    let messages = vec![
        ConversationMessage::user("hello"),
        ConversationMessage::assistant(Some("plain assistant turn".to_string()), vec![]),
    ];
    let body = capture_from_parts_request_body_with_messages(
        "deepseek-reasoner",
        messages,
        LlmRequestOptions {
            model: None,
            thinking_mode: Some(ThinkingMode::High),
            reasoning_capabilities: Some(ReasoningCapabilities {
                protocol: ReasoningProtocol::DeepSeekThinking,
                supported_modes: vec![ThinkingMode::Off, ThinkingMode::High],
                default_mode: None,
                mode_map: Default::default(),
                requires_assistant_reasoning_content: true,
            }),
        },
    )
    .await;

    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["reasoning_content"], "");
}

async fn capture_openai_compatible_request_body(
    provider_id: &str,
    model_id: &str,
    thinking_mode: ThinkingMode,
) -> Value {
    capture_openai_compatible_request_body_with_options(
        provider_id,
        model_id,
        LlmRequestOptions {
            model: None,
            thinking_mode: Some(thinking_mode),
            reasoning_capabilities: None,
        },
    )
    .await
}

async fn capture_openai_compatible_request_body_with_options(
    provider_id: &str,
    model_id: &str,
    options: LlmRequestOptions,
) -> Value {
    let captured_body = Arc::new(Mutex::new(None::<Value>));
    let app_captured_body = captured_body.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let app_captured_body = app_captured_body.clone();
            async move {
                *app_captured_body.lock().unwrap() = Some(body);
                Json(json!({
                    "choices": [{
                        "message": {
                            "content": "ok"
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

    let model = ResolvedModelConfig::from_provider_profile(
        provider_id,
        model_id,
        Some(format!("http://{addr}")),
        ResolvedCredential::ApiKey("test-token".to_string()),
        None,
    );
    let llm = OpenAiCompatibleLlm::from_config(&model).unwrap();

    let completion = llm
        .complete(&[ConversationMessage::user("hello")], &[], &options)
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
    let body = captured_body
        .lock()
        .unwrap()
        .clone()
        .expect("request body should be captured");
    body
}

async fn capture_from_parts_request_body(model_id: &str, thinking_mode: ThinkingMode) -> Value {
    capture_from_parts_request_body_with_messages(
        model_id,
        vec![ConversationMessage::user("hello")],
        LlmRequestOptions {
            model: None,
            thinking_mode: Some(thinking_mode),
            reasoning_capabilities: None,
        },
    )
    .await
}

async fn capture_from_parts_request_body_with_messages(
    model_id: &str,
    messages: Vec<ConversationMessage>,
    options: LlmRequestOptions,
) -> Value {
    let captured_body = Arc::new(Mutex::new(None::<Value>));
    let app_captured_body = captured_body.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let app_captured_body = app_captured_body.clone();
            async move {
                *app_captured_body.lock().unwrap() = Some(body);
                Json(json!({
                    "choices": [{
                        "message": {
                            "content": "ok"
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

    let llm =
        OpenAiCompatibleLlm::from_parts(model_id, format!("http://{addr}"), Some("test-token"))
            .unwrap();

    let completion = llm.complete(&messages, &[], &options).await.unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
    let body = captured_body
        .lock()
        .unwrap()
        .clone()
        .expect("request body should be captured");
    body
}
