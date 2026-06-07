use std::collections::BTreeMap;

use exagent::config::ThinkingMode;
use exagent::model::reasoning::{ReasoningCapabilities, ReasoningProtocol};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingCapability {
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub modes: Vec<ThinkingMode>,
}

impl Default for ThinkingCapability {
    fn default() -> Self {
        Self {
            supported: false,
            modes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub thinking: ThinkingCapability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<CatalogReasoning>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self::unsupported()
    }
}

impl ModelCapabilities {
    pub fn unsupported() -> Self {
        Self {
            supports_tools: false,
            thinking: ThinkingCapability::default(),
            reasoning: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogReasoning {
    pub protocol: ReasoningProtocol,
    pub supported_modes: Vec<ThinkingMode>,
    pub default_mode: Option<ThinkingMode>,
    pub requires_assistant_reasoning_content: bool,
}

impl From<CatalogReasoning> for ReasoningCapabilities {
    fn from(reasoning: CatalogReasoning) -> Self {
        Self {
            protocol: reasoning.protocol,
            supported_modes: reasoning.supported_modes,
            default_mode: reasoning.default_mode,
            mode_map: BTreeMap::new(),
            requires_assistant_reasoning_content: reasoning.requires_assistant_reasoning_content,
        }
    }
}

impl CatalogReasoning {
    fn from_static(reasoning: StaticCatalogReasoning) -> Self {
        Self {
            protocol: reasoning.protocol,
            supported_modes: reasoning.supported_modes.to_vec(),
            default_mode: reasoning.default_mode,
            requires_assistant_reasoning_content: reasoning.requires_assistant_reasoning_content,
        }
    }

    pub fn from_capabilities(reasoning: &ReasoningCapabilities) -> Option<Self> {
        (reasoning.protocol != ReasoningProtocol::None).then(|| Self {
            protocol: reasoning.protocol,
            supported_modes: reasoning.supported_modes.clone(),
            default_mode: reasoning.default_mode,
            requires_assistant_reasoning_content: reasoning.requires_assistant_reasoning_content,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogModel {
    pub provider_id: &'static str,
    pub id: &'static str,
    pub display_name: &'static str,
    pub context_window: Option<i64>,
    pub capabilities: StaticModelCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticModelCapabilities {
    pub supports_tools: bool,
    pub thinking: StaticThinkingCapability,
    pub reasoning: Option<StaticCatalogReasoning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticThinkingCapability {
    pub supported: bool,
    pub modes: &'static [ThinkingMode],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticCatalogReasoning {
    pub protocol: ReasoningProtocol,
    pub supported_modes: &'static [ThinkingMode],
    pub default_mode: Option<ThinkingMode>,
    pub requires_assistant_reasoning_content: bool,
}

const NO_THINKING: StaticThinkingCapability = StaticThinkingCapability {
    supported: false,
    modes: &[],
};

const GPT_5_THINKING: StaticThinkingCapability = StaticThinkingCapability {
    supported: true,
    modes: &[ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::High],
};

const GPT_5_WITH_OFF_THINKING: StaticThinkingCapability = StaticThinkingCapability {
    supported: true,
    modes: &[
        ThinkingMode::Off,
        ThinkingMode::Low,
        ThinkingMode::Medium,
        ThinkingMode::High,
    ],
};

const GPT_5_XHIGH_THINKING: StaticThinkingCapability = StaticThinkingCapability {
    supported: true,
    modes: &[
        ThinkingMode::Off,
        ThinkingMode::Low,
        ThinkingMode::Medium,
        ThinkingMode::High,
        ThinkingMode::XHigh,
    ],
};

const STANDARD_REASONING_MODES: &[ThinkingMode] = &[
    ThinkingMode::Off,
    ThinkingMode::Low,
    ThinkingMode::Medium,
    ThinkingMode::High,
];

const OPENAI_REASONING_WITHOUT_OFF_MODES: &[ThinkingMode] =
    &[ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::High];

const OPENAI_XHIGH_REASONING_MODES: &[ThinkingMode] = &[
    ThinkingMode::Off,
    ThinkingMode::Low,
    ThinkingMode::Medium,
    ThinkingMode::High,
    ThinkingMode::XHigh,
];

const OPENAI_XHIGH_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::OpenAiReasoningEffort,
    supported_modes: OPENAI_XHIGH_REASONING_MODES,
    default_mode: Some(ThinkingMode::Medium),
    requires_assistant_reasoning_content: false,
};

const OPENAI_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::OpenAiReasoningEffort,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::Medium),
    requires_assistant_reasoning_content: false,
};

const OPENAI_REASONING_WITHOUT_OFF: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::OpenAiReasoningEffort,
    supported_modes: OPENAI_REASONING_WITHOUT_OFF_MODES,
    default_mode: Some(ThinkingMode::Medium),
    requires_assistant_reasoning_content: false,
};

const DEEPSEEK_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::DeepSeekThinking,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::Off),
    requires_assistant_reasoning_content: true,
};

const GOOGLE_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::GeminiThinkingConfig,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::High),
    requires_assistant_reasoning_content: false,
};

const ANTHROPIC_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::AnthropicThinkingBudget,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::Medium),
    requires_assistant_reasoning_content: false,
};

