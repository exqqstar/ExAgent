use axum::{http::HeaderMap, routing::post, Json, Router};
use exagent::config::ThinkingMode;
use exagent::llm::{AnthropicLlm, LlmClient, LlmRequestOptions};
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::tools::ToolSpec;
use exagent::types::{
    ConversationMessage, ReasoningBlock, ReasoningSignature, TokenUsage, ToolCall,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn anthropic_client_accepts_resolved_model_config() {
    let model = ResolvedModelConfig::from_provider_profile(
        "anthropic",
        "claude-sonnet-4-5",
        Some("https://api.anthropic.com/v1".to_string()),
        ResolvedCredential::ApiKey("test-token".to_string()),
        None,
    );

    let build = AnthropicLlm::from_config(&model);

    assert!(build.is_ok());
}

#[test]
fn anthropic_response_parses_text_tool_use_and_usage() {
    let completion = AnthropicLlm::parse_response(json!({
        "content": [
            { "type": "text", "text": "I will read the file" },
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "read_file",
                "input": { "path": "Cargo.toml" }
            }
        ],
        "usage": {
            "input_tokens": 11,
            "cache_read_input_tokens": 3,
            "output_tokens": 7
        }
    }))
    .unwrap();

    assert_eq!(
        completion.turn.text.as_deref(),
        Some("I will read the file")
    );
    assert_eq!(completion.turn.tool_calls.len(), 1);
    assert_eq!(completion.turn.tool_calls[0].id, "toolu_1");
    assert_eq!(completion.turn.tool_calls[0].name, "read_file");
    assert_eq!(
        completion.turn.tool_calls[0].arguments["path"],
        "Cargo.toml"
    );
    assert_eq!(
        completion.token_usage,
        Some(TokenUsage {
            input_tokens: 11,
            cached_input_tokens: 3,
            output_tokens: 7,
            reasoning_output_tokens: 0,
            total_tokens: 18,
        })
    );
}

#[test]
fn anthropic_response_preserves_thinking_and_redacted_thinking_blocks() {
    let completion = AnthropicLlm::parse_response(json!({
        "content": [
            {
                "type": "thinking",
                "thinking": "I should inspect the file first.",
                "signature": "anthropic-signature"
            },
            {
                "type": "redacted_thinking",
                "data": "redacted-payload"
            },
            { "type": "text", "text": "I will read the file" }
        ]
    }))
    .unwrap();

    assert_eq!(
        completion.turn.reasoning,
        vec![
            ReasoningBlock {
                text: "I should inspect the file first.".to_string(),
                signature: Some(ReasoningSignature::AnthropicSignature(
                    "anthropic-signature".to_string()
                )),
                redacted: false,
            },
            ReasoningBlock {
                text: String::new(),
                signature: Some(ReasoningSignature::AnthropicRedactedData(
                    "redacted-payload".to_string()
                )),
                redacted: true,
            }
        ]
    );
    assert_eq!(
        completion.turn.text.as_deref(),
        Some("I will read the file")
    );
}

#[tokio::test]
async fn anthropic_client_sends_messages_tools_and_required_headers() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                assert_eq!(
                    headers
                        .get("x-api-key")
                        .and_then(|header| header.to_str().ok()),
                    Some("anthropic-secret")
                );
                assert_eq!(
                    headers
                        .get("anthropic-version")
                        .and_then(|header| header.to_str().ok()),
                    Some("2023-06-01")
                );
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "content": [{ "type": "text", "text": "ok" }],
                    "usage": { "input_tokens": 1, "output_tokens": 2 }
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm = AnthropicLlm::from_parts(
        "claude-sonnet-4-5",
        format!("http://{addr}/v1"),
        "anthropic-secret",
    )
    .unwrap();

    let completion = llm
        .complete(
            &[
                ConversationMessage::system("You are concise."),
                ConversationMessage::user("hello"),
                ConversationMessage::assistant(
                    Some("checking".to_string()),
                    vec![ToolCall {
                        id: "toolu_1".to_string(),
                        name: "read_file".to_string(),
                        arguments: json!({ "path": "Cargo.toml" }),
                        thought_signature: None,
                    }],
                ),
                ConversationMessage::tool("toolu_1", "contents"),
            ],
            &[ToolSpec::function(
                "read_file",
                "Read a file",
                json!({ "type": "object" }),
            )],
            &LlmRequestOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
    let body = captured.lock().unwrap().clone().unwrap();
    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert_eq!(body["system"], "You are concise.");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
}

