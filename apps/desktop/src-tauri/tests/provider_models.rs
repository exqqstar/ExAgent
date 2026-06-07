use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};

use axum::{
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    routing::get,
    Json, Router,
};
use exagent::config::ThinkingMode;
use exagent::model::reasoning::ReasoningProtocol;
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

    fn with_models_dev_api_url(value: &str) -> Self {
        let saved = vec![
            (
                "EXAGENT_MODELS_DEV_API_URL",
                std::env::var("EXAGENT_MODELS_DEV_API_URL").ok(),
            ),
            (
                "EXAGENT_MODELS_DEV_DISABLED",
                std::env::var("EXAGENT_MODELS_DEV_DISABLED").ok(),
            ),
        ];
        std::env::set_var("EXAGENT_MODELS_DEV_API_URL", value);
        std::env::remove_var("EXAGENT_MODELS_DEV_DISABLED");
        Self { saved }
    }

    fn with_models_dev_disabled() -> Self {
        let saved = vec![
            (
                "EXAGENT_MODELS_DEV_API_URL",
                std::env::var("EXAGENT_MODELS_DEV_API_URL").ok(),
            ),
            (
                "EXAGENT_MODELS_DEV_DISABLED",
                std::env::var("EXAGENT_MODELS_DEV_DISABLED").ok(),
            ),
        ];
        std::env::remove_var("EXAGENT_MODELS_DEV_API_URL");
        std::env::set_var("EXAGENT_MODELS_DEV_DISABLED", "1");
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

fn lock_env() -> MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tokio::test]
async fn openai_compatible_model_discovery_lists_models_without_api_key() {
    let _guard = lock_env();
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
    let model = response
        .models
        .iter()
        .find(|model| model.id == "gpt-4.1-mini")
        .expect("discovered gpt-4.1-mini");
    assert_eq!(model.provider_id, "openai_compatible");
    assert!(!model.capabilities.thinking.supported);
    assert!(!model.capabilities.supports_tools);

    let local_model = response
        .models
        .iter()
        .find(|model| model.id == "local-coder")
        .expect("discovered local-coder");
    assert_eq!(local_model.provider_id, "openai_compatible");
    assert!(!local_model.capabilities.thinking.supported);
    assert!(!local_model.capabilities.supports_tools);
}

