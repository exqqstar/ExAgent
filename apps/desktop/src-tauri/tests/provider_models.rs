use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use axum::{
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use exagent_desktop::settings::{
    DesktopSettingsStore, ProviderModelListRequest, ProviderModelListStatus, SecretStore,
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
async fn openai_compatible_model_discovery_lists_models_without_api_key() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let base_url = spawn_models_server(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": [
                { "id": "gpt-4.1-mini" },
                { "id": "local-coder" }
            ]
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert_eq!(
        response
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["gpt-4.1-mini", "local-coder"]
    );
    assert_eq!(response.models[0].display_name, "gpt-4.1-mini");
}

#[tokio::test]
async fn openai_compatible_model_discovery_does_not_leak_openai_env_key() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::with_openai_key("sk-env-openai");
    let saw_authorization = Arc::new(AtomicBool::new(false));
    let base_url = spawn_models_server_with_auth_probe(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": [
                { "id": "local-coder" }
            ]
        }),
        saw_authorization.clone(),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert!(!saw_authorization.load(Ordering::SeqCst));
}

#[tokio::test]
async fn openai_model_discovery_requires_credential_before_network_call() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::MissingCredential);
    assert!(response.models.is_empty());
}

#[tokio::test]
async fn model_discovery_rejects_unsupported_provider() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "anthropic".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(
        response.status,
        ProviderModelListStatus::UnsupportedProvider
    );
    assert!(response.models.is_empty());
}

#[tokio::test]
async fn model_discovery_reports_unavailable_when_models_endpoint_is_missing() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvGuard::without_openai_key();
    let base_url = spawn_models_server(
        StatusCode::NOT_FOUND,
        json!({
            "error": {
                "message": "not found"
            }
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai_compatible".into(),
            base_url,
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Unavailable);
    assert!(response.models.is_empty());
}

async fn spawn_models_server(status: StatusCode, body: serde_json::Value) -> String {
    spawn_models_server_with_auth_probe(status, body, Arc::new(AtomicBool::new(false))).await
}

async fn spawn_models_server_with_auth_probe(
    status: StatusCode,
    body: serde_json::Value,
    saw_authorization: Arc<AtomicBool>,
) -> String {
    let app = Router::new().route(
        "/v1/models",
        get(move |headers: HeaderMap| {
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
