use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use exagent_desktop::settings::{
    DesktopSettingsStore, McpServerSettings, ProviderSettingsSaveRequest, RuntimePresetSettings,
    RuntimeSettingsSaveRequest, SecretStore, SkillRootSettings,
};
use tempfile::tempdir;

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct EnvVarRestore {
    key: &'static str,
    prior: Option<OsString>,
}

impl EnvVarRestore {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let prior = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prior }
    }

    fn remove(key: &'static str) -> Self {
        let prior = std::env::var_os(key);
        std::env::remove_var(key);
        Self { key, prior }
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
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

fn write_skill(root: &Path, dir_name: &str, frontmatter: &str) -> std::path::PathBuf {
    let skill_dir = root.join(dir_name);
    fs::create_dir_all(&skill_dir).unwrap();
    let path = skill_dir.join("SKILL.md");
    fs::write(&path, format!("---\n{frontmatter}\n---\n\nbody\n")).unwrap();
    path
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
async fn runtime_config_preserves_env_skill_root_when_settings_roots_are_saved() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    let env_root = dir.path().join("env-skills");
    let settings_root = dir.path().join("settings-skills");
    let _env = EnvVarRestore::set_path("EXAGENT_SKILLS_USER_ROOT", &env_root);

    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![],
            skill_roots: vec![SkillRootSettings {
                id: "settings-skills".into(),
                name: "Settings skills".into(),
                enabled: true,
                path: settings_root.display().to_string(),
                scope: "global".into(),
            }],
        })
        .await
        .unwrap();

    let config = store.runtime_config().await.unwrap();

    assert_eq!(config.skills_user_roots, vec![env_root, settings_root]);
}

#[tokio::test]
async fn runtime_config_includes_enabled_skill_roots_and_ignores_disabled_roots() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let _env = EnvVarRestore::remove("EXAGENT_SKILLS_USER_ROOT");
    let dir = tempdir().unwrap();
    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    let global_root = dir.path().join("global-skills");
    let user_root = dir.path().join("user-skills");
    let disabled_root = dir.path().join("disabled-skills");

    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![],
            skill_roots: vec![
                SkillRootSettings {
                    id: "global-skills".into(),
                    name: "Global skills".into(),
                    enabled: true,
                    path: format!(" {} ", global_root.display()),
                    scope: "global".into(),
                },
                SkillRootSettings {
                    id: "user-skills".into(),
                    name: "User skills".into(),
                    enabled: true,
                    path: user_root.display().to_string(),
                    scope: "user".into(),
                },
                SkillRootSettings {
                    id: "disabled-skills".into(),
                    name: "Disabled skills".into(),
                    enabled: false,
                    path: disabled_root.display().to_string(),
                    scope: "global".into(),
                },
                SkillRootSettings {
                    id: "project-source".into(),
                    name: "Project source".into(),
                    enabled: true,
                    path: dir.path().join(".agents/skills").display().to_string(),
                    scope: "project".into(),
                },
            ],
        })
        .await
        .unwrap();

    let config = store.runtime_config().await.unwrap();

    assert_eq!(
        config.skills_user_roots,
        vec![global_root.clone(), user_root.clone()]
    );
}

#[tokio::test]
async fn runtime_settings_bootstraps_existing_desktop_skill_roots() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let _env_root = EnvVarRestore::remove("EXAGENT_SKILLS_USER_ROOT");
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let default_root = home.join(".agents").join("skills");
    fs::create_dir_all(&default_root).unwrap();
    let _home = EnvVarRestore::set_path("HOME", &home);
    write_skill(
        &default_root,
        "default-skill",
        "name: default-skill\ndescription: Default desktop skill",
    );

    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![],
            skill_roots: vec![],
        })
        .await
        .unwrap();

    let settings = store.load_runtime_settings().await.unwrap();
    assert_eq!(settings.skill_roots.len(), 1);
    assert_eq!(settings.skill_roots[0].id, "user-skills");
    assert_eq!(
        settings.skill_roots[0].path,
        default_root.display().to_string()
    );

    let config = store.runtime_config().await.unwrap();
    assert_eq!(config.skills_user_roots, vec![default_root.clone()]);

    let scan = store.scan_skill_catalog(None).await.unwrap();
    assert!(scan
        .sources
        .iter()
        .any(|source| source.id == "user-skills" && source.skill_count == 1));
    assert!(scan
        .skills
        .iter()
        .any(|skill| skill.name == "default-skill"));
}

