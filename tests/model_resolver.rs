use std::sync::{Mutex, OnceLock};

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
        "EXAGENT_MODEL_CONTEXT_WINDOW",
    ] {
        std::env::remove_var(key);
    }
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

#[tokio::test]
async fn env_resolver_ignores_non_positive_context_window() {
    let _guard = env_lock().lock().unwrap();
    clear_provider_env();
    std::env::set_var("EXAGENT_MODEL_CONTEXT_WINDOW", "0");

    let model = EnvModelResolver
        .resolve(&ModelRef::new("google", "gemini-2.5-pro"))
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
