use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::mcp::config::McpServerConfig;

use crate::model::resolved::ResolvedModelConfig;
use crate::policy::PolicyMode;
pub use crate::runtime::skills::{
    load_skills, SkillCatalog, SkillConfig, SkillMetadata, SkillScope, SkillWarning,
    SkillWarningKind,
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    #[default]
    FullAccess,
    External,
    Managed,
}

pub fn default_boundary_none() -> String {
    "none".to_string()
}

impl PermissionProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FullAccess => "full_access",
            Self::External => "external",
            Self::Managed => "managed",
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(self, Self::FullAccess)
    }

    pub fn supported_profiles() -> Vec<Self> {
        vec![Self::FullAccess]
    }

    pub fn execution_boundary_summary(&self) -> &'static str {
        match self {
            Self::FullAccess => {
                "filesystem sandbox: none; network sandbox: none; env isolation: none"
            }
            Self::External => "execution boundary: provided outside ExAgent",
            Self::Managed => "execution boundary: managed OS sandbox",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    Auto,
    #[serde(alias = "none")]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(alias = "xhigh", alias = "extra_high", alias = "extra-high")]
    XHigh,
}

impl ThinkingMode {
    pub const ALL: [ThinkingMode; 7] = [
        ThinkingMode::Auto,
        ThinkingMode::Off,
        ThinkingMode::Minimal,
        ThinkingMode::Low,
        ThinkingMode::Medium,
        ThinkingMode::High,
        ThinkingMode::XHigh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "x_high",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebSearchConfig {
    pub provider: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: ResolvedModelConfig,
    pub thinking_mode: Option<ThinkingMode>,
    pub permission_profile: PermissionProfile,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub policy_mode: PolicyMode,
    pub auto_compact_token_limit: Option<i64>,
    pub project_docs_enabled: bool,
    pub project_docs_max_bytes: usize,
    pub skills_enabled: bool,
    pub skills_metadata_max_chars: usize,
    pub skills_user_roots: Vec<PathBuf>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub web_search: Option<WebSearchConfig>,
    pub forge_review_gate_enabled: bool,
    pub memory_enabled: bool,
    pub memory_auto_inject_enabled: bool,
    pub memory_frozen_inject_enabled: bool,
    pub memory_auto_context_max_chars: usize,
    pub memory_frozen_context_max_chars: usize,
    pub memory_tool_context_max_chars: usize,
    pub memory_auto_max_hits: usize,
    pub memory_tool_max_hits: usize,
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
            permission_profile: parse_permission_profile_env("EXAGENT_PERMISSION_PROFILE")
                .unwrap_or_default(),
            workspace_root: cwd.clone(),
            cwd,
            command_timeout_secs: 30,
            max_output_bytes: 8 * 1024,
            policy_mode: std::env::var("EXAGENT_POLICY_MODE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            auto_compact_token_limit: parse_optional_i64_env("EXAGENT_AUTO_COMPACT_TOKEN_LIMIT"),
            project_docs_enabled: parse_optional_bool_env("EXAGENT_PROJECT_DOCS_ENABLED")
                .unwrap_or(true),
            project_docs_max_bytes: parse_optional_usize_env("EXAGENT_PROJECT_DOCS_MAX_BYTES")
                .unwrap_or(64 * 1024),
            skills_enabled: parse_optional_bool_env("EXAGENT_SKILLS_ENABLED").unwrap_or(true),
            skills_metadata_max_chars: parse_optional_usize_env(
                "EXAGENT_SKILLS_METADATA_MAX_CHARS",
            )
            .unwrap_or(8 * 1024),
            skills_user_roots: std::env::var_os("EXAGENT_SKILLS_USER_ROOT")
                .map(PathBuf::from)
                .into_iter()
                .collect(),
            mcp_servers: Vec::new(),
            web_search: web_search_config_from_env(),
            forge_review_gate_enabled: parse_optional_bool_env("EXAGENT_FORGE_REVIEW_GATE_ENABLED")
                .unwrap_or(false),
            memory_enabled: parse_optional_bool_env("EXAGENT_MEMORY_ENABLED").unwrap_or(true),
            memory_auto_inject_enabled: parse_optional_bool_env(
                "EXAGENT_MEMORY_AUTO_INJECT_ENABLED",
            )
            .unwrap_or(true),
            memory_frozen_inject_enabled: parse_optional_bool_env(
                "EXAGENT_MEMORY_FROZEN_INJECT_ENABLED",
            )
            .unwrap_or(true),
            memory_auto_context_max_chars: parse_optional_usize_env(
                "EXAGENT_MEMORY_AUTO_CONTEXT_MAX_CHARS",
            )
            .unwrap_or(2 * 1024),
            memory_frozen_context_max_chars: parse_optional_usize_env(
                "EXAGENT_MEMORY_FROZEN_CONTEXT_MAX_CHARS",
            )
            .unwrap_or(1 * 1024),
            memory_tool_context_max_chars: parse_optional_usize_env(
                "EXAGENT_MEMORY_TOOL_CONTEXT_MAX_CHARS",
            )
            .unwrap_or(12 * 1024),
            memory_auto_max_hits: parse_optional_usize_env("EXAGENT_MEMORY_AUTO_MAX_HITS")
                .unwrap_or(4),
            memory_tool_max_hits: parse_optional_usize_env("EXAGENT_MEMORY_TOOL_MAX_HITS")
                .unwrap_or(20),
        }
    }
}

fn web_search_config_from_env() -> Option<WebSearchConfig> {
    let api_key = std::env::var("EXAGENT_WEB_SEARCH_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let provider = std::env::var("EXAGENT_WEB_SEARCH_PROVIDER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "brave".to_string());
    Some(WebSearchConfig { provider, api_key })
}

fn parse_optional_bool_env(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_bool_value(value.trim()))
}

