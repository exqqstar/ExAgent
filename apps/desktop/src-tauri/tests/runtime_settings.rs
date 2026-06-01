use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use exagent_desktop::settings::{
    DesktopSettingsStore, McpServerSettings, ProviderSettingsSaveRequest, RuntimePresetSettings,
    RuntimeSettingsSaveRequest, SecretStore, SkillRootSettings,
};
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

#[tokio::test]
async fn runtime_settings_round_trip_defaults_mcp_and_skills() {
    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );

    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: Some(exagent::config::ThinkingMode::High),
            presets: vec![RuntimePresetSettings {
                id: "deep-code".into(),
                name: "Deep Code".into(),
                model: "gpt-4.1".into(),
                thinking_mode: Some(exagent::config::ThinkingMode::High),
            }],
            mcp_servers: vec![McpServerSettings {
                id: "filesystem".into(),
                name: "Filesystem".into(),
                enabled: true,
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into(),
                ],
                env: vec![("ROOT".into(), "/tmp".into())],
                working_directory: None,
            }],
            skill_roots: vec![SkillRootSettings {
                id: "local-skills".into(),
                name: "Local skills".into(),
                enabled: true,
                path: dir.path().display().to_string(),
                scope: "global".into(),
            }],
        })
        .await
        .unwrap();

    let loaded = store.load_runtime_settings().await.unwrap();
    assert_eq!(loaded.default_model, "gpt-4.1-mini");
    assert_eq!(
        loaded.default_thinking_mode,
        Some(exagent::config::ThinkingMode::High)
    );
    assert_eq!(loaded.presets[0].id, "deep-code");
    assert_eq!(loaded.mcp_servers[0].command, "npx");
    assert_eq!(loaded.skill_roots[0].name, "Local skills");
}

#[tokio::test]
async fn runtime_settings_reject_blank_mcp_command() {
    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );

    let error = store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![McpServerSettings {
                id: "bad".into(),
                name: "Bad".into(),
                enabled: true,
                command: " ".into(),
                args: vec![],
                env: vec![],
                working_directory: None,
            }],
            skill_roots: vec![],
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("MCP command is required"));
}

#[tokio::test]
async fn provider_settings_save_preserves_runtime_mcp_and_skills() {
    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );

    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: Some(exagent::config::ThinkingMode::Medium),
            presets: vec![],
            mcp_servers: vec![McpServerSettings {
                id: "filesystem".into(),
                name: "Filesystem".into(),
                enabled: false,
                command: "npx".into(),
                args: vec![],
                env: vec![],
                working_directory: None,
            }],
            skill_roots: vec![SkillRootSettings {
                id: "local-skills".into(),
                name: "Local skills".into(),
                enabled: false,
                path: String::new(),
                scope: "global".into(),
            }],
        })
        .await
        .unwrap();

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

    let loaded = store.load_runtime_settings().await.unwrap();
    assert_eq!(loaded.default_model, "gpt-4.1-mini");
    assert_eq!(loaded.mcp_servers[0].id, "filesystem");
    assert_eq!(loaded.skill_roots[0].id, "local-skills");
}
