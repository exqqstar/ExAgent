pub fn legacy_provider_api_key_account(provider_id: &str) -> String {
    format!("provider:{provider_id}:api_key")
}

pub fn credential_api_key_account(provider_id: &str, credential_id: &str) -> String {
    format!("provider:{provider_id}:credential:{credential_id}:api_key")
}

pub fn credential_oauth_account(provider_id: &str, credential_id: &str) -> String {
    format!("provider:{provider_id}:credential:{credential_id}:oauth")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_accounts_keep_api_key_and_oauth_secrets_separate() {
        assert_eq!(
            credential_api_key_account("openai", "key-1"),
            "provider:openai:credential:key-1:api_key"
        );
        assert_eq!(
            credential_oauth_account("openai", "chatgpt-1"),
            "provider:openai:credential:chatgpt-1:oauth"
        );
        assert_eq!(
            legacy_provider_api_key_account("openai"),
            "provider:openai:api_key"
        );
    }
}
