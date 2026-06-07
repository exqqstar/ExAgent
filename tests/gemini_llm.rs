use axum::{http::HeaderMap, routing::post, Json, Router};
use exagent::config::ThinkingMode;
use exagent::llm::{GeminiLlm, LlmClient, LlmRequestOptions};
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::tools::ToolSpec;
use exagent::types::{
    ConversationMessage, ReasoningBlock, ReasoningSignature, TokenUsage, ToolCall,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn gemini_client_accepts_resolved_model_config() {
    let model = ResolvedModelConfig::from_provider_profile(
        "google",
        "gemini-3-flash-preview",
        Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
        ResolvedCredential::ApiKey("google-key".to_string()),
        None,
    );

    let build = GeminiLlm::from_config(&model);

    assert!(build.is_ok());
}

#[test]
fn gemini_response_parses_text_function_call_and_usage() {
    let completion = GeminiLlm::parse_response(json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "I will read the file" },
                    { "functionCall": { "name": "read_file", "args": { "path": "Cargo.toml" } } }
                ]
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 12,
            "cachedContentTokenCount": 2,
            "candidatesTokenCount": 5,
            "thoughtsTokenCount": 1,
            "totalTokenCount": 18
        }
    }))
    .unwrap();

    assert_eq!(
        completion.turn.text.as_deref(),
        Some("I will read the file")
    );
    assert_eq!(completion.turn.tool_calls.len(), 1);
    assert_eq!(completion.turn.tool_calls[0].name, "read_file");
    assert_eq!(
        completion.turn.tool_calls[0].arguments["path"],
        "Cargo.toml"
    );
    assert_eq!(completion.turn.tool_calls[0].thought_signature, None);
    assert_eq!(
        completion.token_usage,
        Some(TokenUsage {
            input_tokens: 12,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 18,
        })
    );
}

#[test]
fn gemini_response_preserves_tool_call_thought_signature() {
    let completion = GeminiLlm::parse_response(json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "thoughtSignature": "gemini-signature",
                    "functionCall": {
                        "name": "read_file",
                        "args": { "path": "Cargo.toml" }
                    }
                }]
            }
        }]
    }))
    .unwrap();

    assert_eq!(completion.turn.tool_calls.len(), 1);
    assert_eq!(
        completion.turn.tool_calls[0].thought_signature,
        Some(json!("gemini-signature"))
    );
}

#[test]
fn gemini_response_keeps_thought_text_out_of_visible_text() {
    let completion = GeminiLlm::parse_response(json!({
        "candidates": [{
            "content": {
                "parts": [
                    {
                        "thought": true,
                        "text": "private thought summary",
                        "thoughtSignature": "thought-sig"
                    },
                    { "text": "visible answer" }
                ]
            }
        }]
    }))
    .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("visible answer"));
    assert_eq!(
        completion.turn.reasoning,
        vec![ReasoningBlock {
            text: "private thought summary".to_string(),
            signature: Some(ReasoningSignature::GeminiThoughtSignature(
                "thought-sig".to_string(),
            )),
            redacted: false,
        }]
    );
}

#[test]
fn gemini_response_preserves_visible_text_thought_signature() {
    let completion = GeminiLlm::parse_response(json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "text": "visible answer",
                    "thoughtSignature": "text-sig"
                }]
            }
        }]
    }))
    .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("visible answer"));
    assert_eq!(
        completion.turn.reasoning,
        vec![ReasoningBlock {
            text: "visible answer".to_string(),
            signature: Some(ReasoningSignature::GeminiThoughtSignature(
                "text-sig".to_string(),
            )),
            redacted: true,
        }]
    );
}