const GLM_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::ZaiThinkingObject,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::Off),
    requires_assistant_reasoning_content: false,
};

const KIMI_REASONING: StaticCatalogReasoning = StaticCatalogReasoning {
    protocol: ReasoningProtocol::ThinkingObject,
    supported_modes: STANDARD_REASONING_MODES,
    default_mode: Some(ThinkingMode::Off),
    requires_assistant_reasoning_content: false,
};

const MODEL_CATALOG: &[CatalogModel] = &[
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5.5",
        display_name: "gpt-5.5",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_XHIGH_THINKING,
            reasoning: Some(OPENAI_XHIGH_REASONING),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5.4",
        display_name: "gpt-5.4",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_XHIGH_THINKING,
            reasoning: Some(OPENAI_XHIGH_REASONING),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5.4-mini",
        display_name: "gpt-5.4-mini",
        context_window: Some(400_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_XHIGH_THINKING,
            reasoning: Some(OPENAI_XHIGH_REASONING),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5.1",
        display_name: "gpt-5.1",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_WITH_OFF_THINKING,
            reasoning: Some(OPENAI_REASONING),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5",
        display_name: "gpt-5",
        context_window: Some(400_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_THINKING,
            reasoning: Some(OPENAI_REASONING_WITHOUT_OFF),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5-mini",
        display_name: "gpt-5-mini",
        context_window: Some(400_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_THINKING,
            reasoning: Some(OPENAI_REASONING_WITHOUT_OFF),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-5-nano",
        display_name: "gpt-5-nano",
        context_window: Some(400_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_THINKING,
            reasoning: Some(OPENAI_REASONING_WITHOUT_OFF),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-4.1",
        display_name: "gpt-4.1",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: None,
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-4.1-mini",
        display_name: "gpt-4.1-mini",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: None,
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "gpt-4.1-nano",
        display_name: "gpt-4.1-nano",
        context_window: Some(1_047_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: None,
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "o3",
        display_name: "o3",
        context_window: Some(200_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_THINKING,
            reasoning: Some(OPENAI_REASONING_WITHOUT_OFF),
        },
    },
    CatalogModel {
        provider_id: "openai",
        id: "o4-mini",
        display_name: "o4-mini",
        context_window: Some(200_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: GPT_5_THINKING,
            reasoning: Some(OPENAI_REASONING_WITHOUT_OFF),
        },
    },
    CatalogModel {
        provider_id: "anthropic",
        id: "claude-sonnet-4-6",
        display_name: "claude-sonnet-4-6",
        context_window: Some(1_000_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(ANTHROPIC_REASONING),
        },
    },
    CatalogModel {
        provider_id: "google",
        id: "gemini-3-pro-preview",
        display_name: "gemini-3-pro-preview",
        context_window: Some(1_048_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(GOOGLE_REASONING),
        },
    },
    CatalogModel {
        provider_id: "google",
        id: "gemini-2.5-pro",
        display_name: "gemini-2.5-pro",
        context_window: Some(1_048_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(GOOGLE_REASONING),
        },
    },
    CatalogModel {
        provider_id: "google",
        id: "gemini-2.5-flash",
        display_name: "gemini-2.5-flash",
        context_window: Some(1_048_576),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(GOOGLE_REASONING),
        },
    },
    CatalogModel {
        provider_id: "deepseek",
        id: "deepseek-v4-flash",
        display_name: "deepseek-v4-flash",
        context_window: Some(1_000_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(DEEPSEEK_REASONING),
        },
    },
    CatalogModel {
        provider_id: "deepseek",
        id: "deepseek-v4-pro",
        display_name: "deepseek-v4-pro",
        context_window: Some(1_000_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(DEEPSEEK_REASONING),
        },
    },
    CatalogModel {
        provider_id: "kimi",
        id: "kimi-k2.6",
        display_name: "kimi-k2.6",
        context_window: Some(256_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(KIMI_REASONING),
        },
    },
    CatalogModel {
        provider_id: "kimi",
        id: "kimi-k2.5",
        display_name: "kimi-k2.5",
        context_window: Some(256_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(KIMI_REASONING),
        },
    },
    CatalogModel {
        provider_id: "glm",
        id: "glm-5.1",
        display_name: "glm-5.1",
        context_window: Some(200_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(GLM_REASONING),
        },
    },
    CatalogModel {
        provider_id: "glm",
        id: "glm-5",
        display_name: "glm-5",
        context_window: Some(200_000),
        capabilities: StaticModelCapabilities {
            supports_tools: true,
            thinking: NO_THINKING,
            reasoning: Some(GLM_REASONING),
        },
    },
];

pub fn catalog_models_for_provider(
    provider_id: &str,
) -> impl Iterator<Item = &'static CatalogModel> + '_ {
    MODEL_CATALOG
        .iter()
        .filter(move |model| model.provider_id == provider_id)
}

pub fn capabilities_for_model(
    provider_id: &str,
    model_id: &str,
    provider_supports_tools: bool,
    discovered_supports_tools: Option<bool>,
) -> ModelCapabilities {
    if let Some(catalog_model) = catalog_model(provider_id, model_id) {
        let mut capabilities = capabilities_from_static(catalog_model.capabilities);
        if let Some(supports_tools) = discovered_supports_tools {
            capabilities.supports_tools = supports_tools;
        }
        return capabilities;
    }

    ModelCapabilities {
        supports_tools: discovered_supports_tools
            .unwrap_or_else(|| default_unknown_tool_support(provider_id, provider_supports_tools)),
        thinking: ThinkingCapability::default(),
        reasoning: None,
    }
}

fn catalog_model(provider_id: &str, model_id: &str) -> Option<&'static CatalogModel> {
    MODEL_CATALOG
        .iter()
        .find(|model| model.provider_id == provider_id && model.id == model_id)
}

fn capabilities_from_static(capabilities: StaticModelCapabilities) -> ModelCapabilities {
    let reasoning = capabilities.reasoning.map(CatalogReasoning::from_static);
    let thinking = reasoning
        .as_ref()
        .filter(|reasoning| !reasoning.supported_modes.is_empty())
        .map(|reasoning| ThinkingCapability {
            supported: true,
            modes: reasoning.supported_modes.clone(),
        })
        .unwrap_or_else(|| ThinkingCapability {
            supported: capabilities.thinking.supported,
            modes: capabilities.thinking.modes.to_vec(),
        });

    ModelCapabilities {
        supports_tools: capabilities.supports_tools,
        thinking,
        reasoning,
    }
}

fn default_unknown_tool_support(_provider_id: &str, _provider_supports_tools: bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_gpt_4_1_does_not_support_thinking() {
        let capabilities = capabilities_for_model("openai", "gpt-4.1", true, None);

        assert!(capabilities.supports_tools);
        assert!(!capabilities.thinking.supported);
        assert!(capabilities.thinking.modes.is_empty());
        assert!(capabilities.reasoning.is_none());
    }

    #[test]
    fn openai_gpt_5_supports_thinking_modes() {
        let capabilities = capabilities_for_model("openai", "gpt-5.5", true, None);

        assert!(capabilities.supports_tools);
        assert!(capabilities.thinking.supported);
        assert_eq!(
            capabilities.thinking.modes,
            vec![
                ThinkingMode::Off,
                ThinkingMode::Low,
                ThinkingMode::Medium,
                ThinkingMode::High,
                ThinkingMode::XHigh
            ]
        );
        let reasoning = capabilities
            .reasoning
            .expect("gpt-5.5 should expose reasoning metadata");
        assert_eq!(reasoning.protocol, ReasoningProtocol::OpenAiReasoningEffort);
        assert_eq!(reasoning.default_mode, Some(ThinkingMode::Medium));
        assert_eq!(
            reasoning.supported_modes,
            vec![
                ThinkingMode::Off,
                ThinkingMode::Low,
                ThinkingMode::Medium,
                ThinkingMode::High,
                ThinkingMode::XHigh
            ]
        );
    }

    #[test]
    fn openai_older_reasoning_models_do_not_support_off() {
        for model_id in ["gpt-5", "gpt-5-mini", "gpt-5-nano", "o3", "o4-mini"] {
            let capabilities = capabilities_for_model("openai", model_id, true, None);
            let reasoning = capabilities
                .reasoning
                .expect("older reasoning model should expose reasoning metadata");

            assert_eq!(reasoning.protocol, ReasoningProtocol::OpenAiReasoningEffort);
            assert!(
                !reasoning.supported_modes.contains(&ThinkingMode::Off),
                "{model_id} should not expose reasoning_effort none"
            );
            assert!(reasoning.supported_modes.contains(&ThinkingMode::High));
        }
    }

    #[test]
    fn openai_current_reasoning_models_support_off() {
        for model_id in ["gpt-5.5", "gpt-5.4", "gpt-5.1"] {
            let capabilities = capabilities_for_model("openai", model_id, true, None);
            let reasoning = capabilities
                .reasoning
                .expect("current reasoning model should expose reasoning metadata");

            assert!(
                reasoning.supported_modes.contains(&ThinkingMode::Off),
                "{model_id} should expose reasoning_effort none"
            );
        }
    }

    #[test]
    fn provider_catalog_includes_current_vendor_defaults() {
        let models = [
            ("anthropic", "claude-sonnet-4-6", Some(1_000_000)),
            ("google", "gemini-3-pro-preview", Some(1_048_576)),
            ("deepseek", "deepseek-v4-flash", Some(1_000_000)),
            ("kimi", "kimi-k2.6", Some(256_000)),
            ("glm", "glm-5.1", Some(200_000)),
        ];

        for (provider_id, model_id, context_window) in models {
            let model = catalog_models_for_provider(provider_id)
                .find(|model| model.id == model_id)
                .expect("provider default model should be catalogued");

            assert_eq!(model.context_window, context_window);
            assert!(model.capabilities.supports_tools);
            assert!(!model.capabilities.thinking.supported);
            assert!(model.capabilities.reasoning.is_some());
        }
    }

    #[test]
    fn kimi_catalog_uses_thinking_object_app_default_off() {
        let model = catalog_models_for_provider("kimi")
            .find(|model| model.id == "kimi-k2.6")
            .expect("provider default model should be catalogued");
        let reasoning = model
            .capabilities
            .reasoning
            .expect("provider default model should expose reasoning metadata");

        assert_eq!(
            serde_json::to_value(reasoning.protocol).unwrap(),
            serde_json::json!("thinking_object")
        );
        assert_eq!(reasoning.default_mode, Some(ThinkingMode::Off));
    }

    #[test]
    fn glm_catalog_uses_zai_thinking_object_app_default_off() {
        let model = catalog_models_for_provider("glm")
            .find(|model| model.id == "glm-5.1")
            .expect("provider default model should be catalogued");
        let reasoning = model
            .capabilities
            .reasoning
            .expect("provider default model should expose reasoning metadata");

        assert_eq!(reasoning.protocol, ReasoningProtocol::ZaiThinkingObject);
        assert_eq!(reasoning.default_mode, Some(ThinkingMode::Off));
    }

    #[test]
    fn discovered_tool_support_overrides_provider_default() {
        let capabilities =
            capabilities_for_model("openai_compatible", "local-model", true, Some(false));

        assert!(!capabilities.supports_tools);
    }

    #[test]
    fn openai_compatible_gpt_5_is_not_treated_as_openai_catalog_model() {
        let capabilities = capabilities_for_model("openai_compatible", "gpt-5", true, None);

        assert!(!capabilities.supports_tools);
        assert!(!capabilities.thinking.supported);
        assert!(capabilities.thinking.modes.is_empty());
        assert!(capabilities.reasoning.is_none());
    }

    #[test]
    fn openai_compatible_gpt_4_1_mini_is_not_treated_as_openai_catalog_model() {
        let capabilities = capabilities_for_model("openai_compatible", "gpt-4.1-mini", true, None);

        assert!(!capabilities.supports_tools);
        assert!(!capabilities.thinking.supported);
        assert!(capabilities.thinking.modes.is_empty());
        assert!(capabilities.reasoning.is_none());
    }

    #[test]
    fn openai_uncatalogued_model_is_conservative() {
        let capabilities = capabilities_for_model("openai", "gpt-future", true, None);

        assert!(!capabilities.supports_tools);
        assert!(!capabilities.thinking.supported);
        assert!(capabilities.thinking.modes.is_empty());
        assert!(capabilities.reasoning.is_none());
    }
}
