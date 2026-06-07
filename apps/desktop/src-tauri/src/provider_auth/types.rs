use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
}

impl Default for CredentialKind {
    fn default() -> Self {
        Self::ApiKey
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Active,
    Expired,
    NeedsLogin,
}

impl Default for CredentialStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAuthMethod {
    #[serde(rename = "chatgpt_oauth")]
    ChatGptOAuth,
    #[serde(rename = "github_copilot_oauth")]
    GitHubCopilotOAuth,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokenBundle {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub raw_id_token: Option<String>,
}

impl OAuthTokenBundle {
    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms
            .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
    }
}

impl fmt::Debug for OAuthTokenBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthTokenBundle")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("account_id", &self.account_id)
            .field("account_label", &self.account_label)
            .field(
                "raw_id_token",
                &self.raw_id_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::OAuthTokenBundle;

    #[test]
    fn oauth_token_debug_redacts_secrets() {
        let debug = format!(
            "{:?}",
            OAuthTokenBundle {
                access_token: "access-secret".to_string(),
                refresh_token: "refresh-secret".to_string(),
                expires_at_ms: Some(42),
                account_id: Some("acct_123".to_string()),
                account_label: Some("user@example.com".to_string()),
                raw_id_token: Some("id-secret".to_string()),
            }
        );

        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(!debug.contains("id-secret"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("acct_123"));
    }
}