#[tokio::test]
async fn gemini_client_sends_generate_content_request() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let app = Router::new().route(
        "/v1beta/models/gemini-3-flash-preview:generateContent",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                assert_eq!(
                    headers
                        .get("x-goog-api-key")
                        .and_then(|header| header.to_str().ok()),
                    Some("google-secret")
                );
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "candidates": [{
                        "content": { "parts": [{ "text": "ok" }] }
                    }],
                    "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 2, "totalTokenCount": 3 }
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm = GeminiLlm::from_parts(
        "gemini-3-flash-preview",
        format!("http://{addr}/v1beta"),
        "google-secret",
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
    assert_eq!(
        body["system_instruction"]["parts"][0]["text"],
        "You are concise."
    );
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(
        body["contents"][1]["parts"][1]["functionCall"]["name"],
        "read_file"
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "read_file"
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "read_file"
    );
    assert!(body.get("generationConfig").is_none());
}

#[tokio::test]
async fn gemini_client_sends_thinking_config_for_high_mode() {
    let body = capture_gemini_request_body(
        "gemini-3-flash-preview",
        LlmRequestOptions {
            thinking_mode: Some(ThinkingMode::High),
            ..LlmRequestOptions::default()
        },
    )
    .await;

    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["includeThoughts"],
        true
    );
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
}

#[tokio::test]
async fn gemini_client_uses_minimal_thinking_level_for_gemini_3_off() {
    let body = capture_gemini_request_body(
        "gemini-3-flash-preview",
        LlmRequestOptions {
            thinking_mode: Some(ThinkingMode::Off),
            ..LlmRequestOptions::default()
        },
    )
    .await;

    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "minimal"
    );
    assert!(body["generationConfig"]["thinkingConfig"]
        .get("thinkingBudget")
        .is_none());
}

#[tokio::test]
async fn gemini_client_uses_thinking_budget_for_gemini_2_5() {
    let high_body = capture_gemini_request_body(
        "gemini-2.5-flash",
        LlmRequestOptions {
            thinking_mode: Some(ThinkingMode::High),
            ..LlmRequestOptions::default()
        },
    )
    .await;
    let off_body = capture_gemini_request_body(
        "gemini-2.5-flash",
        LlmRequestOptions {
            thinking_mode: Some(ThinkingMode::Off),
            ..LlmRequestOptions::default()
        },
    )
    .await;

    assert_eq!(
        high_body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        16384
    );
    assert!(high_body["generationConfig"]["thinkingConfig"]
        .get("thinkingLevel")
        .is_none());
    assert_eq!(
        off_body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        0
    );
}

#[tokio::test]
async fn gemini_client_omits_off_thinking_budget_for_gemini_2_5_pro() {
    let body = capture_gemini_request_body(
        "gemini-2.5-pro",
        LlmRequestOptions {
            thinking_mode: Some(ThinkingMode::Off),
            ..LlmRequestOptions::default()
        },
    )
    .await;

    assert!(body.get("generationConfig").is_none());
}

#[tokio::test]
async fn gemini_client_omits_thinking_config_for_default_mode() {
    let default_body =
        capture_gemini_request_body("gemini-3-flash-preview", LlmRequestOptions::default()).await;

    assert!(default_body.get("generationConfig").is_none());
}

#[tokio::test]
async fn gemini_client_replays_assistant_tool_call_thought_signature() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let app = Router::new().route(
        "/v1beta/models/gemini-3-flash-preview:generateContent",
        post(move |Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "candidates": [{
                        "content": { "parts": [{ "text": "ok" }] }
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

    let llm = GeminiLlm::from_parts(
        "gemini-3-flash-preview",
        format!("http://{addr}/v1beta"),
        "google-secret",
    )
    .unwrap();

    llm.complete(
        &[ConversationMessage::assistant(
            None,
            vec![ToolCall {
                id: "toolu_1".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "Cargo.toml" }),
                thought_signature: Some(json!("gemini-signature")),
            }],
        )],
        &[],
        &LlmRequestOptions::default(),
    )
    .await
    .unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "gemini-signature"
    );
    assert!(body["contents"][0]["parts"][0]["functionCall"]
        .get("thoughtSignature")
        .is_none());
}

