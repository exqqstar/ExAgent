use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use exagent::config::ThinkingMode;
use exagent::model::chatgpt_codex::{ChatGptCodexTokenRefreshSink, ChatGptCodexTokenUpdate};
use exagent::model::reasoning::ReasoningProtocol;
use exagent::resolved::ResolvedCredential;
use exagent_desktop::settings::{
    CredentialAuthMethod, CredentialKind, CredentialSource, CredentialStatus, DesktopSettingsStore,
    ModelCapabilities, OAuthTokenBundle, ProviderAuthMode, ProviderCredentialView,
    ProviderModelView, ProviderProtocol, ProviderSettingsResponse, ProviderSettingsSaveRequest,
    SecretStore,
};
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn write_models_dev_cache(
    dir: &tempfile::TempDir,
    provider_id: &str,
    model_id: &str,
    metadata: serde_json::Value,
) {
    let fetched_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut models = serde_json::Map::new();
    models.insert(model_id.to_string(), metadata);
    let mut provider = serde_json::Map::new();
    provider.insert("models".to_string(), serde_json::Value::Object(models));
    let mut catalog = serde_json::Map::new();
    catalog.insert(provider_id.to_string(), serde_json::Value::Object(provider));
    let cache = json!({
        "fetched_at_ms": fetched_at_ms,
        "catalog": catalog
    });

    tokio::fs::write(
        dir.path().join("models-dev-cache.json"),
        serde_json::to_string(&cache).unwrap(),
    )
    .await
    .unwrap();
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
async fn provider_settings_include_static_catalog_model_options_with_capabilities() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::remove_var("OPENAI_MODEL");
    std::env::remove_var("OPENAI_BASE_URL");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let loaded = store.load_provider_settings().await.expect("settings load");
    let openai_models = loaded
        .model_options
        .iter()
        .filter(|model| model.provider_id == "openai")
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>();
    assert!(
        openai_models.len() > 1,
        "expected static OpenAI catalog options, got {openai_models:?}"
    );
    assert!(openai_models.contains(&"gpt-5.4"));
    assert!(openai_models.contains(&"gpt-4.1"));
    assert!(openai_models.contains(&"gpt-4.1-nano"));
    assert!(openai_models.contains(&"o3"));
    let gpt_5 = loaded
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai" && model.id == "gpt-5.5")
        .expect("configured openai model option");

    assert!(gpt_5.capabilities.supports_tools);
    assert!(gpt_5.capabilities.thinking.supported);
    assert_eq!(gpt_5.context_window, Some(1_047_576));
    assert_eq!(
        gpt_5.capabilities.thinking.modes,
        vec![
            ThinkingMode::Off,
            ThinkingMode::Low,
            ThinkingMode::Medium,
            ThinkingMode::High,
            ThinkingMode::XHigh
        ]
    );

    std::env::remove_var("OPENAI_API_KEY");
}

#[tokio::test]
async fn provider_settings_filter_openai_api_models_when_using_chatgpt_oauth() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(unix_timestamp_millis() + 3_600_000),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    let openai_models = settings
        .model_options
        .iter()
        .filter(|model| model.provider_id == "openai")
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>();

    assert!(openai_models.contains(&"gpt-5.5"));
    assert!(openai_models.contains(&"gpt-5.4"));
    assert!(openai_models.contains(&"gpt-5.4-mini"));
    assert!(
        !openai_models.contains(&"gpt-4.1-nano"),
        "ChatGPT Codex OAuth must not expose OpenAI API-only models: {openai_models:?}"
    );
    assert!(!openai_models.contains(&"gpt-4.1"));
    assert!(!openai_models.contains(&"o3"));

    std::env::remove_var("OPENAI_API_KEY");
}

#[tokio::test]
async fn provider_settings_falls_back_from_unsupported_chatgpt_oauth_model() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let settings_path = dir.path().join("settings.json");
    let store = DesktopSettingsStore::with_secret_store(settings_path, secrets.clone());

    store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(unix_timestamp_millis() + 3_600_000),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();
    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1-nano".into(),
            api_key: None,
            clear_api_key: false,
            credential_id: Some("chatgpt-1".into()),
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();

    assert_eq!(settings.config.model, "gpt-5.5");
    assert!(settings
        .model_options
        .iter()
        .any(|model| model.provider_id == "openai" && model.id == "gpt-5.5"));
    assert!(!settings
        .model_options
        .iter()
        .any(|model| model.provider_id == "openai" && model.id == "gpt-4.1-nano"));
    assert_eq!(
        store
            .runtime_config()
            .await
            .unwrap()
            .model
            .identity
            .model_id,
        "gpt-5.5"
    );

    std::env::remove_var("OPENAI_API_KEY");
}

