use serde::{Deserialize, Serialize};

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
    OAuthPlanned,
}

const PROVIDER_PROFILES: &[ProviderProfile] = &[
    ProviderProfile {
        id: "openai",
        name: "OpenAI",
        description: "Use ChatGPT Pro/Plus or an API key",
        protocol: ProviderProtocol::OpenAiChatCompletions,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.openai.com/v1",
        default_model: "gpt-4.1",
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
        recommended: true,
    },
    ProviderProfile {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude API support is planned",
        protocol: ProviderProtocol::AnthropicMessages,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet",
        supports_model_discovery: false,
        supports_tools: true,
        supported: false,
        unsupported_reason: Some("Anthropic Messages adapter is planned."),
        recommended: false,
    },
    ProviderProfile {
        id: "google",
        name: "Google",
        description: "Gemini API support is planned",
        protocol: ProviderProtocol::GeminiGenerateContent,
        auth_mode: ProviderAuthMode::ApiKeyRequired,
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        default_model: "gemini-2.5-pro",
        supports_model_discovery: true,
        supports_tools: true,
        supported: false,
        unsupported_reason: Some("Gemini Generate Content adapter is planned."),
        recommended: false,
    },
    ProviderProfile {
        id: "github_copilot",
        name: "GitHub Copilot",
        description: "Copilot account support is planned",
        protocol: ProviderProtocol::CopilotOAuth,
        auth_mode: ProviderAuthMode::OAuthPlanned,
        default_base_url: "",
        default_model: "",
        supports_model_discovery: false,
        supports_tools: false,
        supported: false,
        unsupported_reason: Some("Copilot OAuth support is planned."),
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
