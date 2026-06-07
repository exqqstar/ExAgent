use std::sync::{Mutex, OnceLock};

use exagent::config::ThinkingMode;
use exagent::model::reasoning::ReasoningProtocol;
use exagent::provider::ProviderProtocol;
use exagent::resolved::{ModelRef, ResolvedCredential};
use exagent::resolver::{EnvModelResolver, ModelResolver};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_provider_env() {
    for key in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "GOOGLE_API_KEY",
        "GOOGLE_BASE_URL",
        "GEMINI_API_KEY",
        "GEMINI_BASE_URL",
        "DEEPSEEK_API_KEY",
        "DEEPSEEK_BASE_URL",
        "KIMI_API_KEY",
        "KIMI_BASE_URL",
        "MOONSHOT_API_KEY",
        "MOONSHOT_BASE_URL",
        "GLM_API_KEY",
        "GLM_BASE_URL",
        "ZHIPU_API_KEY",
        "ZHIPU_BASE_URL",
        "BIGMODEL_API_KEY",
        "BIGMODEL_BASE_URL",
        "EXAGENT_MODEL_CONTEXT_WINDOW",
    ] {
        std::env::remove_var(key);
    }
}

#[tokio::test]
async fn env_resolver_materializes_openai_compatible_vendor_profiles() {
    let _guard = env_lock().lock().unwrap();
    clear_provider_env();
    std::env::set_var("DEEPSEEK_API_KEY", "deepseek-key");
    std::env::set_var("MOONSHOT_API_KEY", "kimi-key");
    std::env::set_var("ZHIPU_API_KEY", "glm-key");

    let deepseek = EnvModelResolver
        .resolve(&ModelRef::new("deepseek", "deepseek-v4-flash"))
        .await
        .unwrap();
    let kimi = EnvModelResolver
        .resolve(&ModelRef::new("kimi", "kimi-k2.6"))
        .await
        .unwrap();
    let glm = EnvModelResolver
        .resolve(&ModelRef::new("glm", "glm-4.5"))
        .await
        .unwrap();

    assert_eq!(deepseek.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(
        deepseek.endpoint.base_url.as_deref(),
        Some("https://api.deepseek.com")
    );
    assert_eq!(
        deepseek.credential,
        ResolvedCredential::ApiKey("deepseek-key".to_string())
    );
    assert_eq!(
        kimi.endpoint.base_url.as_deref(),
        Some("https://api.moonshot.ai/v1")
    );
    assert_eq!(
        kimi.credential,
        ResolvedCredential::ApiKey("kimi-key".to_string())
    );
    assert_eq!(
        glm.endpoint.base_url.as_deref(),
        Some("https://open.bigmodel.cn/api/paas/v4")
    );
    assert_eq!(
        glm.credential,
        ResolvedCredential::ApiKey("glm-key".to_string())
    );

    clear_provider_env();
}

#[tokio::test]
async fn env_resolver_materializes_openai_profile_from_env() {
    let _guard = env_lock().lock().unwrap();
    clear_provider_env();
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    std::env::set_var("EXAGENT_MODEL_CONTEXT_WINDOW", "128000");

    let model = EnvModelResolver
        .resolve(&ModelRef::new("openai", "gpt-4.1-mini"))
        .await
        .unwrap();

    assert_eq!(model.identity.provider_id, "openai");
    assert_eq!(model.identity.model_id, "gpt-4.1-mini");
    assert_eq!(model.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(
        model.endpoint.base_url.as_deref(),
        Some("https://env.example/v1")
    );
    assert_eq!(
        model.credential,
        ResolvedCredential::ApiKey("sk-env".to_string())
    );
    assert_eq!(model.capabilities.context_window, Some(128_000));
    assert!(model.capabilities.supports_tools);
    assert_eq!(
        model.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    clear_provider_env();
}

#[tokio::test]
async fn env_resolver_uses_provider_defaults_and_allows_missing_key() {
    let _guard = env_lock().lock().unwrap();
    clear_provider_env();

    let model = EnvModelResolver
        .resolve(&ModelRef::new("openai_compatible", "local-model"))
        .await
        .unwrap();

    assert_eq!(model.identity.provider_id, "openai_compatible");
    assert_eq!(model.identity.model_id, "local-model");
    assert_eq!(
        model.endpoint.base_url.as_deref(),
        Some("http://127.0.0.1:11434/v1")
    );
    assert_eq!(model.credential, ResolvedCredential::None);
    assert_eq!(model.capabilities.context_window, None);
}

#[test]
fn deepseek_resolves_deepseek_reasoning_protocol() {
    let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "deepseek",
        "deepseek-v4-flash",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );

    assert_eq!(
        model.capabilities.reasoning.protocol,
        ReasoningProtocol::DeepSeekThinking
    );
    assert!(model.capabilities.reasoning.supports(ThinkingMode::Off));
    assert!(model.capabilities.reasoning.supports(ThinkingMode::High));
    assert_eq!(
        model.capabilities.reasoning.default_mode,
        Some(ThinkingMode::Off)
    );
    assert!(
        model
            .capabilities
            .reasoning
            .requires_assistant_reasoning_content
    );
}

#[test]
fn kimi_resolves_thinking_object_default_off_and_glm_keeps_zai_default_off() {
    let kimi = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "kimi",
        "kimi-k2.6",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );
    let glm = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "glm",
        "glm-5.1",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );

    assert_eq!(
        serde_json::to_value(kimi.capabilities.reasoning.protocol).unwrap(),
        serde_json::json!("thinking_object")
    );
    assert_eq!(
        kimi.capabilities.reasoning.default_mode,
        Some(ThinkingMode::Off)
    );
    assert_eq!(
        glm.capabilities.reasoning.protocol,
        ReasoningProtocol::ZaiThinkingObject
    );
    assert_eq!(
        glm.capabilities.reasoning.default_mode,
        Some(ThinkingMode::Off)
    );
}

