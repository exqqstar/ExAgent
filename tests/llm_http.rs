use exagent::llm::{is_context_window_error, OpenAiCompatibleLlm};
use exagent::types::TokenUsage;
use serde_json::json;

#[test]
fn openai_client_requires_model_configuration() {
    let _guard = EnvGuard::set([
        ("OPENAI_BASE_URL", Some("https://api.openai.com/v1")),
        ("OPENAI_API_KEY", Some("test-key")),
        ("OPENAI_MODEL", None),
    ]);

    let build = OpenAiCompatibleLlm::from_env();
    assert!(build.is_err());
}

#[test]
fn openai_client_accepts_explicit_model_without_openai_model_env() {
    let _guard = EnvGuard::set([
        ("OPENAI_BASE_URL", Some("https://api.openai.com/v1")),
        ("OPENAI_API_KEY", Some("test-key")),
        ("OPENAI_MODEL", None),
    ]);

    let build = OpenAiCompatibleLlm::from_env_with_model("configured-model");
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

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn set<const N: usize>(entries: [(&'static str, Option<&'static str>); N]) -> Self {
        let mut saved = Vec::with_capacity(N);

        for (key, value) in entries {
            saved.push((key, std::env::var(key).ok()));
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
