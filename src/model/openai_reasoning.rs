use serde_json::{json, Value};

use crate::config::ThinkingMode;
use crate::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};
use crate::types::{ReasoningBlock, ReasoningSignature};

const OPENAI_REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

pub fn extract_openai_reasoning_blocks(message: &Value) -> Vec<ReasoningBlock> {
    let mut blocks = Vec::new();

    for field in OPENAI_REASONING_FIELDS {
        let Some(text) = message.get(field).and_then(Value::as_str) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        blocks.push(ReasoningBlock {
            text: text.to_string(),
            signature: Some(ReasoningSignature::OpenAiField {
                field: field.to_string(),
            }),
            redacted: false,
        });
        break;
    }

    if let Some(details) = message.get("reasoning_details") {
        blocks.push(ReasoningBlock {
            text: String::new(),
            signature: Some(ReasoningSignature::ReasoningDetails(details.clone())),
            redacted: true,
        });
    }

    blocks
}

pub fn apply_reasoning_replay(
    assistant_message: &mut Value,
    blocks: &[ReasoningBlock],
    requires_empty_reasoning_content: bool,
) {
    let Some(object) = assistant_message.as_object_mut() else {
        return;
    };
    let mut wrote_reasoning_content = false;

    for block in blocks {
        match &block.signature {
            Some(ReasoningSignature::OpenAiField { field }) => {
                if block.text.is_empty() {
                    continue;
                }
                if !OPENAI_REASONING_FIELDS.contains(&field.as_str()) {
                    continue;
                }
                if field == "reasoning_content" {
                    wrote_reasoning_content = true;
                }
                object.insert(field.clone(), json!(block.text));
            }
            Some(ReasoningSignature::ReasoningDetails(value)) => {
                object.insert("reasoning_details".to_string(), value.clone());
            }
            Some(
                ReasoningSignature::GeminiThoughtSignature(_)
                | ReasoningSignature::AnthropicSignature(_)
                | ReasoningSignature::AnthropicRedactedData(_),
            )
            | None => {}
        }
    }

    if requires_empty_reasoning_content && !wrote_reasoning_content {
        object.insert("reasoning_content".to_string(), json!(""));
    }
}