#[tokio::test]
async fn gemini_client_replays_multiple_visible_text_signatures_without_merging() {
    let body = capture_gemini_request_body_with_messages(
        "gemini-3-flash-preview",
        vec![ConversationMessage::assistant_with_reasoning(
            Some("first\nsecond".to_string()),
            vec![
                ReasoningBlock {
                    text: "first".to_string(),
                    signature: Some(ReasoningSignature::GeminiThoughtSignature(
                        "first-sig".to_string(),
                    )),
                    redacted: true,
                },
                ReasoningBlock {
                    text: "second".to_string(),
                    signature: Some(ReasoningSignature::GeminiThoughtSignature(
                        "second-sig".to_string(),
                    )),
                    redacted: true,
                },
            ],
            vec![],
        )],
        LlmRequestOptions::default(),
    )
    .await;

    assert_eq!(body["contents"][0]["parts"][0]["text"], "first");
    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "first-sig"
    );
    assert_eq!(body["contents"][0]["parts"][1]["text"], "second");
    assert_eq!(
        body["contents"][0]["parts"][1]["thoughtSignature"],
        "second-sig"
    );
}

#[tokio::test]
async fn gemini_client_preserves_unsigned_visible_text_around_signed_parts() {
    let body = capture_gemini_request_body_with_messages(
        "gemini-3-flash-preview",
        vec![ConversationMessage::assistant_with_reasoning(
            Some("intro\nsigned\noutro".to_string()),
            vec![ReasoningBlock {
                text: "signed".to_string(),
                signature: Some(ReasoningSignature::GeminiThoughtSignature(
                    "signed-sig".to_string(),
                )),
                redacted: true,
            }],
            vec![],
        )],
        LlmRequestOptions::default(),
    )
    .await;

    assert_eq!(body["contents"][0]["parts"][0]["text"], "intro");
    assert!(body["contents"][0]["parts"][0]
        .get("thoughtSignature")
        .is_none());
    assert_eq!(body["contents"][0]["parts"][1]["text"], "signed");
    assert_eq!(
        body["contents"][0]["parts"][1]["thoughtSignature"],
        "signed-sig"
    );
    assert_eq!(body["contents"][0]["parts"][2]["text"], "outro");
    assert!(body["contents"][0]["parts"][2]
        .get("thoughtSignature")
        .is_none());
}

#[tokio::test]
async fn gemini_client_replays_visible_and_thought_text_signatures() {
    let body = capture_gemini_request_body_with_messages(
        "gemini-3-flash-preview",
        vec![ConversationMessage::assistant_with_reasoning(
            Some("visible answer".to_string()),
            vec![
                ReasoningBlock {
                    text: String::new(),
                    signature: Some(ReasoningSignature::GeminiThoughtSignature(
                        "text-sig".to_string(),
                    )),
                    redacted: true,
                },
                ReasoningBlock {
                    text: "private thought summary".to_string(),
                    signature: Some(ReasoningSignature::GeminiThoughtSignature(
                        "thought-sig".to_string(),
                    )),
                    redacted: false,
                },
            ],
            vec![],
        )],
        LlmRequestOptions::default(),
    )
    .await;

    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "text-sig"
    );
    assert_eq!(body["contents"][0]["parts"][1]["thought"], true);
    assert_eq!(
        body["contents"][0]["parts"][1]["thoughtSignature"],
        "thought-sig"
    );
}

async fn capture_gemini_request_body(model: &str, options: LlmRequestOptions) -> Value {
    capture_gemini_request_body_with_messages(
        model,
        vec![ConversationMessage::user("hello")],
        options,
    )
    .await
}

async fn capture_gemini_request_body_with_messages(
    model: &str,
    messages: Vec<ConversationMessage>,
    options: LlmRequestOptions,
) -> Value {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let app_captured = captured.clone();
    let route = format!("/v1beta/models/{model}:generateContent");
    let app = Router::new().route(
        &route,
        post(move |Json(body): Json<Value>| {
            let app_captured = app_captured.clone();
            async move {
                *app_captured.lock().unwrap() = Some(body);
                Json(json!({
                    "candidates": [{
                        "content": { "parts": [{ "text": "ok" }] }
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
        GeminiLlm::from_parts(model, format!("http://{addr}/v1beta"), "google-secret").unwrap();

    llm.complete(&messages, &[], &options).await.unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    body
}
