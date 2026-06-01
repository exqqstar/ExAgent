use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use axum::{
    http::header::AUTHORIZATION, http::HeaderMap, http::StatusCode, routing::post, Json, Router,
};
use exagent_desktop::settings::{
    DesktopSettingsStore, ProviderConnectionStatus, ProviderConnectionTestRequest,
    ProviderSettingsSaveRequest, SecretStore,
};
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
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

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn without_openai_key() -> Self {
        let saved = vec![("OPENAI_API_KEY", std::env::var("OPENAI_API_KEY").ok())];
        std::env::remove_var("OPENAI_API_KEY");
        Self { saved }
    }

    fn with_openai_key(value: &str) -> Self {
        let saved = vec![("OPENAI_API_KEY", std::env::var("OPENAI_API_KEY").ok())];
        std::env::set_var("OPENAI_API_KEY", value);
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn provider_connection_test_rejects_unsupported_provider() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            model: "claude".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(
        response.status,
        ProviderConnectionStatus::UnsupportedProvider
    );
}

#[tokio::test]
async fn provider_connection_test_reports_missing_openai_credential_before_network_call() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderConnectionStatus::MissingCredential);
}

#[tokio::test]
async fn provider_connection_test_allows_openai_compatible_without_api_key() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_chat_server(
        StatusCode::OK,
        json!({
            "choices": [{
                "message": {
                    "content": "ok"
                }
            }]
        }),
        saw_authorization.clone(),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            model: "local-model".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderConnectionStatus::Success);
    assert!(!saw_authorization.load(Ordering::SeqCst));
}

#[tokio::test]
async fn provider_connection_test_does_not_leak_openai_env_key_to_compatible_endpoint() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::with_openai_key("sk-env-openai");
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_chat_server(
        StatusCode::OK,
        json!({
            "choices": [{
                "message": {
                    "content": "ok"
                }
            }]
        }),
        saw_authorization.clone(),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            model: "local-model".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderConnectionStatus::Success);
    assert!(!saw_authorization.load(Ordering::SeqCst));
}

#[tokio::test]
async fn provider_connection_test_maps_unauthorized_response() {
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_chat_server(
        StatusCode::UNAUTHORIZED,
        json!({
            "error": {
                "message": "bad key"
            }
        }),
        saw_authorization.clone(),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            model: "local-model".into(),
            api_key: Some("sk-bad".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(
        response.status,
        ProviderConnectionStatus::AuthenticationFailed
    );
    assert!(saw_authorization.load(Ordering::SeqCst));
}

#[tokio::test]
async fn provider_connection_test_reports_generic_not_found_as_provider_error() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_chat_server(
        StatusCode::NOT_FOUND,
        json!({
            "error": {
                "message": "route not found"
            }
        }),
        saw_authorization,
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            model: "local-model".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderConnectionStatus::ProviderError);
}

#[tokio::test]
async fn provider_connection_test_persists_status_for_active_provider() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_chat_server(
        StatusCode::OK,
        json!({
            "choices": [{
                "message": {
                    "content": "ok"
                }
            }]
        }),
        saw_authorization,
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai_compatible".into(),
            base_url: base_url.clone(),
            model: "local-model".into(),
            api_key: None,
            clear_api_key: false,
        })
        .await
        .unwrap();

    let response = store
        .test_provider_connection(ProviderConnectionTestRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            model: "local-model".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();
    let settings = store.load_provider_settings().await.unwrap();
    let last_connection = settings.last_connection.unwrap();

    assert_eq!(response.status, ProviderConnectionStatus::Success);
    assert_eq!(last_connection.status, ProviderConnectionStatus::Success);
    assert_eq!(last_connection.message, "Connection succeeded.");
    assert!(!last_connection.checked_at.is_empty());
}

async fn spawn_chat_server(
    status: StatusCode,
    body: serde_json::Value,
    saw_authorization: Arc<AtomicBool>,
) -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap| {
            let body = body.clone();
            let saw_authorization = saw_authorization.clone();
            async move {
                if headers.contains_key(AUTHORIZATION) {
                    saw_authorization.store(true, Ordering::SeqCst);
                }
                (status, Json(body))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}
