use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use exagent::resolved::ResolvedCredential;
use exagent_desktop::settings::{
    CredentialSource, DesktopSettingsStore, ProviderAuthMode, ProviderProtocol,
    ProviderSettingsSaveRequest, SecretStore,
};
use tempfile::tempdir;

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl SecretStore for MemorySecrets {
    fn get_secret(&self, account: &str) -> anyhow::Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> anyhow::Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> anyhow::Result<()> {
        self.values.lock().unwrap().remove(account);
        Ok(())
    }
}

#[tokio::test]
async fn provider_settings_save_key_in_secret_store_and_report_connected_provider() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1".into(),
            api_key: Some("sk-test".into()),
            clear_api_key: false,
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();

    assert_eq!(settings.active_provider_id, "openai");
    assert_eq!(settings.config.model, "gpt-4.1");
    assert!(settings.config.has_api_key);
    assert_eq!(
        settings.config.credential_source,
        CredentialSource::Keychain
    );
    assert!(settings.connected_provider.is_some());
    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ApiKey("sk-test".to_string())
    );
}

#[tokio::test]
async fn runtime_config_uses_env_values_when_keychain_and_settings_file_are_empty() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    std::env::set_var("OPENAI_MODEL", "env-model");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store.load_provider_settings().await.unwrap();
    let config = store.runtime_config().await.unwrap();

    assert_eq!(
        settings.config.credential_source,
        CredentialSource::Environment
    );
    assert!(settings.config.has_api_key);
    assert!(settings.connected_provider.is_some());
    assert_eq!(
        config.model.credential,
        ResolvedCredential::ApiKey("sk-env".to_string())
    );
    assert_eq!(
        config.model.endpoint.base_url.as_deref(),
        Some("https://env.example/v1")
    );
    assert_eq!(config.model.identity.model_id, "env-model");

    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");
}

#[tokio::test]
async fn runtime_config_prefers_keychain_key_over_env_key() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    std::env::set_var("OPENAI_MODEL", "env-model");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1".into(),
            api_key: Some("sk-keychain".into()),
            clear_api_key: false,
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();
    let config = store.runtime_config().await.unwrap();

    assert_eq!(
        settings.config.credential_source,
        CredentialSource::Keychain
    );
    assert_eq!(
        config.model.credential,
        ResolvedCredential::ApiKey("sk-keychain".to_string())
    );
    assert_eq!(
        config.model.endpoint.base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(config.model.identity.model_id, "gpt-4.1");

    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");
}

#[tokio::test]
async fn openai_compatible_settings_can_be_saved_without_api_key() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai_compatible".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "local-model".into(),
            api_key: None,
            clear_api_key: false,
        })
        .await
        .unwrap();

    let openai_compatible = settings
        .providers
        .iter()
        .find(|provider| provider.id == "openai_compatible")
        .unwrap();

    assert_eq!(
        openai_compatible.auth_mode,
        ProviderAuthMode::ApiKeyOptional
    );
    assert_eq!(settings.active_provider_id, "openai_compatible");
    assert_eq!(settings.config.credential_source, CredentialSource::None);
    assert!(!settings.config.has_api_key);
    assert!(!settings.config.auth_required);
    assert!(settings.connected_provider.is_some());
}

#[tokio::test]
async fn openai_compatible_runtime_does_not_reuse_openai_env_key() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("OPENAI_API_KEY", "sk-env-openai");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai_compatible".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "local-model".into(),
            api_key: None,
            clear_api_key: false,
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();
    let config = store.runtime_config().await.unwrap();

    assert_eq!(settings.config.credential_source, CredentialSource::None);
    assert_eq!(config.model.credential, ResolvedCredential::None);

    std::env::remove_var("OPENAI_API_KEY");
}

#[tokio::test]
async fn provider_catalog_is_backed_by_structured_profiles() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store.load_provider_settings().await.unwrap();
    let openai = settings
        .providers
        .iter()
        .find(|provider| provider.id == "openai")
        .unwrap();
    let anthropic = settings
        .providers
        .iter()
        .find(|provider| provider.id == "anthropic")
        .unwrap();

    assert_eq!(openai.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(openai.default_base_url, "https://api.openai.com/v1");
    assert_eq!(openai.default_model, "gpt-4.1");
    assert!(openai.supports_model_discovery);
    assert!(openai.supports_tools);
    assert_eq!(openai.unsupported_reason, None);

    assert_eq!(anthropic.protocol, ProviderProtocol::AnthropicMessages);
    assert_eq!(anthropic.default_base_url, "https://api.anthropic.com/v1");
    assert!(!anthropic.supported);
    assert_eq!(
        anthropic.unsupported_reason.as_deref(),
        Some("Anthropic Messages adapter is planned.")
    );
}
