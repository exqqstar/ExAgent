use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::provider::{provider_profile_by_id, ProviderProtocol};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }

    pub fn display(&self) -> String {
        format!("{}:{}", self.provider_id, self.model_id)
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub base_url: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ResolvedCredential {
    None,
    ApiKey(String),
    BearerToken(String),
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolvedCredential::None => formatter.write_str("None"),
            ResolvedCredential::ApiKey(_) => formatter.write_str("ApiKey(***)"),
            ResolvedCredential::BearerToken(_) => formatter.write_str("BearerToken(***)"),
        }
    }
}

impl Default for ResolvedCredential {
    fn default() -> Self {
        ResolvedCredential::None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub context_window: Option<i64>,
    pub supports_tools: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModelConfig {
    pub identity: ModelRef,
    pub protocol: ProviderProtocol,
    pub endpoint: ProviderEndpoint,
    pub credential: ResolvedCredential,
    pub capabilities: ModelCapabilities,
}

impl ResolvedModelConfig {
    pub fn from_provider_profile(
        provider_id: &str,
        model_id: impl Into<String>,
        base_url: Option<String>,
        credential: ResolvedCredential,
        context_window: Option<i64>,
    ) -> Self {
        let profile = provider_profile_by_id(provider_id)
            .unwrap_or_else(|| panic!("unknown provider profile {provider_id}"));

        Self {
            identity: ModelRef::new(profile.id, model_id),
            protocol: profile.protocol,
            endpoint: ProviderEndpoint {
                base_url: base_url.or_else(|| {
                    (!profile.default_base_url.is_empty())
                        .then(|| profile.default_base_url.to_string())
                }),
            },
            credential,
            capabilities: ModelCapabilities {
                context_window,
                supports_tools: profile.supports_tools,
            },
        }
    }
}

impl Default for ResolvedModelConfig {
    fn default() -> Self {
        let profile = provider_profile_by_id("openai").expect("openai profile exists");
        Self::from_provider_profile(
            profile.id,
            profile.default_model,
            None,
            ResolvedCredential::None,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ref_display_uses_provider_and_model() {
        let model = ModelRef {
            provider_id: "openai".to_string(),
            model_id: "gpt-4.1".to_string(),
        };

        assert_eq!(model.display(), "openai:gpt-4.1");
    }

    #[test]
    fn resolved_credential_debug_redacts_secret_values() {
        let credential = ResolvedCredential::ApiKey("sk-secret".to_string());
        let debug = format!("{credential:?}");

        assert!(debug.contains("***"));
        assert!(!debug.contains("sk-secret"));
    }

    #[test]
    fn bearer_token_debug_redacts_secret_values() {
        let credential = ResolvedCredential::BearerToken("bearer-secret".to_string());
        let debug = format!("{credential:?}");

        assert!(debug.contains("***"));
        assert!(!debug.contains("bearer-secret"));
    }

    #[test]
    fn resolved_model_debug_redacts_nested_credentials() {
        let model = ResolvedModelConfig::from_provider_profile(
            "openai",
            "gpt-4.1",
            None,
            ResolvedCredential::ApiKey("sk-secret".to_string()),
            None,
        );
        let debug = format!("{model:?}");

        assert!(debug.contains("***"));
        assert!(!debug.contains("sk-secret"));
    }

    #[test]
    #[should_panic(expected = "unknown provider profile missing")]
    fn from_provider_profile_panics_for_unknown_provider() {
        let _ = ResolvedModelConfig::from_provider_profile(
            "missing",
            "model",
            None,
            ResolvedCredential::None,
            None,
        );
    }
}