fn parse_optional_i64_env(key: &str) -> Option<i64> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_positive_i64(value.trim()))
}

fn parse_optional_usize_env(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_positive_usize(value.trim()))
}

fn parse_optional_thinking_mode_env(key: &str) -> Option<ThinkingMode> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_thinking_mode_value(value.trim()))
}

fn parse_permission_profile_env(key: &str) -> Option<PermissionProfile> {
    std::env::var(key)
        .ok()
        .and_then(|value| parse_permission_profile_value(value.trim()))
}

fn parse_positive_i64(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn parse_positive_usize(value: &str) -> Option<usize> {
    let parsed = value.trim().parse::<usize>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn parse_bool_value(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_permission_profile_value(value: &str) -> Option<PermissionProfile> {
    match value.trim().to_ascii_lowercase().as_str() {
        "full_access" => Some(PermissionProfile::FullAccess),
        "external" => Some(PermissionProfile::External),
        "managed" => Some(PermissionProfile::Managed),
        _ => None,
    }
}

fn parse_thinking_mode_value(value: &str) -> Option<ThinkingMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(ThinkingMode::Auto),
        "off" | "none" => Some(ThinkingMode::Off),
        "minimal" => Some(ThinkingMode::Minimal),
        "low" => Some(ThinkingMode::Low),
        "medium" => Some(ThinkingMode::Medium),
        "high" => Some(ThinkingMode::High),
        "xhigh" | "extra_high" | "extra-high" => Some(ThinkingMode::XHigh),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::provider::ProviderProtocol;
    use crate::model::resolved::ResolvedCredential;

    static WEB_SEARCH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn thinking_mode_labels_round_trip_through_serde() {
        for mode in ThinkingMode::ALL {
            let from_label: ThinkingMode =
                serde_json::from_value(serde_json::Value::String(mode.label().to_string()))
                    .expect("label must deserialize back to its variant");
            assert_eq!(from_label, mode);
        }
    }

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
        assert_eq!(config.model.identity.model_id, "gpt-5.5");
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
    fn thinking_mode_values_accept_off_minimal_and_xhigh() {
        assert_eq!(parse_thinking_mode_value("off"), Some(ThinkingMode::Off));
        assert_eq!(
            parse_thinking_mode_value("minimal"),
            Some(ThinkingMode::Minimal)
        );
        assert_eq!(
            parse_thinking_mode_value("xhigh"),
            Some(ThinkingMode::XHigh)
        );
    }

    #[test]
    fn thinking_mode_values_accept_none_as_off() {
        assert_eq!(parse_thinking_mode_value("none"), Some(ThinkingMode::Off));
    }

    #[test]
    fn thinking_mode_deserializes_env_aliases_from_json() {
        assert_eq!(
            serde_json::from_str::<ThinkingMode>("\"none\"").unwrap(),
            ThinkingMode::Off
        );
        assert_eq!(
            serde_json::from_str::<ThinkingMode>("\"xhigh\"").unwrap(),
            ThinkingMode::XHigh
        );
        assert_eq!(
            serde_json::from_str::<ThinkingMode>("\"extra_high\"").unwrap(),
            ThinkingMode::XHigh
        );
        assert_eq!(
            serde_json::from_str::<ThinkingMode>("\"extra-high\"").unwrap(),
            ThinkingMode::XHigh
        );
    }

    #[test]
    fn thinking_mode_values_ignore_invalid_or_empty_values() {
        assert_eq!(parse_thinking_mode_value(""), None);
        assert_eq!(parse_thinking_mode_value("maximum"), None);
    }

    #[test]
    fn permission_profile_defaults_to_full_access() {
        assert_eq!(PermissionProfile::default(), PermissionProfile::FullAccess);
    }

    #[test]
    fn permission_profile_values_parse_known_modes() {
        assert_eq!(
            parse_permission_profile_value("full_access"),
            Some(PermissionProfile::FullAccess)
        );
        assert_eq!(
            parse_permission_profile_value("external"),
            Some(PermissionProfile::External)
        );
        assert_eq!(
            parse_permission_profile_value("managed"),
            Some(PermissionProfile::Managed)
        );
    }

    #[test]
    fn permission_profile_values_ignore_invalid_modes() {
        assert_eq!(parse_permission_profile_value(""), None);
        assert_eq!(parse_permission_profile_value("disabled"), None);
        assert_eq!(parse_permission_profile_value("sandbox"), None);
    }

    #[test]
    fn web_search_config_from_env_requires_api_key_and_defaults_to_brave() {
        let _guard = WEB_SEARCH_ENV_LOCK.lock().unwrap();
        let previous_key = std::env::var("EXAGENT_WEB_SEARCH_API_KEY").ok();
        let previous_provider = std::env::var("EXAGENT_WEB_SEARCH_PROVIDER").ok();
        std::env::set_var("EXAGENT_WEB_SEARCH_API_KEY", " search-key ");
        std::env::remove_var("EXAGENT_WEB_SEARCH_PROVIDER");

        let config = web_search_config_from_env().expect("web search config");

        assert_eq!(config.provider, "brave");
        assert_eq!(config.api_key, "search-key");

        restore_env("EXAGENT_WEB_SEARCH_API_KEY", previous_key);
        restore_env("EXAGENT_WEB_SEARCH_PROVIDER", previous_provider);
    }

    #[test]
    fn web_search_config_from_env_normalizes_provider_and_ignores_empty_key() {
        let _guard = WEB_SEARCH_ENV_LOCK.lock().unwrap();
        let previous_key = std::env::var("EXAGENT_WEB_SEARCH_API_KEY").ok();
        let previous_provider = std::env::var("EXAGENT_WEB_SEARCH_PROVIDER").ok();
        std::env::set_var("EXAGENT_WEB_SEARCH_API_KEY", " ");
        std::env::set_var("EXAGENT_WEB_SEARCH_PROVIDER", " BRAVE ");

        assert!(web_search_config_from_env().is_none());

        std::env::set_var("EXAGENT_WEB_SEARCH_API_KEY", "key");
        let config = web_search_config_from_env().expect("web search config");
        assert_eq!(config.provider, "brave");

        restore_env("EXAGENT_WEB_SEARCH_API_KEY", previous_key);
        restore_env("EXAGENT_WEB_SEARCH_PROVIDER", previous_provider);
    }

    #[test]
    fn only_full_access_is_supported_for_now() {
        assert!(PermissionProfile::FullAccess.is_supported());
        assert!(!PermissionProfile::External.is_supported());
        assert!(!PermissionProfile::Managed.is_supported());
        assert_eq!(
            PermissionProfile::supported_profiles(),
            vec![PermissionProfile::FullAccess]
        );
    }

    fn restore_env(key: &str, value: Option<String>) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