#[tokio::test]
async fn provider_settings_keep_legacy_active_config_when_saving_oauth_provider() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("DEEPSEEK_API_KEY");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    secrets
        .set_secret("provider:deepseek:api_key", "sk-deepseek")
        .unwrap();
    let settings_path = dir.path().join("settings.json");
    tokio::fs::write(
        &settings_path,
        serde_json::to_string(&json!({
            "provider_id": "deepseek",
            "base_url": "https://api.deepseek.com",
            "model": "deepseek-v4-flash",
            "provider_configs": {},
            "provider_credentials": {}
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let store = DesktopSettingsStore::with_secret_store(settings_path, secrets);

    let settings = store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(unix_timestamp_millis() + 3_600_000),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    let deepseek = settings
        .configured_providers
        .iter()
        .find(|provider| provider.provider_id == "deepseek")
        .expect("deepseek should remain configured after switching to OpenAI OAuth");
    assert_eq!(deepseek.model, "deepseek-v4-flash");
    assert!(deepseek.has_api_key);
    assert!(settings
        .model_options
        .iter()
        .any(|model| model.provider_id == "deepseek" && model.id == "deepseek-v4-flash"));
}

#[tokio::test]
async fn provider_settings_surface_credential_only_provider_with_default_config() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("DEEPSEEK_API_KEY");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    secrets
        .set_secret("provider:deepseek:api_key", "sk-deepseek")
        .unwrap();
    let settings_path = dir.path().join("settings.json");
    tokio::fs::write(
        &settings_path,
        serde_json::to_string(&json!({
            "provider_id": "openai",
            "base_url": "https://api.openai.com/v1",
            "model": "gpt-5.5",
            "provider_configs": {
                "openai": {
                    "base_url": "https://api.openai.com/v1",
                    "model": "gpt-5.5",
                    "model_options": []
                }
            },
            "provider_credentials": {
                "deepseek": {
                    "active_credential_id": "key-1",
                    "credentials": [{
                        "id": "key-1",
                        "label": "API key 1",
                        "kind": "api_key"
                    }]
                }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let store = DesktopSettingsStore::with_secret_store(settings_path, secrets);

    let settings = store.load_provider_settings().await.unwrap();

    let deepseek = settings
        .configured_providers
        .iter()
        .find(|provider| provider.provider_id == "deepseek")
        .expect("deepseek credential should be visible even if its config was lost");
    assert_eq!(deepseek.base_url, "https://api.deepseek.com");
    assert_eq!(deepseek.model, "deepseek-v4-flash");
    assert!(deepseek.has_api_key);
    assert!(settings
        .model_options
        .iter()
        .any(|model| model.provider_id == "deepseek" && model.id == "deepseek-v4-flash"));
}

#[test]
fn legacy_provider_settings_payload_defaults_missing_model_options() {
    let payload = json!({
        "providers": [],
        "active_provider_id": "openai",
        "config": {
            "provider_id": "openai",
            "base_url": "https://api.openai.com/v1",
            "model": "gpt-4.1",
            "has_api_key": false,
            "credential_source": "none",
            "auth_required": true
        },
        "connected_provider": null,
        "last_connection": null
    });

    let response: ProviderSettingsResponse = serde_json::from_value(payload).unwrap();

    assert!(response.model_options.is_empty());
}

#[test]
fn legacy_provider_model_view_defaults_missing_provider_id_and_capabilities() {
    let payload = json!({
        "id": "legacy-model",
        "display_name": "legacy-model",
        "context_window": null,
        "supports_tools": null
    });

    let model: ProviderModelView = serde_json::from_value(payload).unwrap();

    assert_eq!(model.provider_id, "");
    assert_eq!(model.capabilities, ModelCapabilities::default());
    assert!(!model.capabilities.supports_tools);
    assert!(!model.capabilities.thinking.supported);
    assert!(model.capabilities.thinking.modes.is_empty());
}

#[test]
fn legacy_provider_credential_defaults_to_active_api_key() {
    let payload = json!({
        "id": "key-1",
        "label": "API key 1",
        "source": "keychain"
    });

    let credential: ProviderCredentialView = serde_json::from_value(payload).unwrap();

    assert_eq!(credential.kind, CredentialKind::ApiKey);
    assert_eq!(credential.status, CredentialStatus::Active);
    assert!(credential.auth_method.is_none());
    assert!(credential.account_label.is_none());
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
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
    assert_eq!(settings.active_credential_id.as_deref(), Some("key-1"));
    assert_eq!(settings.credentials.len(), 1);
    assert_eq!(settings.credentials[0].id, "key-1");
    assert_eq!(settings.credentials[0].label, "API key 1");
    assert_eq!(settings.credentials[0].source, CredentialSource::Keychain);
    assert_eq!(settings.credentials[0].kind, CredentialKind::ApiKey);
    assert_eq!(settings.credentials[0].status, CredentialStatus::Active);
    assert!(settings.credentials[0].auth_method.is_none());
    assert!(settings.credentials[0].account_label.is_none());
    assert!(settings.connected_provider.is_some());
    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ApiKey("sk-test".to_string())
    );
}

#[tokio::test]
async fn provider_settings_saves_chatgpt_oauth_credential_without_api_key() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store =
        DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets.clone());
    let expires_at_ms = unix_timestamp_millis() + 3_600_000;

    store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(expires_at_ms),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();

    assert_eq!(settings.active_provider_id, "openai");
    assert_eq!(settings.active_credential_id.as_deref(), Some("chatgpt-1"));
    assert!(!settings.config.has_api_key);
    assert!(settings.config.has_credential);
    assert_eq!(settings.config.credential_kind, Some(CredentialKind::OAuth));
    assert!(settings.connected_provider.is_some());
    assert_eq!(settings.credentials.len(), 1);
    assert_eq!(settings.credentials[0].id, "chatgpt-1");
    assert_eq!(settings.credentials[0].label, "ChatGPT Pro");
    assert_eq!(settings.credentials[0].kind, CredentialKind::OAuth);
    assert_eq!(settings.credentials[0].status, CredentialStatus::Active);
    assert_eq!(
        settings.credentials[0].auth_method,
        Some(CredentialAuthMethod::ChatGptOAuth)
    );
    assert_eq!(
        settings.credentials[0].account_label.as_deref(),
        Some("user@example.com")
    );
    assert!(secrets
        .get_secret("provider:openai:credential:chatgpt-1:oauth")
        .unwrap()
        .is_some());
    assert!(secrets
        .get_secret("provider:openai:credential:chatgpt-1:api_key")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clearing_saved_provider_credential_removes_oauth_secret() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store =
        DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets.clone());

    store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(unix_timestamp_millis() + 3_600_000),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5.5".to_string(),
            api_key: None,
            clear_api_key: true,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();
    assert!(!settings.config.has_credential);
    assert!(settings.credentials.is_empty());
    assert!(secrets
        .get_secret("provider:openai:credential:chatgpt-1:oauth")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn runtime_config_uses_chatgpt_oauth_credential_for_openai_provider() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);
    let expires_at_ms = unix_timestamp_millis() + 3_600_000;

    store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(expires_at_ms),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ChatGptOAuth {
            access_token: "access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at_ms: Some(expires_at_ms),
            account_id: Some("acct_123".to_string()),
            credential_id: Some("chatgpt-1".to_string())
        }
    );
}

#[tokio::test]
async fn refreshed_chatgpt_oauth_tokens_are_persisted_to_keychain() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store =
        DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets.clone());
    let original_expires_at_ms = unix_timestamp_millis() + 3_600_000;
    let refreshed_expires_at_ms = unix_timestamp_millis() + 7_200_000;

    store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: Some(original_expires_at_ms),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-token".to_string()),
            },
        )
        .await
        .unwrap();

    store
        .save_chatgpt_codex_tokens(ChatGptCodexTokenUpdate {
            access_token: "new-access-token".to_string(),
            refresh_token: "new-refresh-token".to_string(),
            expires_at_ms: Some(refreshed_expires_at_ms),
            account_id: Some("acct_123".to_string()),
            credential_id: Some("chatgpt-1".to_string()),
        })
        .await
        .unwrap();

    let persisted: OAuthTokenBundle = serde_json::from_str(
        &secrets
            .get_secret("provider:openai:credential:chatgpt-1:oauth")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted.access_token, "new-access-token");
    assert_eq!(persisted.refresh_token, "new-refresh-token");
    assert_eq!(persisted.expires_at_ms, Some(refreshed_expires_at_ms));
    assert_eq!(persisted.account_id.as_deref(), Some("acct_123"));

    let config = store.runtime_config().await.unwrap();
    assert_eq!(
        config.model.credential,
        ResolvedCredential::ChatGptOAuth {
            access_token: "new-access-token".to_string(),
            refresh_token: "new-refresh-token".to_string(),
            expires_at_ms: Some(refreshed_expires_at_ms),
            account_id: Some("acct_123".to_string()),
            credential_id: Some("chatgpt-1".to_string())
        }
    );
}

