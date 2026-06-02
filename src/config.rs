use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::resolved::ResolvedModelConfig;
use crate::policy::PolicyMode;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    Auto,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: ResolvedModelConfig,
    pub thinking_mode: Option<ThinkingMode>,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub policy_mode: PolicyMode,
    pub auto_compact_token_limit: Option<i64>,
}

impl AgentConfig {
    pub fn resolved_auto_compact_token_limit(&self) -> Option<i64> {
        let context_limit = self.model.capabilities.context_window.map(ninety_percent);

        match (self.auto_compact_token_limit, context_limit) {
            (Some(configured), Some(context_limit)) => Some(configured.min(context_limit)),
            (Some(configured), None) => Some(configured),
            (None, Some(context_limit)) => Some(context_limit),
            (None, None) => None,
        }
    }
}

fn ninety_percent(value: i64) -> i64 {
    ((i128::from(value) * 9) / 10).min(i128::from(i64::MAX)) as i64
}

impl Default for AgentConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            model: ResolvedModelConfig::default(),
            thinking_mode: parse_optional_thinking_mode_env("EXAGENT_THINKING_MODE"),
            workspace_root: cwd.clone(),
            cwd,
            command_timeout_secs: 30,
            max_output_bytes: 8 * 1024,
            policy_mode: std::env::var("EXAGENT_POLICY_MODE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            auto_compact_token_limit: parse_optional_i64_env("EXAGENT_AUTO_COMPACT_TOKEN_LIMIT"),
        }
    }
}

fn parse_optional_i64_env(key: &str) -> Option<i64> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_positive_i64(value.trim()))
}

fn parse_optional_thinking_mode_env(key: &str) -> Option<ThinkingMode> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_thinking_mode_value(value.trim()))
}

fn parse_positive_i64(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn parse_thinking_mode_value(value: &str) -> Option<ThinkingMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(ThinkingMode::Auto),
        "low" => Some(ThinkingMode::Low),
        "medium" => Some(ThinkingMode::Medium),
        "high" => Some(ThinkingMode::High),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::provider::ProviderProtocol;
    use crate::model::resolved::ResolvedCredential;

    #[test]
    fn auto_compact_limit_uses_explicit_limit_without_context_window() {
        let mut config = AgentConfig {
            auto_compact_token_limit: Some(32_000),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = None;

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(32_000));
    }

    #[test]
    fn auto_compact_limit_derives_ninety_percent_of_context_window() {
        let mut config = AgentConfig {
            auto_compact_token_limit: None,
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(100_000);

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(90_000));
    }

    #[test]
    fn auto_compact_limit_clamps_explicit_limit_to_context_window_headroom() {
        let mut config = AgentConfig {
            auto_compact_token_limit: Some(95_000),
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(100_000);

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(90_000));
    }

    #[test]
    fn auto_compact_limit_is_none_without_limit_or_context_window() {
        let mut config = AgentConfig {
            auto_compact_token_limit: None,
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = None;

        assert_eq!(config.resolved_auto_compact_token_limit(), None);
    }

    #[test]
    fn auto_compact_limit_handles_large_context_window_without_overflow() {
        let mut config = AgentConfig {
            auto_compact_token_limit: None,
            ..AgentConfig::default()
        };
        config.model.capabilities.context_window = Some(i64::MAX);

        assert_eq!(
            config.resolved_auto_compact_token_limit(),
            Some(((i64::MAX as i128 * 9) / 10) as i64)
        );
    }

    #[test]
    fn default_agent_config_has_resolved_openai_model_without_env_secret() {
        let previous = std::env::var("OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "sk-env");

        let config = AgentConfig::default();

        assert_eq!(config.model.identity.provider_id, "openai");
        assert_eq!(config.model.identity.model_id, "gpt-4.1");
        assert_eq!(
            config.model.protocol,
            ProviderProtocol::OpenAiChatCompletions
        );
        assert_eq!(config.model.credential, ResolvedCredential::None);

        match previous {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[test]
    fn token_budget_env_values_accept_positive_integers() {
        assert_eq!(parse_positive_i64("128000"), Some(128_000));
        assert_eq!(parse_positive_i64(" 64000 "), Some(64_000));
    }

    #[test]
    fn token_budget_env_values_ignore_invalid_or_non_positive_values() {
        assert_eq!(parse_positive_i64(""), None);
        assert_eq!(parse_positive_i64("abc"), None);
        assert_eq!(parse_positive_i64("0"), None);
        assert_eq!(parse_positive_i64("-1"), None);
    }

    #[test]
    fn thinking_mode_values_accept_known_modes() {
        assert_eq!(parse_thinking_mode_value("auto"), Some(ThinkingMode::Auto));
        assert_eq!(parse_thinking_mode_value("low"), Some(ThinkingMode::Low));
        assert_eq!(
            parse_thinking_mode_value("medium"),
            Some(ThinkingMode::Medium)
        );
        assert_eq!(parse_thinking_mode_value("high"), Some(ThinkingMode::High));
    }

    #[test]
    fn thinking_mode_values_ignore_invalid_or_empty_values() {
        assert_eq!(parse_thinking_mode_value(""), None);
        assert_eq!(parse_thinking_mode_value("none"), None);
        assert_eq!(parse_thinking_mode_value("maximum"), None);
    }
}