pub fn apply_openai_reasoning(
    request: &mut Value,
    capabilities: &ReasoningCapabilities,
    requested: Option<ThinkingMode>,
) {
    let Some(object) = request.as_object_mut() else {
        return;
    };

    let effective = capabilities.effective_mode(requested);
    let enabled = !matches!(effective, None | Some(ThinkingMode::Off));

    match capabilities.protocol {
        ReasoningProtocol::None
        | ReasoningProtocol::GeminiThinkingConfig
        | ReasoningProtocol::AnthropicThinkingBudget => {}
        ReasoningProtocol::OpenAiReasoningEffort => {
            if matches!(requested, Some(ThinkingMode::Off))
                && !capabilities.supports(ThinkingMode::Off)
            {
                return;
            }
            if let Some(mode) = effective {
                if let Some(value) = capabilities.provider_mode_value(mode) {
                    object.insert("reasoning_effort".to_string(), json!(value));
                }
            }
        }
        ReasoningProtocol::DeepSeekThinking => {
            object.insert(
                "thinking".to_string(),
                json!({ "type": if enabled { "enabled" } else { "disabled" } }),
            );
            if let Some(mode) = effective.filter(|mode| *mode != ThinkingMode::Off) {
                if let Some(value) = capabilities.provider_mode_value(mode) {
                    object.insert("reasoning_effort".to_string(), json!(value));
                }
            }
        }
        ReasoningProtocol::OpenRouterReasoningObject => {
            let reasoning = if let Some(mode) = effective.filter(|mode| *mode != ThinkingMode::Off)
            {
                capabilities
                    .provider_mode_value(mode)
                    .map(|value| json!({ "effort": value }))
                    .unwrap_or_else(|| json!({ "enabled": false }))
            } else {
                json!({ "enabled": false })
            };
            object.insert("reasoning".to_string(), reasoning);
        }
        ReasoningProtocol::ThinkingObject | ReasoningProtocol::ZaiThinkingObject => {
            object.insert(
                "thinking".to_string(),
                json!({ "type": if enabled { "enabled" } else { "disabled" } }),
            );
        }
        ReasoningProtocol::QwenChatTemplate => {
            object.insert(
                "chat_template_args".to_string(),
                json!({ "enable_thinking": enabled, "preserve_thinking": true }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};

    use super::{apply_openai_reasoning, apply_reasoning_replay, extract_openai_reasoning_blocks};
    use crate::config::ThinkingMode;
    use crate::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};
    use crate::types::{ReasoningBlock, ReasoningSignature};

    fn capabilities(protocol: ReasoningProtocol) -> ReasoningCapabilities {
        ReasoningCapabilities {
            protocol,
            supported_modes: vec![
                ThinkingMode::Off,
                ThinkingMode::Low,
                ThinkingMode::Medium,
                ThinkingMode::High,
            ],
            default_mode: Some(ThinkingMode::Medium),
            mode_map: BTreeMap::new(),
            requires_assistant_reasoning_content: false,
        }
    }

    #[test]
    fn openai_high_writes_reasoning_effort() {
        let mut request = json!({ "model": "reasoner", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::OpenAiReasoningEffort),
            Some(ThinkingMode::High),
        );

        assert_eq!(request["reasoning_effort"], "high");
    }

    #[test]
    fn openai_off_writes_reasoning_effort_none() {
        let mut request = json!({ "model": "reasoner", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::OpenAiReasoningEffort),
            Some(ThinkingMode::Off),
        );

        assert_eq!(request["reasoning_effort"], "none");
    }

    #[test]
    fn openai_off_without_supported_mode_does_not_write_none() {
        let mut request = json!({ "model": "older-reasoner", "messages": [] });
        let mut capabilities = capabilities(ReasoningProtocol::OpenAiReasoningEffort);
        capabilities
            .supported_modes
            .retain(|mode| *mode != ThinkingMode::Off);
        capabilities.default_mode = None;

        apply_openai_reasoning(&mut request, &capabilities, Some(ThinkingMode::Off));

        assert!(request.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_high_writes_thinking_enabled_and_reasoning_effort() {
        let mut request = json!({ "model": "deepseek", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::DeepSeekThinking),
            Some(ThinkingMode::High),
        );

        assert_eq!(request["thinking"], json!({ "type": "enabled" }));
        assert_eq!(request["reasoning_effort"], "high");
    }

    #[test]
    fn deepseek_off_writes_thinking_disabled_and_omits_reasoning_effort() {
        let mut request = json!({ "model": "deepseek", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::DeepSeekThinking),
            Some(ThinkingMode::Off),
        );

        assert_eq!(request["thinking"], json!({ "type": "disabled" }));
        assert!(request.get("reasoning_effort").is_none());
    }

    #[test]
    fn openrouter_off_writes_disabled_reasoning_object() {
        let mut request = json!({ "model": "router", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::OpenRouterReasoningObject),
            Some(ThinkingMode::Off),
        );

        assert_eq!(request["reasoning"], json!({ "enabled": false }));
    }

    #[test]
    fn openrouter_enabled_writes_effort_reasoning_object() {
        let mut request = json!({ "model": "router", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::OpenRouterReasoningObject),
            Some(ThinkingMode::High),
        );

        assert_eq!(request["reasoning"], json!({ "effort": "high" }));
    }

    #[test]
    fn zai_sets_thinking_object_type() {
        let mut enabled = json!({ "model": "glm", "messages": [] });
        let mut disabled = enabled.clone();

        apply_openai_reasoning(
            &mut enabled,
            &capabilities(ReasoningProtocol::ZaiThinkingObject),
            Some(ThinkingMode::High),
        );
        apply_openai_reasoning(
            &mut disabled,
            &capabilities(ReasoningProtocol::ZaiThinkingObject),
            Some(ThinkingMode::Off),
        );

        assert_eq!(enabled["thinking"], json!({ "type": "enabled" }));
        assert_eq!(disabled["thinking"], json!({ "type": "disabled" }));
        assert!(enabled.get("enable_thinking").is_none());
        assert!(disabled.get("enable_thinking").is_none());
    }

    #[test]
    fn qwen_sets_chat_template_args() {
        let mut request = json!({ "model": "qwen", "messages": [] });

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::QwenChatTemplate),
            Some(ThinkingMode::Low),
        );

        assert_eq!(
            request["chat_template_args"],
            json!({ "enable_thinking": true, "preserve_thinking": true })
        );
    }

    #[test]
    fn none_protocol_writes_nothing() {
        let mut request = json!({ "model": "plain", "messages": [] });
        let original: Value = request.clone();

        apply_openai_reasoning(
            &mut request,
            &capabilities(ReasoningProtocol::None),
            Some(ThinkingMode::High),
        );

        assert_eq!(request, original);
    }

    #[test]
    fn non_openai_reasoning_protocols_write_nothing() {
        for protocol in [
            ReasoningProtocol::GeminiThinkingConfig,
            ReasoningProtocol::AnthropicThinkingBudget,
        ] {
            let mut request = json!({ "model": "plain", "messages": [] });
            let original: Value = request.clone();

            apply_openai_reasoning(
                &mut request,
                &capabilities(protocol),
                Some(ThinkingMode::High),
            );

            assert_eq!(request, original);
        }
    }

    #[test]
    fn extracts_first_non_empty_openai_reasoning_field() {
        let message = json!({
            "content": "visible",
            "reasoning_content": "",
            "reasoning": "kept thought",
            "reasoning_text": "ignored thought"
        });

        let blocks = extract_openai_reasoning_blocks(&message);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "kept thought");
        assert_eq!(blocks[0].redacted, false);
        assert_eq!(
            blocks[0].signature,
            Some(ReasoningSignature::OpenAiField {
                field: "reasoning".to_string()
            })
        );
    }

    #[test]
    fn extracts_reasoning_details_as_redacted_block() {
        let details = json!([{ "type": "summary", "text": "preserved provider payload" }]);
        let message = json!({
            "content": "visible",
            "reasoning_details": details
        });

        let blocks = extract_openai_reasoning_blocks(&message);

        assert_eq!(
            blocks,
            vec![ReasoningBlock {
                text: String::new(),
                signature: Some(ReasoningSignature::ReasoningDetails(details)),
                redacted: true,
            }]
        );
    }

    #[test]
    fn replays_openai_reasoning_fields_and_details() {
        let details = json!([{ "type": "encrypted", "data": "opaque" }]);
        let mut assistant = json!({
            "role": "assistant",
            "content": "visible"
        });

        apply_reasoning_replay(
            &mut assistant,
            &[
                ReasoningBlock {
                    text: "saved thought".to_string(),
                    signature: Some(ReasoningSignature::OpenAiField {
                        field: "reasoning_content".to_string(),
                    }),
                    redacted: false,
                },
                ReasoningBlock {
                    text: String::new(),
                    signature: Some(ReasoningSignature::ReasoningDetails(details.clone())),
                    redacted: true,
                },
                ReasoningBlock {
                    text: "ignored".to_string(),
                    signature: Some(ReasoningSignature::GeminiThoughtSignature(
                        "gemini".to_string(),
                    )),
                    redacted: false,
                },
            ],
            false,
        );

        assert_eq!(assistant["reasoning_content"], "saved thought");
        assert_eq!(assistant["reasoning_details"], details);
        assert!(assistant.get("thought_signature").is_none());
    }

    #[test]
    fn replay_ignores_non_whitelisted_openai_reasoning_field() {
        let mut assistant = json!({
            "role": "assistant",
            "content": "visible"
        });

        apply_reasoning_replay(
            &mut assistant,
            &[ReasoningBlock {
                text: "saved thought".to_string(),
                signature: Some(ReasoningSignature::OpenAiField {
                    field: "unexpected_field".to_string(),
                }),
                redacted: false,
            }],
            false,
        );

        assert!(assistant.get("unexpected_field").is_none());
    }

    #[test]
    fn replay_writes_empty_reasoning_content_when_required() {
        let mut assistant = json!({
            "role": "assistant",
            "content": "visible"
        });

        apply_reasoning_replay(&mut assistant, &[], true);

        assert_eq!(assistant["reasoning_content"], "");
    }
}