#[test]
fn openai_resolves_openai_reasoning_protocol() {
    let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "openai",
        "gpt-5.5",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );

    assert_eq!(
        model.capabilities.reasoning.protocol,
        ReasoningProtocol::OpenAiReasoningEffort
    );
    assert!(model.capabilities.reasoning.supports(ThinkingMode::Off));
    assert!(model.capabilities.reasoning.supports(ThinkingMode::XHigh));
    assert_eq!(
        model.capabilities.reasoning.default_mode,
        Some(ThinkingMode::Medium)
    );
}

#[test]
fn older_openai_reasoning_models_do_not_support_off() {
    for model_id in ["gpt-5", "gpt-5-mini", "gpt-5-nano", "o3", "o4-mini"] {
        let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
            "openai",
            model_id,
            None,
            ResolvedCredential::ApiKey("secret".to_string()),
            None,
        );

        assert_eq!(
            model.capabilities.reasoning.protocol,
            ReasoningProtocol::OpenAiReasoningEffort,
            "{model_id} should still expose OpenAI reasoning"
        );
        assert!(
            !model.capabilities.reasoning.supports(ThinkingMode::Off),
            "{model_id} should not expose reasoning_effort none"
        );
        assert!(model.capabilities.reasoning.supports(ThinkingMode::High));
    }
}

#[test]
fn current_openai_reasoning_models_support_off() {
    for model_id in ["gpt-5.5", "gpt-5.4", "gpt-5.1"] {
        let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
            "openai",
            model_id,
            None,
            ResolvedCredential::ApiKey("secret".to_string()),
            None,
        );

        assert!(
            model.capabilities.reasoning.supports(ThinkingMode::Off),
            "{model_id} should expose reasoning_effort none"
        );
    }
}

#[test]
fn unknown_openai_reasoning_model_does_not_support_off() {
    let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "openai",
        "gpt-future-reasoner",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );

    assert_eq!(
        model.capabilities.reasoning.protocol,
        ReasoningProtocol::OpenAiReasoningEffort
    );
    assert!(
        !model.capabilities.reasoning.supports(ThinkingMode::Off),
        "unknown OpenAI models should not expose reasoning_effort none"
    );
    assert!(model.capabilities.reasoning.supports(ThinkingMode::High));
    assert!(
        !model.capabilities.reasoning.supports(ThinkingMode::XHigh),
        "unknown OpenAI models should not infer xhigh support"
    );
}

#[test]
fn openai_gpt_4_1_models_resolve_without_reasoning_protocol() {
    for model_id in [
        "gpt-4.1",
        "gpt-4.1-mini",
        "gpt-4.1-nano",
        "gpt-4.1-2025-04-14",
        "gpt-4.1-mini-2025-04-14",
        "gpt-4.1-nano-2025-04-14",
    ] {
        let model = exagent::resolved::ResolvedModelConfig::from_provider_profile(
            "openai",
            model_id,
            None,
            ResolvedCredential::ApiKey("secret".to_string()),
            None,
        );

        assert_eq!(
            model.capabilities.reasoning.protocol,
            ReasoningProtocol::None,
            "{model_id} should not expose a reasoning protocol"
        );
    }
}

#[test]
fn google_and_anthropic_resolves_provider_reasoning_defaults() {
    let google = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "google",
        "gemini-3-pro-preview",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );
    let anthropic = exagent::resolved::ResolvedModelConfig::from_provider_profile(
        "anthropic",
        "claude-sonnet-4-6",
        None,
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );

    assert_eq!(
        google.capabilities.reasoning.protocol,
        ReasoningProtocol::GeminiThinkingConfig
    );
    assert_eq!(
        google.capabilities.reasoning.default_mode,
        Some(ThinkingMode::High)
    );
    assert_eq!(
        anthropic.capabilities.reasoning.protocol,
        ReasoningProtocol::AnthropicThinkingBudget
    );
    assert_eq!(
        anthropic.capabilities.reasoning.default_mode,
        Some(ThinkingMode::Medium)
    );
}

#[tokio::test]
async fn env_resolver_ignores_non_positive_context_window() {
    let _guard = env_lock().lock().unwrap();
    clear_provider_env();
    std::env::set_var("EXAGENT_MODEL_CONTEXT_WINDOW", "0");

    let model = EnvModelResolver
        .resolve(&ModelRef::new("google", "gemini-3-flash-preview"))
        .await
        .unwrap();

    assert_eq!(model.protocol, ProviderProtocol::GeminiGenerateContent);
    assert_eq!(model.capabilities.context_window, None);

    clear_provider_env();
}

#[tokio::test]
async fn env_resolver_errors_for_unknown_provider() {
    let error = EnvModelResolver
        .resolve(&ModelRef::new("missing", "model"))
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("unknown provider"),
        "error was: {error}"
    );
}