#[tokio::test]
async fn runtime_config_uses_github_copilot_oauth_credential_as_bearer_token() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_oauth_credential(
            "github_copilot",
            "copilot-1",
            "GitHub Copilot",
            CredentialAuthMethod::GitHubCopilotOAuth,
            OAuthTokenBundle {
                access_token: "copilot-token".to_string(),
                refresh_token: "copilot-token".to_string(),
                expires_at_ms: None,
                account_id: None,
                account_label: None,
                raw_id_token: None,
            },
        )
        .await
        .unwrap();

    let config = store.runtime_config().await.unwrap();

    assert_eq!(config.model.identity.provider_id, "github_copilot");
    assert_eq!(config.model.identity.model_id, "gpt-5.1-copilot");
    assert_eq!(
        config.model.credential,
        ResolvedCredential::BearerToken("copilot-token".to_string())
    );
}

#[tokio::test]
async fn provider_settings_treat_legacy_keychain_key_as_first_credential() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    secrets
        .set_secret("provider:openai:api_key", "sk-legacy")
        .unwrap();
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store.load_provider_settings().await.unwrap();

    assert_eq!(settings.active_credential_id.as_deref(), Some("key-1"));
    assert_eq!(settings.credentials.len(), 1);
    assert_eq!(settings.credentials[0].id, "key-1");
    assert_eq!(settings.credentials[0].label, "API key 1");
    assert_eq!(settings.credentials[0].source, CredentialSource::Keychain);
    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ApiKey("sk-legacy".to_string())
    );
}

