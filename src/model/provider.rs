use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::ThinkingMode;
use crate::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub protocol: ProviderProtocol,
    pub auth_mode: ProviderAuthMode,
    pub default_base_url: &'static str,
    pub default_model: &'static str,
    pub supports_model_discovery: bool,
    pub supports_tools: bool,
    pub supported: bool,
    pub unsupported_reason: Option<&'static str>,
    pub recommended: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    #[serde(rename = "copilot_oauth")]
    CopilotOAuth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthMode {
    ApiKeyRequired,
    ApiKeyOptional,
    OAuthRequired,
    OAuthPlanned,
}

impl ProviderProfile {
    pub fn reasoning_capabilities(&self) -> ReasoningCapabilities {
        self.reasoning_capabilities_for_model(self.default_model)
    }

    pub fn reasoning_capabilities_for_model(&self, model_id: &str) -> ReasoningCapabilities {
        if self.id == "openai" {
            return openai_reasoning_capabilities_for_model(model_id);
        }

        self.default_reasoning_capabilities()
    }

    fn default_reasoning_capabilities(&self) -> ReasoningCapabilities {
        match self.id {
            "openai" => current_openai_xhigh_reasoning_capabilities(),
            "deepseek" => ReasoningCapabilities {
                protocol: ReasoningProtocol::DeepSeekThinking,
                supported_modes: standard_reasoning_modes(),
                default_mode: Some(ThinkingMode::Off),
                mode_map: BTreeMap::new(),
                requires_assistant_reasoning_content: true,
            },
            "google" => ReasoningCapabilities {
                protocol: ReasoningProtocol::GeminiThinkingConfig,
                supported_modes: standard_reasoning_modes(),
                default_mode: Some(ThinkingMode::High),
                mode_map: BTreeMap::new(),
                requires_assistant_reasoning_content: false,
            },
            "anthropic" => ReasoningCapabilities {
                protocol: ReasoningProtocol::AnthropicThinkingBudget,
                supported_modes: standard_reasoning_modes(),
                default_mode: Some(ThinkingMode::Medium),
                mode_map: BTreeMap::new(),
                requires_assistant_reasoning_content: false,
            },
            "glm" => ReasoningCapabilities {
                protocol: ReasoningProtocol::ZaiThinkingObject,
                supported_modes: standard_reasoning_modes(),
                default_mode: Some(ThinkingMode::Off),
                mode_map: BTreeMap::new(),
                requires_assistant_reasoning_content: false,
            },
            "kimi" => ReasoningCapabilities {
                protocol: ReasoningProtocol::ThinkingObject,
                supported_modes: standard_reasoning_modes(),
                default_mode: Some(ThinkingMode::Off),
                mode_map: BTreeMap::new(),
                requires_assistant_reasoning_content: false,
            },
            _ => ReasoningCapabilities::unsupported(),
        }
    }
}

fn openai_reasoning_capabilities_for_model(model_id: &str) -> ReasoningCapabilities {
    if is_known_openai_non_reasoning_model(model_id) {
        ReasoningCapabilities::unsupported()
    } else if is_openai_reasoning_model_without_off(model_id) {
        conservative_openai_reasoning_capabilities(Some(ThinkingMode::Medium))
    } else if is_current_openai_xhigh_reasoning_model(model_id) {
        current_openai_xhigh_reasoning_capabilities()
    } else if matches!(model_id, "gpt-5.1") {
        ReasoningCapabilities {
            protocol: ReasoningProtocol::OpenAiReasoningEffort,
            supported_modes: standard_reasoning_modes(),
            default_mode: Some(ThinkingMode::Medium),
            mode_map: BTreeMap::new(),
            requires_assistant_reasoning_content: false,
        }
    } else {
        conservative_openai_reasoning_capabilities(Some(ThinkingMode::Medium))
    }
}

fn current_openai_xhigh_reasoning_capabilities() -> ReasoningCapabilities {
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

fn conservative_openai_reasoning_capabilities(
    default_mode: Option<ThinkingMode>,
) -> ReasoningCapabilities {
    ReasoningCapabilities {
        protocol: ReasoningProtocol::OpenAiReasoningEffort,
        supported_modes: vec![ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::High],
        default_mode,
        mode_map: BTreeMap::new(),
        requires_assistant_reasoning_content: false,
    }
}

fn is_known_openai_non_reasoning_model(model_id: &str) -> bool {
    ["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"]
        .iter()
        .any(|family| model_id == *family || model_id.starts_with(&format!("{family}-")))
}

fn is_current_openai_xhigh_reasoning_model(model_id: &str) -> bool {
    matches!(model_id, "gpt-5.5" | "gpt-5.4" | "gpt-5.4-mini")
}

fn is_openai_reasoning_model_without_off(model_id: &str) -> bool {
    matches!(
        model_id,
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" | "o3" | "o4-mini"
    )
}

fn standard_reasoning_modes() -> Vec<ThinkingMode> {
    vec![
        ThinkingMode::Off,
        ThinkingMode::Low,
        ThinkingMode::Medium,
        ThinkingMode::High,
    ]
}

const PROVIDER_PROFILES: &[ProviderProfile] = &[
    ProviderProfile {
        id: "openai",
        name: "OpenAI",
        description: "Use ChatGPT Pro/Plus or an API key",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.openai.com/v1",
        default_model: "gpt-5.5",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: true,
    },
    ProviderProfile {
        id: "openai_compatible",
        name: "OpenAI Compatible",
        description: "Use OpenRouter, DeepSeek, local gateways, or another compatible endpoint",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyOptional,
        default_base_url: "http://127.0.0.1:11434/v1",
        default_model: "local-model",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "anthropic",
        name: "Anthropic",
        description: "Use Claude Pro/Max or an API key",
        protocol: ProviderProtocol::AnthropicMessages,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-6",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "google",
        name: "Google",
        description: "Use Gemini models with a Google API key",
        protocol: ProviderProtocol::GeminiGenerateContent,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        default_model: "gemini-3-pro-preview",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "deepseek",
        name: "DeepSeek",
        description: "Use DeepSeek API with an API key",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.deepseek.com",
        default_model: "deepseek-v4-flash",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "kimi",
        name: "Kimi",
        description: "Use Kimi API with a Moonshot API key",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.moonshot.ai/v1",
        default_model: "kimi-k2.6",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "glm",
        name: "GLM",
        description: "Use GLM API with a Zhipu API key",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        default_model: "glm-5.1",
        supports_model_discovery: true,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
    ProviderProfile {
        id: "github_copilot",
        name: "GitHub Copilot",
        description: "Use GitHub Copilot with device OAuth",
        protocol: ProviderProtocol::CopilotOAuth,
        auth_mode: ProviderAuthMode::OAuthRequired,
        default_base_url: "https://api.githubcopilot.com",
        default_model: "gpt-5.1-copilot",
        supports_model_discovery: false,
        supports_tools: true,
        supported: true,
        unsupported_reason: None,
        recommended: false,
    },
];

pub fn provider_profiles() -> &'static [ProviderProfile] {
    PROVIDER_PROFILES
}

pub fn provider_profile_by_id(provider_id: &str) -> Option<&'static ProviderProfile> {
    PROVIDER_PROFILES
        .iter()
        .find(|profile| profile.id == provider_id)
}
