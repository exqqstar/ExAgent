use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use reqwest::Url;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::OAuthTokenBundle;

pub const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const DEFAULT_CHATGPT_ISSUER: &str = "https://auth.openai.com";
pub const DEFAULT_CHATGPT_DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
const DEFAULT_CHATGPT_DEVICE_EXPIRES_IN: u64 = 900;
const DEFAULT_CHATGPT_DEVICE_INTERVAL: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

impl PkcePair {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 64];
        rand::rng().fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptDeviceCode {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub struct ChatGptOAuthClient {
    issuer: String,
    client: reqwest::Client,
}

impl Default for ChatGptOAuthClient {
    fn default() -> Self {
        Self::with_issuer(DEFAULT_CHATGPT_ISSUER)
    }
}

impl ChatGptOAuthClient {
    pub fn with_issuer(issuer: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into().trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn browser_authorize_url(
        issuer: &str,
        redirect_uri: &str,
        state: &str,
        pkce: &PkcePair,
    ) -> Result<String> {
        let mut url = Url::parse(&format!("{}/oauth/authorize", issuer.trim_end_matches('/')))
            .context("invalid ChatGPT OAuth issuer")?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CHATGPT_CLIENT_ID)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair(
                "scope",
                "openid profile email offline_access api.connectors.read api.connectors.invoke",
            )
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", "exagent");
        Ok(url.to_string())
    }

    pub async fn request_device_code(&self) -> Result<ChatGptDeviceCode> {
        let response = self
            .client
            .post(format!("{}/api/accounts/deviceauth/usercode", self.issuer))
            .json(&json!({ "client_id": CHATGPT_CLIENT_ID }))
            .send()
            .await
            .context("failed to request ChatGPT device code")?;
        response
            .error_for_status()
            .context("ChatGPT device code request failed")?
            .json::<ChatGptDeviceCodeResponse>()
            .await
            .context("failed to decode ChatGPT device code response")
            .map(ChatGptDeviceCodeResponse::into_device_code)
    }

    pub async fn complete_device_code(
        &self,
        device: &ChatGptDeviceCode,
    ) -> Result<OAuthTokenBundle> {
        let response = self
            .client
            .post(format!("{}/api/accounts/deviceauth/token", self.issuer))
            .json(&json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await
            .context("failed to poll ChatGPT device authorization")?;
        let code_response: DeviceTokenResponse = response
            .error_for_status()
            .context("ChatGPT device authorization is not complete")?
            .json()
            .await
            .context("failed to decode ChatGPT device authorization response")?;

        self.exchange_code_for_tokens(
            &code_response.authorization_code,
            &format!("{}/deviceauth/callback", self.issuer),
            &code_response.code_verifier,
        )
        .await
    }

    pub async fn exchange_code_for_tokens(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenBundle> {
        let response = self
            .client
            .post(format!("{}/oauth/token", self.issuer))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", CHATGPT_CLIENT_ID),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await
            .context("failed to exchange ChatGPT OAuth code")?;
        let token_response: TokenResponse = response
            .error_for_status()
            .context("ChatGPT OAuth token exchange failed")?
            .json()
            .await
            .context("failed to decode ChatGPT OAuth token response")?;

        Ok(token_response.into_bundle())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ChatGptDeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64")]
    expires_in: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64")]
    interval: Option<u64>,
}

impl ChatGptDeviceCodeResponse {
    fn into_device_code(self) -> ChatGptDeviceCode {
        ChatGptDeviceCode {
            device_auth_id: self.device_auth_id,
            user_code: self.user_code,
            verification_uri: self
                .verification_uri
                .unwrap_or_else(|| DEFAULT_CHATGPT_DEVICE_VERIFICATION_URI.to_string()),
            expires_in: self.expires_in.unwrap_or(DEFAULT_CHATGPT_DEVICE_EXPIRES_IN),
            interval: self.interval.unwrap_or(DEFAULT_CHATGPT_DEVICE_INTERVAL),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum U64OrString {
    Number(u64),
    String(String),
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<U64OrString>::deserialize(deserializer)?;
    match value {
        Some(U64OrString::Number(number)) => Ok(Some(number)),
        Some(U64OrString::String(text)) => text.parse::<u64>().map(Some).map_err(de::Error::custom),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    id_token: Option<String>,
}

impl TokenResponse {
    fn into_bundle(self) -> OAuthTokenBundle {
        let claims = self
            .id_token
            .as_deref()
            .and_then(parse_chatgpt_id_token_claims);
        OAuthTokenBundle {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at_ms: self.expires_in.map(|seconds| now_millis() + seconds * 1000),
            account_id: claims.as_ref().and_then(|claims| claims.account_id.clone()),
            account_label: claims.as_ref().and_then(|claims| claims.email.clone()),
            raw_id_token: self.id_token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatGptTokenClaims {
    account_id: Option<String>,
    email: Option<String>,
}

fn parse_chatgpt_id_token_claims(token: &str) -> Option<ChatGptTokenClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let auth_claims = value.get("https://api.openai.com/auth");
    let account_id = auth_claims
        .and_then(|claims| claims.get("chatgpt_account_id"))
        .or_else(|| value.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let email = value
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(ChatGptTokenClaims { account_id, email })
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chatgpt_account_claims_from_id_token() {
        let claims = parse_chatgpt_id_token_claims("e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF8xMjMifSwiZW1haWwiOiJ1c2VyQGV4YW1wbGUuY29tIn0.sig")
            .expect("claims");

        assert_eq!(claims.account_id.as_deref(), Some("acct_123"));
        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
    }
}