#[tokio::test]
async fn provider_settings_exposes_env_key_as_first_credential() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("DEEPSEEK_API_KEY", "sk-env");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
            api_key: None,
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    let settings = store.load_provider_settings().await.unwrap();

    assert_eq!(settings.active_credential_id.as_deref(), Some("key-1"));
    assert_eq!(settings.credentials.len(), 1);
    assert_eq!(settings.credentials[0].id, "key-1");
    assert_eq!(settings.credentials[0].label, "API key 1");
    assert_eq!(
        settings.credentials[0].source,
        CredentialSource::Environment
    );
    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ApiKey("sk-env".to_string())
    );

    std::env::remove_var("DEEPSEEK_API_KEY");
}

#[tokio::test]
async fn provider_settings_replaces_the_single_saved_api_key() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let first = store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
            api_key: Some("sk-first".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(first.active_credential_id.as_deref(), Some("key-1"));

    let second = store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
            api_key: Some("sk-second".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: true,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(second.active_credential_id.as_deref(), Some("key-1"));
    assert_eq!(
        second
            .credentials
            .iter()
            .map(|credential| credential.label.as_str())
            .collect::<Vec<_>>(),
        vec!["API key 1"]
    );
    assert_eq!(
        store.runtime_config().await.unwrap().model.credential,
        ResolvedCredential::ApiKey("sk-second".to_string())
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
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
async fn runtime_config_uses_catalog_reasoning_semantics() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");
    std::env::remove_var("EXAGENT_MODELS_DEV_API_URL");
    std::env::remove_var("EXAGENT_MODELS_DEV_DISABLED");

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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let non_reasoning = store.runtime_config().await.unwrap().model;
    assert_eq!(non_reasoning.identity.model_id, "gpt-4.1");
    assert_eq!(
        non_reasoning.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5.5".into(),
            api_key: Some("sk-test".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let reasoning = store.runtime_config().await.unwrap().model;
    assert_eq!(reasoning.identity.model_id, "gpt-5.5");
    assert_eq!(
        reasoning.capabilities.reasoning.protocol,
        ReasoningProtocol::OpenAiReasoningEffort
    );

    write_models_dev_cache(&dir, "openai", "gpt-5.5", json!({ "reasoning": false })).await;

    let settings = store.load_provider_settings().await.unwrap();
    let metadata_disabled_option = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai" && model.id == "gpt-5.5")
        .expect("saved model option");
    assert!(!metadata_disabled_option.capabilities.thinking.supported);
    assert!(metadata_disabled_option.capabilities.reasoning.is_none());
    let serialized_option = serde_json::to_value(metadata_disabled_option).unwrap();
    assert!(serialized_option["capabilities"].get("reasoning").is_none());

    let metadata_disabled_reasoning = store.runtime_config().await.unwrap().model;
    assert_eq!(metadata_disabled_reasoning.identity.model_id, "gpt-5.5");
    assert_eq!(
        metadata_disabled_reasoning.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    write_models_dev_cache(&dir, "openai", "gpt-4.1", json!({ "reasoning": true })).await;

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1".into(),
            api_key: Some("sk-test".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let settings = store.load_provider_settings().await.unwrap();
    let metadata_enabled_option = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai" && model.id == "gpt-4.1")
        .expect("saved model option");
    assert!(!metadata_enabled_option.capabilities.thinking.supported);
    assert!(metadata_enabled_option.capabilities.reasoning.is_none());

    let metadata_enabled_reasoning = store.runtime_config().await.unwrap().model;
    assert_eq!(metadata_enabled_reasoning.identity.model_id, "gpt-4.1");
    assert_eq!(
        metadata_enabled_reasoning.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    std::env::remove_var("EXAGENT_MODELS_DEV_API_URL");
    std::env::remove_var("EXAGENT_MODELS_DEV_DISABLED");
}

#[tokio::test]
async fn models_dev_context_overrides_static_catalog_for_saved_model_and_runtime() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");
    std::env::remove_var("EXAGENT_MODEL_CONTEXT_WINDOW");
    std::env::remove_var("EXAGENT_MODELS_DEV_API_URL");
    std::env::remove_var("EXAGENT_MODELS_DEV_DISABLED");

    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);
    let metadata_context_window = 765_432;

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5.5".into(),
            api_key: Some("sk-test".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    write_models_dev_cache(
        &dir,
        "openai",
        "gpt-5.5",
        json!({ "limit": { "context": metadata_context_window } }),
    )
    .await;

    let settings = store.load_provider_settings().await.unwrap();
    let saved_option = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai" && model.id == "gpt-5.5")
        .expect("saved catalog model option");
    assert_eq!(
        saved_option.context_window,
        Some(metadata_context_window),
        "saved/current model option should use models.dev context"
    );

    let runtime_model = store.runtime_config().await.unwrap().model;
    assert_eq!(
        runtime_model.capabilities.context_window,
        Some(metadata_context_window),
        "runtime should resolve the same models.dev context"
    );

    std::env::remove_var("EXAGENT_MODEL_CONTEXT_WINDOW");
    std::env::remove_var("EXAGENT_MODELS_DEV_API_URL");
    std::env::remove_var("EXAGENT_MODELS_DEV_DISABLED");
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
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
async fn openai_compatible_uncatalogued_model_options_are_conservative() {
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    let local_model = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai_compatible" && model.id == "local-model")
        .expect("openai-compatible default model option");

    assert_ne!(local_model.supports_tools, Some(true));
    assert!(!local_model.capabilities.supports_tools);
    assert!(!local_model.capabilities.thinking.supported);
    let local_runtime_model = store.runtime_config().await.unwrap().model;
    assert!(!local_runtime_model.capabilities.supports_tools);
    assert_eq!(
        local_runtime_model.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    let settings = store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai_compatible".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "custom-coder".into(),
            api_key: None,
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    let custom_model = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai_compatible" && model.id == "custom-coder")
        .expect("openai-compatible saved custom model option");

    assert_ne!(custom_model.supports_tools, Some(true));
    assert!(!custom_model.capabilities.supports_tools);
    assert!(!custom_model.capabilities.thinking.supported);
    let custom_runtime_model = store.runtime_config().await.unwrap().model;
    assert!(!custom_runtime_model.capabilities.supports_tools);
    assert_eq!(
        custom_runtime_model.capabilities.reasoning.protocol,
        ReasoningProtocol::None
    );

    std::env::remove_var("OPENAI_API_KEY");
}

#[tokio::test]
async fn openai_uncatalogued_model_options_and_runtime_are_conservative() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let settings = store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-future".into(),
            api_key: Some("sk-test".into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();
    let model = settings
        .model_options
        .iter()
        .find(|model| model.provider_id == "openai" && model.id == "gpt-future")
        .expect("saved uncatalogued openai model option");

    assert_ne!(model.supports_tools, Some(true));
    assert!(!model.capabilities.supports_tools);
    assert!(!model.capabilities.thinking.supported);
    let runtime_model = store.runtime_config().await.unwrap().model;
    assert!(!runtime_model.capabilities.supports_tools);
    assert_eq!(
        runtime_model.capabilities.reasoning.protocol,
        ReasoningProtocol::OpenAiReasoningEffort
    );
    assert!(
        !runtime_model
            .capabilities
            .reasoning
            .supports(ThinkingMode::Off),
        "uncatalogued OpenAI runtime config should not infer reasoning_effort none"
    );
    assert!(runtime_model
        .capabilities
        .reasoning
        .supports(ThinkingMode::High));
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
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
    let openai_compatible = settings
        .providers
        .iter()
        .find(|provider| provider.id == "openai_compatible")
        .unwrap();
    let anthropic = settings
        .providers
        .iter()
        .find(|provider| provider.id == "anthropic")
        .unwrap();
    let google = settings
        .providers
        .iter()
        .find(|provider| provider.id == "google")
        .unwrap();
    let deepseek = settings
        .providers
        .iter()
        .find(|provider| provider.id == "deepseek")
        .unwrap();
    let kimi = settings
        .providers
        .iter()
        .find(|provider| provider.id == "kimi")
        .unwrap();
    let glm = settings
        .providers
        .iter()
        .find(|provider| provider.id == "glm")
        .unwrap();
    let copilot = settings
        .providers
        .iter()
        .find(|provider| provider.id == "github_copilot")
        .unwrap();

    assert_eq!(openai.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(openai.default_base_url, "https://api.openai.com/v1");
    assert_eq!(openai.default_model, "gpt-5.5");
    assert!(openai.supports_model_discovery);
    assert!(openai.supports_tools);
    assert_eq!(openai.unsupported_reason, None);
    assert!(openai.recommended);

    assert_eq!(
        openai_compatible.protocol,
        ProviderProtocol::OpenAiChatCompletions
    );
    assert!(!openai_compatible.recommended);

    assert_eq!(anthropic.protocol, ProviderProtocol::AnthropicMessages);
    assert_eq!(anthropic.default_base_url, "https://api.anthropic.com/v1");
    assert!(anthropic.supported);
    assert!(anthropic.supports_model_discovery);
    assert_eq!(anthropic.unsupported_reason, None);

    assert_eq!(google.protocol, ProviderProtocol::GeminiGenerateContent);
    assert_eq!(
        google.default_base_url,
        "https://generativelanguage.googleapis.com/v1beta"
    );
    assert!(google.supported);
    assert_eq!(google.unsupported_reason, None);

    assert_eq!(deepseek.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(deepseek.default_base_url, "https://api.deepseek.com");
    assert!(deepseek.supported);
    assert_eq!(kimi.default_base_url, "https://api.moonshot.ai/v1");
    assert!(kimi.supported);
    assert_eq!(glm.default_base_url, "https://open.bigmodel.cn/api/paas/v4");
    assert!(glm.supported);
    assert_eq!(copilot.protocol, ProviderProtocol::CopilotOAuth);
    assert_eq!(copilot.auth_mode, ProviderAuthMode::OAuthRequired);
    assert!(copilot.supported);
    assert_eq!(copilot.unsupported_reason, None);
}