#[tokio::test]
async fn openai_compatible_model_discovery_does_not_leak_openai_env_key() {
    let _guard = lock_env();
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
async fn kimi_model_discovery_uses_thinking_object_reasoning_defaults() {
    let _guard = lock_env();
    let _models_dev = EnvGuard::with_models_dev_disabled();
    let base_url = spawn_models_server(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": [
                { "id": "kimi-k2.6" }
            ]
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "kimi".into(),
            base_url,
            api_key: Some("kimi-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert_eq!(response.models.len(), 1);
    let reasoning = response.models[0]
        .capabilities
        .reasoning
        .as_ref()
        .expect("kimi model should expose reasoning metadata");
    assert_eq!(
        serde_json::to_value(reasoning.protocol).unwrap(),
        json!("thinking_object")
    );
    assert_eq!(reasoning.default_mode, Some(ThinkingMode::Off));
}

#[tokio::test]
async fn openai_model_discovery_requires_credential_before_network_call() {
    let _guard = lock_env();
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
async fn vendor_model_discovery_enriches_discovered_models_from_models_dev_metadata() {
    let _guard = lock_env();
    let models_dev_url = spawn_models_dev_server(json!({
        "openai": {
            "models": {
                "gpt-future-reasoner": {
                    "id": "gpt-future-reasoner",
                    "name": "GPT future reasoner",
                    "tool_call": true,
                    "reasoning": true,
                    "limit": {
                        "context": 123456
                    }
                }
            }
        }
    }))
    .await;
    let _models_dev = EnvGuard::with_models_dev_api_url(&models_dev_url);
    let base_url = spawn_models_server(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": [
                { "id": "gpt-future-reasoner" }
            ]
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai".into(),
            base_url,
            api_key: Some("openai-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert_eq!(response.models.len(), 1);
    assert_eq!(response.models[0].id, "gpt-future-reasoner");
    assert_eq!(response.models[0].display_name, "gpt-future-reasoner");
    assert_eq!(response.models[0].context_window, Some(123456));
    assert_eq!(response.models[0].supports_tools, Some(true));
    assert!(response.models[0].capabilities.supports_tools);
    assert!(response.models[0].capabilities.thinking.supported);
    assert!(
        !response.models[0]
            .capabilities
            .thinking
            .modes
            .contains(&ThinkingMode::Off),
        "uncatalogued OpenAI metadata should not infer Off support"
    );
    let reasoning = response.models[0]
        .capabilities
        .reasoning
        .as_ref()
        .expect("openai model should expose reasoning metadata");
    assert_eq!(reasoning.protocol, ReasoningProtocol::OpenAiReasoningEffort);
    assert!(reasoning.supported_modes.contains(&ThinkingMode::High));
    assert!(
        !reasoning.supported_modes.contains(&ThinkingMode::Off),
        "uncatalogued OpenAI metadata should not expose reasoning_effort none"
    );
    assert_eq!(reasoning.default_mode, Some(ThinkingMode::Medium));
}

#[tokio::test]
async fn models_dev_metadata_does_not_add_models_missing_from_vendor_response() {
    let _guard = lock_env();
    let models_dev_url = spawn_models_dev_server(json!({
        "openai": {
            "models": {
                "metadata-only-model": {
                    "id": "metadata-only-model",
                    "name": "Metadata only model",
                    "tool_call": true,
                    "limit": {
                        "context": 999999
                    }
                }
            }
        }
    }))
    .await;
    let _models_dev = EnvGuard::with_models_dev_api_url(&models_dev_url);
    let base_url = spawn_models_server(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": []
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai".into(),
            base_url,
            api_key: Some("openai-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert!(response.models.is_empty());
}

#[tokio::test]
async fn model_discovery_succeeds_when_models_dev_metadata_is_unavailable() {
    let _guard = lock_env();
    let models_dev_url = spawn_models_dev_status_server(StatusCode::INTERNAL_SERVER_ERROR).await;
    let _models_dev = EnvGuard::with_models_dev_api_url(&models_dev_url);
    let base_url = spawn_models_server(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": [
                { "id": "new-live-model" }
            ]
        }),
    )
    .await;
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "openai".into(),
            base_url,
            api_key: Some("openai-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert_eq!(response.models.len(), 1);
    assert_eq!(response.models[0].id, "new-live-model");
    assert_eq!(response.models[0].context_window, None);
    assert_eq!(response.models[0].supports_tools, None);
}

#[tokio::test]
async fn google_model_discovery_lists_generate_content_models() {
    let _guard = lock_env();
    let _models_dev = EnvGuard::with_models_dev_disabled();
    let saw_google_key = Arc::new(AtomicBool::new(false));
    let app_saw_google_key = saw_google_key.clone();
    let app = Router::new().route(
        "/v1beta/models",
        get(move |headers: HeaderMap| {
            let app_saw_google_key = app_saw_google_key.clone();
            async move {
                if headers
                    .get("x-goog-api-key")
                    .and_then(|header| header.to_str().ok())
                    == Some("google-secret")
                {
                    app_saw_google_key.store(true, Ordering::SeqCst);
                }
                Json(json!({
                    "models": [
                        {
                            "name": "models/gemini-3-flash-preview",
                            "displayName": "Gemini 3 Flash Preview",
                            "inputTokenLimit": 1048576,
                            "outputTokenLimit": 65536,
                            "supportedGenerationMethods": ["generateContent", "countTokens"]
                        },
                        {
                            "name": "models/gemini-embedding-001",
                            "displayName": "Gemini Embedding",
                            "supportedGenerationMethods": ["embedContent"]
                        }
                    ]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "google".into(),
            base_url: format!("http://{addr}/v1beta"),
            api_key: Some("google-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert!(saw_google_key.load(Ordering::SeqCst));
    assert_eq!(response.models.len(), 1);
    assert_eq!(response.models[0].id, "gemini-3-flash-preview");
    assert_eq!(response.models[0].display_name, "Gemini 3 Flash Preview");
    assert_eq!(response.models[0].context_window, Some(1_048_576));
    assert!(response.models[0].capabilities.supports_tools);
    let reasoning = response.models[0]
        .capabilities
        .reasoning
        .as_ref()
        .expect("google catalog model should expose reasoning metadata");
    assert_eq!(reasoning.protocol, ReasoningProtocol::GeminiThinkingConfig);
    assert!(reasoning.supported_modes.contains(&ThinkingMode::Off));
    assert!(reasoning.supported_modes.contains(&ThinkingMode::High));
    assert_eq!(reasoning.default_mode, Some(ThinkingMode::High));
}

#[tokio::test]
async fn anthropic_model_discovery_lists_models_with_api_key() {
    let _guard = lock_env();
    let _models_dev = EnvGuard::with_models_dev_disabled();
    let saw_anthropic_key = Arc::new(AtomicBool::new(false));
    let app_saw_anthropic_key = saw_anthropic_key.clone();
    let app = Router::new().route(
        "/v1/models",
        get(move |headers: HeaderMap| {
            let app_saw_anthropic_key = app_saw_anthropic_key.clone();
            async move {
                if headers
                    .get("x-api-key")
                    .and_then(|header| header.to_str().ok())
                    == Some("anthropic-secret")
                    && headers.get("anthropic-version")
                        == Some(&HeaderValue::from_static("2023-06-01"))
                {
                    app_saw_anthropic_key.store(true, Ordering::SeqCst);
                }
                Json(json!({
                    "data": [
                        {
                            "type": "model",
                            "id": "claude-sonnet-4-6",
                            "display_name": "Claude Sonnet 4.6"
                        },
                        {
                            "type": "model",
                            "id": "claude-haiku-4-5",
                            "display_name": "Claude Haiku 4.5"
                        }
                    ],
                    "has_more": false
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "anthropic".into(),
            base_url: format!("http://{addr}/v1"),
            api_key: Some("anthropic-secret".into()),
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Success);
    assert!(saw_anthropic_key.load(Ordering::SeqCst));
    assert_eq!(
        response
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.display_name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
            ("claude-haiku-4-5", "Claude Haiku 4.5")
        ]
    );
    assert_eq!(response.models[0].provider_id, "anthropic");
    let reasoning = response.models[0]
        .capabilities
        .reasoning
        .as_ref()
        .expect("anthropic catalog model should expose reasoning metadata");
    assert_eq!(
        reasoning.protocol,
        ReasoningProtocol::AnthropicThinkingBudget
    );
    assert!(reasoning.supported_modes.contains(&ThinkingMode::Off));
    assert!(reasoning.supported_modes.contains(&ThinkingMode::High));
    assert_eq!(reasoning.default_mode, Some(ThinkingMode::Medium));
}

#[tokio::test]
async fn model_discovery_reports_unavailable_for_copilot_provider() {
    let dir = tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let response = store
        .list_provider_models(ProviderModelListRequest {
            provider_id: "github_copilot".into(),
            base_url: "".into(),
            api_key: None,
            use_saved_api_key: false,
        })
        .await
        .unwrap();

    assert_eq!(response.status, ProviderModelListStatus::Unavailable);
    assert!(response.models.is_empty());
}

#[tokio::test]
async fn model_discovery_reports_unavailable_when_models_endpoint_is_missing() {
    let _guard = lock_env();
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

async fn spawn_models_dev_server(body: serde_json::Value) -> String {
    let app = Router::new().route(
        "/api.json",
        get(move || {
            let body = body.clone();
            async move { Json(body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/api.json")
}

async fn spawn_models_dev_status_server(status: StatusCode) -> String {
    let app = Router::new().route("/api.json", get(move || async move { status }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/api.json")
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
