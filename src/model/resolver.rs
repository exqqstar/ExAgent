use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::model::provider::{provider_profile_by_id, ProviderProfile};
use crate::model::resolved::{ModelRef, ResolvedCredential, ResolvedModelConfig};

#[async_trait]
pub trait ModelResolver: Send + Sync {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig>;
}

pub struct EnvModelResolver;

#[async_trait]
impl ModelResolver for EnvModelResolver {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig> {
        let profile = provider_profile_by_id(&model_ref.provider_id)
            .ok_or_else(|| anyhow!("unknown provider `{}`", model_ref.provider_id))?;
        Ok(resolve_from_profile(
            profile,
            &model_ref.model_id,
            provider_env_base_url(profile.id),
            provider_env_api_key(profile.id),
            model_context_window_from_env(),
        ))
    }
}

pub fn resolve_from_profile(
    profile: &ProviderProfile,
    model_id: &str,
    base_url: Option<String>,
    api_key: Option<String>,
    context_window: Option<i64>,
) -> ResolvedModelConfig {
    ResolvedModelConfig::from_provider_profile(
        profile.id,
        model_id,
        base_url,
        api_key
            .filter(|value| !value.trim().is_empty())
            .map(ResolvedCredential::ApiKey)
            .unwrap_or_default(),
        context_window,
    )
}

pub fn provider_env_api_key(provider_id: &str) -> Option<String> {
    provider_env_var(provider_id, ProviderEnvKind::ApiKey)
        .and_then(|key| std::env::var(key).ok())
        .filter(|value| !value.trim().is_empty())
}

pub fn provider_env_base_url(provider_id: &str) -> Option<String> {
    provider_env_var(provider_id, ProviderEnvKind::BaseUrl)
        .and_then(|key| std::env::var(key).ok())
        .filter(|value| !value.trim().is_empty())
}

pub fn model_context_window_from_env() -> Option<i64> {
    std::env::var("EXAGENT_MODEL_CONTEXT_WINDOW")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
}

enum ProviderEnvKind {
    ApiKey,
    BaseUrl,
}

fn provider_env_var(provider_id: &str, kind: ProviderEnvKind) -> Option<&'static str> {
    match (provider_id, kind) {
        ("openai", ProviderEnvKind::ApiKey) => Some("OPENAI_API_KEY"),
        ("openai", ProviderEnvKind::BaseUrl) => Some("OPENAI_BASE_URL"),
        ("anthropic", ProviderEnvKind::ApiKey) => Some("ANTHROPIC_API_KEY"),
        ("anthropic", ProviderEnvKind::BaseUrl) => Some("ANTHROPIC_BASE_URL"),
        ("google", ProviderEnvKind::ApiKey) => Some("GOOGLE_API_KEY"),
        ("google", ProviderEnvKind::BaseUrl) => Some("GOOGLE_BASE_URL"),
        _ => None,
    }
}
