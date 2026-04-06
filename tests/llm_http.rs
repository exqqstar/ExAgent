use exagent::llm::OpenAiCompatibleLlm;
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
fn openai_response_parses_assistant_text_and_tool_calls() {
    let turn = OpenAiCompatibleLlm::parse_response(json!({
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

    assert_eq!(turn.text.as_deref(), Some("I will read the file"));
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_1");
    assert_eq!(turn.tool_calls[0].name, "read_file");
    assert_eq!(turn.tool_calls[0].arguments["path"], "Cargo.toml");
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
