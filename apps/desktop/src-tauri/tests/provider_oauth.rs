use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{extract::Form, routing::post, Json, Router};
use exagent_desktop::provider_auth::chatgpt::{ChatGptOAuthClient, PkcePair};
use exagent_desktop::provider_auth::github_copilot::GitHubCopilotOAuthClient;
use exagent_desktop::settings::{
    CredentialAuthMethod, CredentialKind, DesktopSettingsStore, SecretStore,
};
use reqwest::Url;
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

fn fake_chatgpt_id_token() -> String {
    let payload = [
        "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgi",
        "OnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF8xMjMifSwiZW1haWwi",
        "OiJ1c2VyQGV4YW1wbGUuY29tIn0",
    ]
    .join("");
    format!("e30.{payload}.sig")
}

#[test]
fn chatgpt_browser_authorize_url_includes_codex_oauth_parameters() {
    let url = ChatGptOAuthClient::browser_authorize_url(
        "https://auth.openai.com",
        "http://localhost:1455/auth/callback",
        "state-123",
        &PkcePair {
            verifier: "verifier-123".to_string(),
            challenge: "challenge-123".to_string(),
        },
    )
    .unwrap();
    let parsed = Url::parse(&url).unwrap();
    let params = parsed.query_pairs().into_owned().collect::<HashMap<_, _>>();

    assert_eq!(
        parsed.as_str().split('?').next().unwrap(),
        "https://auth.openai.com/oauth/authorize"
    );
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(
        params.get("client_id").map(String::as_str),
        Some("app_EMoamEEZ73f0CkXaXp7hrann")
    );
    assert_eq!(
        params.get("redirect_uri").map(String::as_str),
        Some("http://localhost:1455/auth/callback")
    );
    assert_eq!(
        params.get("code_challenge").map(String::as_str),
        Some("challenge-123")
    );
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(params.get("state").map(String::as_str), Some("state-123"));
    assert_eq!(
        params.get("id_token_add_organizations").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        params.get("codex_cli_simplified_flow").map(String::as_str),
        Some("true")
    );
}

#[tokio::test]
async fn chatgpt_device_flow_exchanges_tokens_and_saves_oauth_credential() {
    let id_token = fake_chatgpt_id_token();
    let app = Router::new()
        .route(
            "/api/accounts/deviceauth/usercode",
            post(|| async {
                Json(json!({
                    "device_auth_id": "device-1",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://auth.openai.com/codex/device",
                    "expires_in": 900,
                    "interval": 1
                }))
            }),
        )
        .route(
            "/api/accounts/deviceauth/token",
            post(|| async {
                Json(json!({
                    "authorization_code": "authorization-code-1",
                    "code_verifier": "verifier-1"
                }))
            }),
        )
        .route(
            "/oauth/token",
            post(
                move |Form(form): Form<HashMap<String, String>>| async move {
                    assert_eq!(
                        form.get("grant_type").map(String::as_str),
                        Some("authorization_code")
                    );
                    assert_eq!(
                        form.get("code").map(String::as_str),
                        Some("authorization-code-1")
                    );
                    assert!(form
                        .get("redirect_uri")
                        .is_some_and(|value| value.ends_with("/deviceauth/callback")));
                    assert_eq!(
                        form.get("code_verifier").map(String::as_str),
                        Some("verifier-1")
                    );
                    Json(json!({
                        "access_token": "access-token-1",
                        "refresh_token": "refresh-token-1",
                        "expires_in": 3600,
                        "id_token": id_token
                    }))
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = ChatGptOAuthClient::with_issuer(format!("http://{addr}"));
    let device = client.request_device_code().await.unwrap();
    assert_eq!(device.user_code, "ABCD-EFGH");

    let tokens = client.complete_device_code(&device).await.unwrap();
    assert_eq!(tokens.access_token, "access-token-1");
    assert_eq!(tokens.refresh_token, "refresh-token-1");
    assert_eq!(tokens.account_id.as_deref(), Some("acct_123"));
    assert_eq!(tokens.account_label.as_deref(), Some("user@example.com"));
    assert!(
        tokens.expires_at_ms.unwrap()
            > SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
    );

    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    let settings = store
        .save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            tokens,
        )
        .await
        .unwrap();

    assert_eq!(settings.config.credential_kind, Some(CredentialKind::OAuth));
    assert_eq!(
        settings.credentials[0].account_label.as_deref(),
        Some("user@example.com")
    );
}

#[tokio::test]
async fn chatgpt_device_flow_accepts_current_usercode_response_shape() {
    let app = Router::new().route(
        "/api/accounts/deviceauth/usercode",
        post(|| async {
            Json(json!({
                "device_auth_id": "device-1",
                "user_code": "ABCD-EFGH",
                "interval": "5",
                "expires_at": "2999-01-01T00:00:00.000000+00:00"
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = ChatGptOAuthClient::with_issuer(format!("http://{addr}"));
    let device = client.request_device_code().await.unwrap();

    assert_eq!(device.user_code, "ABCD-EFGH");
    assert_eq!(
        device.verification_uri,
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(device.interval, 5);
    assert!(device.expires_in > 0);
}

#[tokio::test]
async fn github_copilot_device_flow_exchanges_token_and_saves_oauth_credential() {
    let app = Router::new()
        .route(
            "/login/device/code",
            post(|| async {
                Json(json!({
                    "device_code": "device-code-1",
                    "user_code": "WXYZ-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 1
                }))
            }),
        )
        .route(
            "/login/oauth/access_token",
            post(|Json(body): Json<serde_json::Value>| async move {
                assert_eq!(
                    body.get("grant_type").and_then(serde_json::Value::as_str),
                    Some("urn:ietf:params:oauth:grant-type:device_code")
                );
                assert_eq!(
                    body.get("device_code").and_then(serde_json::Value::as_str),
                    Some("device-code-1")
                );
                Json(json!({
                    "access_token": "copilot-token-1"
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client =
        GitHubCopilotOAuthClient::with_domain(format!("127.0.0.1:{addr}", addr = addr.port()));
    let device = client.request_device_code().await.unwrap();
    assert_eq!(device.user_code, "WXYZ-1234");

    let tokens = client.complete_device_code(&device).await.unwrap();
    assert_eq!(tokens.access_token, "copilot-token-1");
    assert_eq!(tokens.refresh_token, "copilot-token-1");
    assert!(tokens.expires_at_ms.is_none());

    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    let settings = store
        .save_oauth_credential(
            "github_copilot",
            "copilot-1",
            "GitHub Copilot",
            CredentialAuthMethod::GitHubCopilotOAuth,
            tokens,
        )
        .await
        .unwrap();

    assert_eq!(settings.active_provider_id, "github_copilot");
    assert_eq!(settings.config.credential_kind, Some(CredentialKind::OAuth));
    assert_eq!(
        settings.credentials[0].auth_method,
        Some(CredentialAuthMethod::GitHubCopilotOAuth)
    );
}
