use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    pub model: String,
    pub thinking_mode: Option<ThinkingMode>,
    pub max_turns: usize,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub policy_mode: PolicyMode,
    pub model_context_window: Option<i64>,
    pub auto_compact_token_limit: Option<i64>,
}

impl AgentConfig {
    pub fn resolved_auto_compact_token_limit(&self) -> Option<i64> {
        let context_limit = self.model_context_window.map(ninety_percent);

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
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
            thinking_mode: parse_optional_thinking_mode_env("EXAGENT_THINKING_MODE"),
            max_turns: 12,
            workspace_root: cwd.clone(),
            cwd,
            command_timeout_secs: 30,
            max_output_bytes: 8 * 1024,
            policy_mode: std::env::var("EXAGENT_POLICY_MODE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            model_context_window: parse_optional_i64_env("EXAGENT_MODEL_CONTEXT_WINDOW"),
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

    #[test]
    fn auto_compact_limit_uses_explicit_limit_without_context_window() {
        let config = AgentConfig {
            auto_compact_token_limit: Some(32_000),
            model_context_window: None,
            ..AgentConfig::default()
        };

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(32_000));
    }

    #[test]
    fn auto_compact_limit_derives_ninety_percent_of_context_window() {
        let config = AgentConfig {
            auto_compact_token_limit: None,
            model_context_window: Some(100_000),
            ..AgentConfig::default()
        };

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(90_000));
    }

    #[test]
    fn auto_compact_limit_clamps_explicit_limit_to_context_window_headroom() {
        let config = AgentConfig {
            auto_compact_token_limit: Some(95_000),
            model_context_window: Some(100_000),
            ..AgentConfig::default()
        };

        assert_eq!(config.resolved_auto_compact_token_limit(), Some(90_000));
    }

    #[test]
    fn auto_compact_limit_is_none_without_limit_or_context_window() {
        let config = AgentConfig {
            auto_compact_token_limit: None,
            model_context_window: None,
            ..AgentConfig::default()
        };

        assert_eq!(config.resolved_auto_compact_token_limit(), None);
    }

    #[test]
    fn auto_compact_limit_handles_large_context_window_without_overflow() {
        let config = AgentConfig {
            auto_compact_token_limit: None,
            model_context_window: Some(i64::MAX),
            ..AgentConfig::default()
        };

        assert_eq!(
            config.resolved_auto_compact_token_limit(),
            Some(((i64::MAX as i128 * 9) / 10) as i64)
        );
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