#[tokio::test]
async fn anthropic_client_sends_thinking_budget_for_high_mode() {
    let body = capture_anthropic_request_body(LlmRequestOptions {
        thinking_mode: Some(ThinkingMode::High),
        ..LlmRequestOptions::default()
    })
    .await;

    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 16000);
    assert!(
        body["max_tokens"].as_i64().unwrap() > body["thinking"]["budget_tokens"].as_i64().unwrap()
    );
}

#[tokio::test]
async fn anthropic_client_keeps_max_tokens_above_each_thinking_budget() {
    for (mode, expected_budget) in [
        (ThinkingMode::Low, 4000),
        (ThinkingMode::Medium, 8000),
        (ThinkingMode::XHigh, 32000),
    ] {
        let body = capture_anthropic_request_body(LlmRequestOptions {
            thinking_mode: Some(mode),
            ..LlmRequestOptions::default()
        })
        .await;

        assert_eq!(body["thinking"]["budget_tokens"], expected_budget);
        assert!(
            body["max_tokens"].as_i64().unwrap() > expected_budget,
            "{mode:?} max_tokens should be greater than thinking budget"
        );
    }
}

#[tokio::test]
async fn anthropic_client_omits_thinking_for_off_and_default_modes() {
    let default_body = capture_anthropic_request_body(LlmRequestOptions::default()).await;
    let off_body = capture_anthropic_request_body(LlmRequestOptions {
        thinking_mode: Some(ThinkingMode::Off),
        ..LlmRequestOptions::default()
    })
    .await;

    assert!(default_body.get("thinking").is_none());
    assert!(off_body.get("thinking").is_none());
}

#[tokio::test]
async fn anthropic_client_replays_assistant_thinking_signature_and_redacted_data() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "content": [{ "type": "text", "text": "ok" }]
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm = AnthropicLlm::from_parts(
        "claude-sonnet-4-5",
        format!("http://{addr}/v1"),
        "anthropic-secret",
    )
    .unwrap();

    llm.complete(
        &[ConversationMessage::assistant_with_reasoning(
            Some("visible".to_string()),
            vec![
                ReasoningBlock {
                    text: "private reasoning".to_string(),
                    signature: Some(ReasoningSignature::AnthropicSignature(
                        "anthropic-signature".to_string(),
                    )),
                    redacted: false,
                },
                ReasoningBlock {
                    text: String::new(),
                    signature: Some(ReasoningSignature::AnthropicRedactedData(
                        "redacted-data".to_string(),
                    )),
                    redacted: true,
                },
            ],
            vec![ToolCall {
                id: "toolu_1".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "Cargo.toml" }),
                thought_signature: None,
            }],
        )],
        &[],
        &LlmRequestOptions::default(),
    )
    .await
    .unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "private reasoning");
    assert_eq!(content[0]["signature"], "anthropic-signature");
    assert_eq!(content[1]["type"], "redacted_thinking");
    assert_eq!(content[1]["data"], "redacted-data");
    assert_eq!(content[2]["type"], "text");
    assert_eq!(content[3]["type"], "tool_use");
}

async fn capture_anthropic_request_body(options: LlmRequestOptions) -> Value {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "content": [{ "type": "text", "text": "ok" }]
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm = AnthropicLlm::from_parts(
        "claude-sonnet-4-5",
        format!("http://{addr}/v1"),
        "anthropic-secret",
    )
    .unwrap();

    llm.complete(&[ConversationMessage::user("hello")], &[], &options)
        .await
        .unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    body
}
