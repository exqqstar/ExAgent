use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{http::header::AUTHORIZATION, http::HeaderMap, routing::post, Json, Router};
use exagent::llm::{is_context_window_error, LlmClient, LlmRequestOptions, OpenAiCompatibleLlm};
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::types::{ConversationMessage, TokenUsage};
use serde_json::json;

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