#[tokio::test]
async fn scan_skill_catalog_reports_sources_explicit_only_and_shadowed_skills() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    let project_root = workspace.join(".agents").join("skills");
    let global_root = dir.path().join("global-skills");
    let missing_root = dir.path().join("missing-skills");
    let disabled_root = dir.path().join("disabled-skills");
    fs::create_dir_all(&workspace).unwrap();

    let project_shared = write_skill(
        &project_root,
        "shared-project",
        "name: shared\ndescription: Project skill wins",
    );
    let global_shared = write_skill(
        &global_root,
        "shared-global",
        "name: shared\ndescription: Shadowed global skill",
    );
    let explicit_only = write_skill(
        &global_root,
        "explicit-only",
        "name: explicit-only\ndescription: Explicit only skill\nallow_implicit_invocation: false",
    );
    write_skill(
        &disabled_root,
        "disabled-only",
        "name: disabled-only\ndescription: Disabled source skill",
    );

    let store = DesktopSettingsStore::with_secret_store(
        dir.path().join("settings.json"),
        Arc::new(MemorySecrets::default()),
    );
    store
        .save_runtime_settings(RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: None,
            presets: vec![],
            mcp_servers: vec![],
            skill_roots: vec![
                SkillRootSettings {
                    id: "global-skills".into(),
                    name: "Global skills".into(),
                    enabled: true,
                    path: global_root.display().to_string(),
                    scope: "global".into(),
                },
                SkillRootSettings {
                    id: "missing-skills".into(),
                    name: "Missing skills".into(),
                    enabled: true,
                    path: missing_root.display().to_string(),
                    scope: "global".into(),
                },
                SkillRootSettings {
                    id: "disabled-skills".into(),
                    name: "Disabled skills".into(),
                    enabled: false,
                    path: disabled_root.display().to_string(),
                    scope: "global".into(),
                },
            ],
        })
        .await
        .unwrap();

    let scan = store.scan_skill_catalog(Some(workspace)).await.unwrap();

    assert_eq!(scan.sources.len(), 4);
    let project_source = scan
        .sources
        .iter()
        .find(|source| source.scope == "project")
        .unwrap();
    assert_eq!(project_source.status, "ready");
    assert_eq!(project_source.skill_count, 1);
    assert_eq!(project_source.warning_count, 0);
    let global_source = scan
        .sources
        .iter()
        .find(|source| source.id == "global-skills")
        .unwrap();
    assert_eq!(global_source.status, "ready");
    assert_eq!(global_source.skill_count, 2);
    assert_eq!(global_source.warning_count, 1);
    assert_eq!(
        scan.sources
            .iter()
            .find(|source| source.id == "missing-skills")
            .unwrap()
            .status,
        "missing"
    );
    assert_eq!(
        scan.sources
            .iter()
            .find(|source| source.id == "disabled-skills")
            .unwrap()
            .status,
        "disabled"
    );

    let active_shared = scan
        .skills
        .iter()
        .find(|skill| skill.name == "shared" && skill.status == "active")
        .unwrap();
    assert_eq!(active_shared.scope, "project");
    assert_eq!(active_shared.path, project_shared.display().to_string());
    assert!(active_shared.effective_implicit);

    let shadowed_shared = scan
        .skills
        .iter()
        .find(|skill| skill.name == "shared" && skill.status == "shadowed")
        .unwrap();
    assert_eq!(shadowed_shared.scope, "global");
    assert_eq!(shadowed_shared.path, global_shared.display().to_string());
    assert!(!shadowed_shared.effective_implicit);

    let explicit = scan
        .skills
        .iter()
        .find(|skill| skill.name == "explicit-only")
        .unwrap();
    assert_eq!(explicit.path, explicit_only.display().to_string());
    assert!(!explicit.allow_implicit_invocation);
    assert!(!explicit.effective_implicit);
    assert_eq!(explicit.status, "explicit_only");

    let duplicate = scan
        .warnings
        .iter()
        .find(|warning| warning.kind == "duplicate_name" && warning.name == "shared")
        .unwrap();
    assert!(duplicate
        .paths
        .iter()
        .any(|path| path == &project_shared.display().to_string()));
    assert!(duplicate
        .paths
        .iter()
        .any(|path| path == &global_shared.display().to_string()));
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
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        })
        .await
        .unwrap();

    let loaded = store.load_runtime_settings().await.unwrap();
    assert_eq!(loaded.default_model, "gpt-4.1-mini");
    assert_eq!(loaded.mcp_servers[0].id, "filesystem");
    assert_eq!(loaded.skill_roots[0].id, "local-skills");
}
