use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::OAuthTokenBundle;

pub const GITHUB_COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubCopilotDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub struct GitHubCopilotOAuthClient {
    auth_base_url: String,
    client: reqwest::Client,
}

impl Default for GitHubCopilotOAuthClient {
    fn default() -> Self {
        Self::with_domain("github.com")
    }
}

impl GitHubCopilotOAuthClient {
    pub fn with_domain(domain_or_url: impl AsRef<str>) -> Self {
        let value = domain_or_url.as_ref().trim().trim_end_matches('/');
        let auth_base_url = if value.starts_with("http://") || value.starts_with("https://") {
            value.to_string()
        } else if value.starts_with("127.0.0.1") || value.starts_with("localhost") {
            format!("http://{value}")
        } else {
            format!("https://{value}")
        };
        Self {
            auth_base_url,
            client: reqwest::Client::new(),
        }
    }

    pub async fn request_device_code(&self) -> Result<GitHubCopilotDeviceCode> {
        let response = self
            .client
            .post(format!("{}/login/device/code", self.auth_base_url))
            .header("Accept", "application/json")
            .json(&json!({
                "client_id": GITHUB_COPILOT_CLIENT_ID,
                "scope": "read:user",
            }))
            .send()
            .await
            .context("failed to request GitHub Copilot device code")?;
        response
            .error_for_status()
            .context("GitHub Copilot device code request failed")?
            .json()
            .await
            .context("failed to decode GitHub Copilot device code response")
    }

    pub async fn complete_device_code(
        &self,
        device: &GitHubCopilotDeviceCode,
    ) -> Result<OAuthTokenBundle> {
        let response = self
            .client
            .post(format!("{}/login/oauth/access_token", self.auth_base_url))
            .header("Accept", "application/json")
            .json(&json!({
                "client_id": GITHUB_COPILOT_CLIENT_ID,
                "device_code": device.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .context("failed to exchange GitHub Copilot device code")?;
        let token_response: GitHubCopilotTokenResponse = response
            .error_for_status()
            .context("GitHub Copilot device token exchange failed")?
            .json()
            .await
            .context("failed to decode GitHub Copilot token response")?;

        Ok(OAuthTokenBundle {
            access_token: token_response.access_token.clone(),
            refresh_token: token_response.access_token,
            expires_at_ms: None,
            account_id: None,
            account_label: None,
            raw_id_token: None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubCopilotTokenResponse {
    access_token: String,
}
