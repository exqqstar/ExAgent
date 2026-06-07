use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::ThinkingMode;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningProtocol {
    #[default]
    None,
    OpenAiReasoningEffort,
    DeepSeekThinking,
    ThinkingObject,
    OpenRouterReasoningObject,
    #[serde(alias = "zai_enable_thinking")]
    ZaiThinkingObject,
    QwenChatTemplate,
    GeminiThinkingConfig,
    AnthropicThinkingBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningCapabilities {
    pub protocol: ReasoningProtocol,
    pub supported_modes: Vec<ThinkingMode>,
    pub default_mode: Option<ThinkingMode>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mode_map: BTreeMap<ThinkingMode, String>,
    pub requires_assistant_reasoning_content: bool,
}

impl Default for ReasoningCapabilities {
    fn default() -> Self {
        Self {
            protocol: ReasoningProtocol::None,
            supported_modes: Vec::new(),
            default_mode: None,
            mode_map: BTreeMap::new(),
            requires_assistant_reasoning_content: false,
        }
    }
}

impl ReasoningCapabilities {
    pub fn unsupported() -> Self {
        Self::default()
    }

    pub fn supports(&self, mode: ThinkingMode) -> bool {
        self.supported_modes.contains(&mode)
    }

    pub fn effective_mode(&self, requested: Option<ThinkingMode>) -> Option<ThinkingMode> {
        match requested {
            None | Some(ThinkingMode::Auto) => self.supported_default_mode(),
            Some(mode) if self.supports(mode) => Some(mode),
            Some(_) => self.supported_default_mode(),
        }
    }

    fn supported_default_mode(&self) -> Option<ThinkingMode> {
        self.default_mode.filter(|mode| self.supports(*mode))
    }

    pub fn provider_mode_value(&self, mode: ThinkingMode) -> Option<&str> {
        self.mode_map
            .get(&mode)
            .map(String::as_str)
            .or_else(|| match mode {
                ThinkingMode::Off => Some("none"),
                ThinkingMode::Minimal => Some("minimal"),
                ThinkingMode::Low => Some("low"),
                ThinkingMode::Medium => Some("medium"),
                ThinkingMode::High => Some("high"),
                ThinkingMode::XHigh => Some("xhigh"),
                ThinkingMode::Auto => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> ReasoningCapabilities {
        ReasoningCapabilities {
            protocol: ReasoningProtocol::OpenRouterReasoningObject,
            supported_modes: vec![
                ThinkingMode::Minimal,
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
    fn effective_mode_uses_default_for_auto_or_missing_request() {
        let capabilities = capabilities();

        assert_eq!(
            capabilities.effective_mode(None),
            Some(ThinkingMode::Medium)
        );
        assert_eq!(
            capabilities.effective_mode(Some(ThinkingMode::Auto)),
            Some(ThinkingMode::Medium)
        );
    }

    #[test]
    fn effective_mode_keeps_supported_requested_mode() {
        let capabilities = capabilities();

        assert_eq!(
            capabilities.effective_mode(Some(ThinkingMode::XHigh)),
            Some(ThinkingMode::XHigh)
        );
    }

    #[test]
    fn effective_mode_falls_back_to_default_for_unsupported_modes() {
        let mut capabilities = capabilities();
        capabilities.supported_modes = vec![ThinkingMode::Low, ThinkingMode::Medium];

        assert_eq!(
            capabilities.effective_mode(Some(ThinkingMode::High)),
            Some(ThinkingMode::Medium)
        );
    }

    #[test]
    fn effective_mode_falls_back_for_unsupported_off() {
        let capabilities = capabilities();

        assert_eq!(
            capabilities.effective_mode(Some(ThinkingMode::Off)),
            Some(ThinkingMode::Medium)
        );
    }

    #[test]
    fn effective_mode_returns_none_for_unsupported_capabilities_without_default() {
        let capabilities = ReasoningCapabilities::unsupported();

        assert_eq!(capabilities.effective_mode(None), None);
        assert_eq!(capabilities.effective_mode(Some(ThinkingMode::High)), None);
    }

    #[test]
    fn effective_mode_returns_none_for_unsupported_default() {
        let mut capabilities = capabilities();
        capabilities.supported_modes = vec![ThinkingMode::Low, ThinkingMode::Medium];
        capabilities.default_mode = Some(ThinkingMode::High);

        assert_eq!(capabilities.effective_mode(None), None);
        assert_eq!(capabilities.effective_mode(Some(ThinkingMode::Auto)), None);
        assert_eq!(capabilities.effective_mode(Some(ThinkingMode::XHigh)), None);
    }

    #[test]
    fn reasoning_protocol_serializes_wire_names() {
        assert_eq!(
            serde_json::to_string(&ReasoningProtocol::OpenAiReasoningEffort).unwrap(),
            "\"open_ai_reasoning_effort\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningProtocol::DeepSeekThinking).unwrap(),
            "\"deep_seek_thinking\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningProtocol::ThinkingObject).unwrap(),
            "\"thinking_object\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningProtocol::ZaiThinkingObject).unwrap(),
            "\"zai_thinking_object\""
        );
    }

    #[test]
    fn reasoning_protocol_deserializes_wire_names() {
        assert_eq!(
            serde_json::from_str::<ReasoningProtocol>("\"open_ai_reasoning_effort\"").unwrap(),
            ReasoningProtocol::OpenAiReasoningEffort
        );
        assert_eq!(
            serde_json::from_str::<ReasoningProtocol>("\"deep_seek_thinking\"").unwrap(),
            ReasoningProtocol::DeepSeekThinking
        );
        assert_eq!(
            serde_json::from_str::<ReasoningProtocol>("\"thinking_object\"").unwrap(),
            ReasoningProtocol::ThinkingObject
        );
        assert_eq!(
            serde_json::from_str::<ReasoningProtocol>("\"zai_enable_thinking\"").unwrap(),
            ReasoningProtocol::ZaiThinkingObject
        );
        assert_eq!(
            serde_json::from_str::<ReasoningProtocol>("\"zai_thinking_object\"").unwrap(),
            ReasoningProtocol::ZaiThinkingObject
        );
    }

    #[test]
    fn provider_mode_value_uses_mode_map_override() {
        let mut capabilities = capabilities();
        capabilities
            .mode_map
            .insert(ThinkingMode::XHigh, "extra_high".to_string());

        assert_eq!(
            capabilities.provider_mode_value(ThinkingMode::XHigh),
            Some("extra_high")
        );
    }

    #[test]
    fn provider_mode_value_uses_default_mode_strings() {
        let capabilities = ReasoningCapabilities::unsupported();

        assert_eq!(
            capabilities.provider_mode_value(ThinkingMode::Off),
            Some("none")
        );
        assert_eq!(
            capabilities.provider_mode_value(ThinkingMode::Minimal),
            Some("minimal")
        );
        assert_eq!(
            capabilities.provider_mode_value(ThinkingMode::XHigh),
            Some("xhigh")
        );
        assert_eq!(capabilities.provider_mode_value(ThinkingMode::Auto), None);
    }
}
