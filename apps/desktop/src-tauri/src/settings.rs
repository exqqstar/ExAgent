use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model_catalog::{capabilities_for_model, catalog_models_for_provider, CatalogReasoning};
use crate::model_metadata::{self, ModelsDevModelMetadata};
use crate::provider_auth::chatgpt::ChatGptOAuthClient;
use crate::provider_auth::github_copilot::GitHubCopilotOAuthClient;
use crate::provider_auth::{
    credential_api_key_account, credential_oauth_account, legacy_provider_api_key_account,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use exagent::config::{
    load_skills, AgentConfig, SkillConfig, SkillMetadata, SkillScope, SkillWarning,
    SkillWarningKind, ThinkingMode, WebSearchConfig,
};
use exagent::llm::OpenAiCompatibleLlm;
use exagent::mcp::config::McpServerConfig;
use exagent::model::chatgpt_codex::{ChatGptCodexTokenRefreshSink, ChatGptCodexTokenUpdate};
use exagent::model::github_copilot::{
    models_endpoint as copilot_models_endpoint, CopilotRequestBuilderExt,
};
use exagent::model::reasoning::ReasoningCapabilities;
use exagent::provider::{provider_profile_by_id, provider_profiles, ProviderProfile};
use exagent::resolved::{ModelRef, ResolvedModelConfig};
use exagent::resolver::{model_context_window_from_env, resolve_from_profile, ModelResolver};
use keyring::Entry;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use crate::model_catalog::{ModelCapabilities, ThinkingCapability};
pub use crate::provider_auth::chatgpt::ChatGptDeviceCode;
pub use crate::provider_auth::github_copilot::GitHubCopilotDeviceCode;
pub use crate::provider_auth::{
    CredentialAuthMethod, CredentialKind, CredentialStatus, OAuthTokenBundle,
};
pub use exagent::provider::{ProviderAuthMode, ProviderProtocol};

const SECRET_SERVICE: &str = "io.github.exqqstar.exagent";
const DEFAULT_PROVIDER_ID: &str = "openai";
const DEFAULT_CREDENTIAL_ID: &str = "key-1";
const DEFAULT_WEB_SEARCH_PROVIDER: &str = "brave";

pub trait SecretStore: Send + Sync {
    fn get_secret(&self, account: &str) -> Result<Option<String>>;
    fn set_secret(&self, account: &str, secret: &str) -> Result<()>;
    fn delete_secret(&self, account: &str) -> Result<()>;
}

#[derive(Default)]
pub struct KeyringSecretStore;

impl SecretStore for KeyringSecretStore {
    fn get_secret(&self, account: &str) -> Result<Option<String>> {
        let entry = Entry::new(SECRET_SERVICE, account)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
        Entry::new(SECRET_SERVICE, account)?
            .set_password(secret)
            .map_err(Into::into)
    }

    fn delete_secret(&self, account: &str) -> Result<()> {
        let entry = Entry::new(SECRET_SERVICE, account)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// Stores secrets as a plaintext JSON map in `auth.json` (owner-only `0600`).
///
/// This is the default backend: it never touches the OS keychain, so it does
/// not trigger macOS Keychain access prompts. The trade-off is that secrets are
/// stored in plaintext on disk, readable by any process running as the user.
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read_map(&self) -> Result<serde_json::Map<String, Value>> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(serde_json::Map::new()),
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes).with_context(|| {
                    format!("failed to parse secrets at {}", self.path.display())
                })?;
                match value {
                    Value::Object(map) => Ok(map),
                    _ => Ok(serde_json::Map::new()),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(serde_json::Map::new())
            }
            Err(error) => Err(error)
                .with_context(|| format!("failed to read secrets at {}", self.path.display())),
        }
    }

    fn write_map(&self, map: &serde_json::Map<String, Value>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create secrets dir {}", parent.display()))?;
        }
        let contents = serde_json::to_vec_pretty(&Value::Object(map.clone()))?;
        // Write to a temp file and rename so a crash mid-write can't truncate
        // the existing secrets, and apply 0600 before the file holds any data.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &contents)
            .with_context(|| format!("failed to write secrets at {}", tmp.display()))?;
        set_owner_only_permissions(&tmp)?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("failed to persist secrets at {}", self.path.display()))?;
        set_owner_only_permissions(&self.path)?;
        Ok(())
    }
}

impl SecretStore for FileSecretStore {
    fn get_secret(&self, account: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().expect("secret store lock poisoned");
        let map = self.read_map()?;
        Ok(map.get(account).and_then(Value::as_str).map(str::to_string))
    }

    fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("secret store lock poisoned");
        let mut map = self.read_map()?;
        map.insert(account.to_string(), Value::String(secret.to_string()));
        self.write_map(&map)
    }

    fn delete_secret(&self, account: &str) -> Result<()> {
        let _guard = self.lock.lock().expect("secret store lock poisoned");
        let mut map = self.read_map()?;
        if map.remove(account).is_some() {
            self.write_map(&map)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Clone)]
pub struct DesktopSettingsStore {
    path: PathBuf,
    secrets: Arc<dyn SecretStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub recommended: bool,
    pub supported: bool,
    pub auth_mode: ProviderAuthMode,
    pub protocol: ProviderProtocol,
    pub default_base_url: String,
    pub default_model: String,
    pub supports_model_discovery: bool,
    pub supports_tools: bool,
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfigView {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
    #[serde(default)]
    pub has_credential: bool,
    #[serde(default)]
    pub credential_kind: Option<CredentialKind>,
    pub credential_source: CredentialSource,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    Keychain,
    Environment,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectedProviderView {
    pub id: String,
    pub name: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderSettingsResponse {
    pub providers: Vec<ProviderDescriptor>,
    pub active_provider_id: String,
    #[serde(default)]
    pub active_credential_id: Option<String>,
    #[serde(default)]
    pub credentials: Vec<ProviderCredentialView>,
    pub config: ProviderConfigView,
    pub connected_provider: Option<ConnectedProviderView>,
    pub last_connection: Option<ProviderConnectionStatusView>,
    #[serde(default)]
    pub configured_providers: Vec<ProviderConfigView>,
    #[serde(default)]
    pub model_options: Vec<ProviderModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsSaveRequest {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub clear_api_key: bool,
    #[serde(default)]
    pub credential_id: Option<String>,
    #[serde(default)]
    pub create_credential: bool,
    #[serde(default)]
    pub model_options: Vec<ProviderModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCredentialView {
    pub id: String,
    pub label: String,
    pub source: CredentialSource,
    #[serde(default)]
    pub kind: CredentialKind,
    #[serde(default)]
    pub status: CredentialStatus,
    #[serde(default)]
    pub auth_method: Option<CredentialAuthMethod>,
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionTestRequest {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub use_saved_api_key: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConnectionStatus {
    Success,
    UnsupportedProvider,
    MissingCredential,
    AuthenticationFailed,
    ModelNotFound,
    NetworkError,
    ProviderError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConnectionTestResponse {
    pub status: ProviderConnectionStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConnectionStatusView {
    pub status: ProviderConnectionStatus,
    pub message: String,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelListRequest {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub use_saved_api_key: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderModelListStatus {
    Success,
    UnsupportedProvider,
    MissingCredential,
    Unavailable,
    AuthenticationFailed,
    NetworkError,
    ProviderError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderModelView {
    #[serde(default)]
    pub provider_id: String,
    pub id: String,
    pub display_name: String,
    pub context_window: Option<i64>,
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderModelListResponse {
    pub status: ProviderModelListStatus,
    pub message: String,
    pub models: Vec<ProviderModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePresetSettings {
    pub id: String,
    pub name: String,
    pub model: String,
    pub thinking_mode: Option<ThinkingMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerSettings {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRootSettings {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub path: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_search_provider")]
    pub provider: String,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

impl Default for WebSearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: DEFAULT_WEB_SEARCH_PROVIDER.to_string(),
            has_api_key: false,
            api_key: None,
            clear_api_key: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingsResponse {
    pub default_model: String,
    pub default_thinking_mode: Option<ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
    pub web_search: WebSearchSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingsSaveRequest {
    pub default_model: String,
    pub default_thinking_mode: Option<ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
    #[serde(default)]
    pub web_search: WebSearchSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SkillCatalogScanResponse {
    pub sources: Vec<SkillSourceView>,
    pub skills: Vec<SkillCatalogItemView>,
    pub warnings: Vec<SkillCatalogWarningView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SkillSourceView {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub enabled: bool,
    pub path: String,
    pub status: String,
    pub skill_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SkillCatalogItemView {
    pub name: String,
    pub scope: String,
    pub description: String,
    pub path: String,
    pub source_id: String,
    pub allow_implicit_invocation: bool,
    pub effective_implicit: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SkillCatalogWarningView {
    pub kind: String,
    pub scope: String,
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SettingsFile {
    provider_id: String,
    base_url: String,
    model: String,
    #[serde(default)]
    provider_configs: HashMap<String, ProviderConfigSettings>,
    #[serde(default)]
    provider_credentials: HashMap<String, ProviderCredentialCollection>,
    #[serde(default)]
    last_connection_status: Option<ProviderConnectionStatus>,
    #[serde(default)]
    last_connection_message: Option<String>,
    #[serde(default)]
    last_connection_checked_at: Option<String>,
    #[serde(default = "default_runtime_model")]
    runtime_default_model: String,
    #[serde(default)]
    runtime_default_thinking_mode: Option<ThinkingMode>,
    #[serde(default)]
    runtime_presets: Vec<RuntimePresetSettings>,
    #[serde(default)]
    mcp_servers: Vec<McpServerSettings>,
    #[serde(default)]
    skill_roots: Vec<SkillRootSettings>,
    #[serde(default)]
    web_search: StoredWebSearchSettings,
}

impl Default for SettingsFile {
    fn default() -> Self {
        let profile = default_provider_profile();
        Self {
            provider_id: profile.id.to_string(),
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| profile.default_base_url.to_string()),
            model: std::env::var("OPENAI_MODEL")
                .unwrap_or_else(|_| profile.default_model.to_string()),
            provider_configs: HashMap::new(),
            provider_credentials: HashMap::new(),
            last_connection_status: None,
            last_connection_message: None,
            last_connection_checked_at: None,
            runtime_default_model: profile.default_model.to_string(),
            runtime_default_thinking_mode: None,
            runtime_presets: Vec::new(),
            mcp_servers: Vec::new(),
            skill_roots: Vec::new(),
            web_search: StoredWebSearchSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StoredWebSearchSettings {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_web_search_provider")]
    provider: String,
}

impl Default for StoredWebSearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: DEFAULT_WEB_SEARCH_PROVIDER.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ProviderConfigSettings {
    base_url: String,
    model: String,
    #[serde(default)]
    model_options: Vec<ProviderModelView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ProviderCredentialCollection {
    #[serde(default)]
    active_credential_id: Option<String>,
    #[serde(default)]
    credentials: Vec<ProviderCredentialSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProviderCredentialSettings {
    id: String,
    label: String,
    #[serde(default)]
    kind: CredentialKind,
    #[serde(default)]
    auth_method: Option<CredentialAuthMethod>,
    #[serde(default)]
    account_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderCredentialSummary {
    source: CredentialSource,
    kind: Option<CredentialKind>,
    has_credential: bool,
    has_api_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderModelCatalogScope {
    ProviderApi,
    ChatGptCodex,
}

impl ProviderCredentialSummary {
    fn none() -> Self {
        Self {
            source: CredentialSource::None,
            kind: None,
            has_credential: false,
            has_api_key: false,
        }
    }

    fn from_credential(credential: &ProviderCredentialView) -> Self {
        Self {
            source: credential.source,
            kind: Some(credential.kind),
            has_credential: true,
            has_api_key: credential.kind == CredentialKind::ApiKey,
        }
    }
}

impl DesktopSettingsStore {
    pub fn new(path: PathBuf) -> Self {
        let secrets_path = path
            .parent()
            .map(|parent| parent.join("auth.json"))
            .unwrap_or_else(|| PathBuf::from("auth.json"));
        Self::with_secret_store(path, Arc::new(FileSecretStore::new(secrets_path)))
    }

    pub fn with_secret_store(path: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        Self { path, secrets }
    }

    pub async fn load_provider_settings(&self) -> Result<ProviderSettingsResponse> {
        let mut file = self.load_file().await?;
        if should_refresh_provider_model_options_on_load(&file, "github_copilot") {
            if let Ok(Ok(true)) = tokio::time::timeout(
                Duration::from_secs(5),
                self.refresh_provider_model_options("github_copilot"),
            )
            .await
            {
                file = self.load_file().await?;
            }
        }
        let credentials = self.provider_credentials_for_file(&file, &file.provider_id)?;
        let active_credential_id = active_credential_id_for_file(&file, &file.provider_id)
            .filter(|credential_id| {
                credentials
                    .iter()
                    .any(|credential| credential.id == *credential_id)
            })
            .or_else(|| credentials.first().map(|credential| credential.id.clone()));
        let active_credential = active_credential_id
            .as_deref()
            .and_then(|credential_id| credentials.iter().find(|item| item.id == credential_id));
        let mut response = self.response_from_file(
            file,
            active_credential.cloned(),
            active_credential_id,
            credentials,
        )?;
        self.enrich_active_model_option_from_cache(&mut response)
            .await;
        Ok(response)
    }

    pub async fn save_provider_settings(
        &self,
        request: ProviderSettingsSaveRequest,
    ) -> Result<ProviderSettingsResponse> {
        let provider = provider_by_id(&request.provider_id)
            .filter(|provider| provider.supported)
            .with_context(|| format!("unsupported provider `{}`", request.provider_id))?;
        let mut existing = self.load_file().await?;
        preserve_active_provider_config(&mut existing);
        let existing_credential_ids = existing
            .provider_credentials
            .get(&provider.id)
            .map(|collection| {
                collection
                    .credentials
                    .iter()
                    .map(|credential| credential.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut file = SettingsFile {
            provider_id: provider.id.clone(),
            base_url: normalized_or_default(&request.base_url, &provider.default_base_url),
            model: normalized_or_default(&request.model, &provider.default_model),
            provider_configs: existing.provider_configs,
            provider_credentials: existing.provider_credentials,
            last_connection_status: None,
            last_connection_message: None,
            last_connection_checked_at: None,
            runtime_default_model: existing.runtime_default_model,
            runtime_default_thinking_mode: existing.runtime_default_thinking_mode,
            runtime_presets: existing.runtime_presets,
            mcp_servers: existing.mcp_servers,
            skill_roots: existing.skill_roots,
            web_search: existing.web_search,
        };

        let selected_credential_id = self.apply_credential_save_request(&mut file, &request)?;
        file.provider_configs.insert(
            provider.id.clone(),
            ProviderConfigSettings {
                base_url: file.base_url.clone(),
                model: file.model.clone(),
                model_options: request.model_options.clone(),
            },
        );

        if request.clear_api_key {
            let mut credential_ids = existing_credential_ids;
            if let Some(credential_id) = selected_credential_id {
                credential_ids.push(credential_id);
            }
            credential_ids.push(DEFAULT_CREDENTIAL_ID.to_string());
            credential_ids.sort();
            credential_ids.dedup();
            for credential_id in credential_ids {
                self.delete_credential_secrets(&file.provider_id, &credential_id)?;
            }
            self.secrets
                .delete_secret(&legacy_provider_api_key_account(&file.provider_id))?;
        } else if let Some(api_key) = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.secrets
                .set_secret(&legacy_provider_api_key_account(&file.provider_id), api_key)?;
            if let Some(credential_id) = selected_credential_id.as_deref() {
                self.secrets.set_secret(
                    &credential_api_key_account(&file.provider_id, credential_id),
                    api_key,
                )?;
            }
        }

        self.save_file(&file).await?;
        self.load_provider_settings().await
    }

    pub async fn save_oauth_credential(
        &self,
        provider_id: &str,
        credential_id: &str,
        label: &str,
        auth_method: CredentialAuthMethod,
        tokens: OAuthTokenBundle,
    ) -> Result<ProviderSettingsResponse> {
        let provider = provider_by_id(provider_id)
            .filter(|provider| provider.supported)
            .with_context(|| format!("unsupported provider `{provider_id}`"))?;
        let mut file = self.load_file().await?;
        preserve_active_provider_config(&mut file);
        let provider_config =
            effective_provider_config(&file, provider_profile_by_id(provider_id).unwrap());
        file.provider_id = provider.id.clone();
        file.base_url = provider_config
            .as_ref()
            .map(|config| normalized_or_default(&config.base_url, &provider.default_base_url))
            .unwrap_or_else(|| provider.default_base_url.clone());
        file.model = provider_config
            .as_ref()
            .map(|config| normalized_or_default(&config.model, &provider.default_model))
            .unwrap_or_else(|| provider.default_model.clone());
        file.provider_configs.insert(
            provider.id.clone(),
            ProviderConfigSettings {
                base_url: file.base_url.clone(),
                model: file.model.clone(),
                model_options: provider_config
                    .map(|config| config.model_options)
                    .unwrap_or_default(),
            },
        );

        let mut collection = file
            .provider_credentials
            .remove(provider_id)
            .unwrap_or_default();
        collection
            .credentials
            .retain(|credential| credential.id != credential_id);
        collection.credentials.push(ProviderCredentialSettings {
            id: credential_id.to_string(),
            label: normalized_or_default(label, "OAuth credential"),
            kind: CredentialKind::OAuth,
            auth_method: Some(auth_method),
            account_label: tokens.account_label.clone(),
        });
        collection.active_credential_id = Some(credential_id.to_string());
        file.provider_credentials
            .insert(provider_id.to_string(), collection);

        self.secrets.set_secret(
            &credential_oauth_account(provider_id, credential_id),
            &serde_json::to_string(&tokens)?,
        )?;
        self.save_file(&file).await?;
        self.load_provider_settings().await
    }

    async fn save_refreshed_oauth_credential(
        &self,
        provider_id: &str,
        credential_id: &str,
        auth_method: CredentialAuthMethod,
        tokens: OAuthTokenBundle,
    ) -> Result<()> {
        let mut file = self.load_file().await?;
        let collection = file
            .provider_credentials
            .get_mut(provider_id)
            .with_context(|| format!("provider `{provider_id}` has no OAuth credential store"))?;
        let credential = collection
            .credentials
            .iter_mut()
            .find(|credential| credential.id == credential_id)
            .with_context(|| format!("OAuth credential `{credential_id}` was not found"))?;
        if credential.kind != CredentialKind::OAuth || credential.auth_method != Some(auth_method) {
            bail!("credential `{credential_id}` is not a compatible OAuth credential");
        }
        if let Some(account_label) = tokens.account_label.clone() {
            credential.account_label = Some(account_label);
        }
        self.secrets.set_secret(
            &credential_oauth_account(provider_id, credential_id),
            &serde_json::to_string(&tokens)?,
        )?;
        self.save_file(&file).await
    }

    pub async fn start_chatgpt_device_login(&self) -> Result<ChatGptDeviceCode> {
        ChatGptOAuthClient::default().request_device_code().await
    }

    pub async fn complete_chatgpt_device_login(
        &self,
        device: &ChatGptDeviceCode,
    ) -> Result<ProviderSettingsResponse> {
        let tokens = ChatGptOAuthClient::default()
            .complete_device_code(device)
            .await?;
        self.save_oauth_credential(
            "openai",
            "chatgpt-1",
            "ChatGPT Pro",
            CredentialAuthMethod::ChatGptOAuth,
            tokens,
        )
        .await
    }

    pub async fn start_github_copilot_device_login(&self) -> Result<GitHubCopilotDeviceCode> {
        GitHubCopilotOAuthClient::default()
            .request_device_code()
            .await
    }

    pub async fn complete_github_copilot_device_login(
        &self,
        device: &GitHubCopilotDeviceCode,
    ) -> Result<ProviderSettingsResponse> {
        let tokens = GitHubCopilotOAuthClient::default()
            .complete_device_code(device)
            .await?;
        let response = self
            .save_oauth_credential(
                "github_copilot",
                "copilot-1",
                "GitHub Copilot",
                CredentialAuthMethod::GitHubCopilotOAuth,
                tokens,
            )
            .await?;
        match self
            .refresh_provider_model_options("github_copilot")
            .await?
        {
            true => self.load_provider_settings().await,
            false => Ok(response),
        }
    }

    async fn refresh_provider_model_options(&self, provider_id: &str) -> Result<bool> {
        let Some(provider) = provider_by_id(provider_id) else {
            return Ok(false);
        };
        if !provider.supports_model_discovery {
            return Ok(false);
        }

        let file = self.load_file().await?;
        let Some(config) = effective_provider_config(
            &file,
            provider_profile_by_id(provider_id)
                .with_context(|| format!("unknown provider `{provider_id}`"))?,
        ) else {
            return Ok(false);
        };
        let response = self
            .list_provider_models(ProviderModelListRequest {
                provider_id: provider_id.to_string(),
                base_url: normalized_or_default(&config.base_url, &provider.default_base_url),
                api_key: None,
                use_saved_api_key: false,
            })
            .await?;
        if response.status != ProviderModelListStatus::Success || response.models.is_empty() {
            return Ok(false);
        }

        let mut file = self.load_file().await?;
        let Some(profile) = provider_profile_by_id(provider_id) else {
            return Ok(false);
        };
        let config = effective_provider_config(&file, profile);
        let current_model = config
            .as_ref()
            .map(|config| normalized_or_default(&config.model, profile.default_model))
            .unwrap_or_else(|| profile.default_model.to_string());
        let discovered_model_ids = response
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        let next_model = if discovered_model_ids
            .iter()
            .any(|model_id| *model_id == current_model.as_str())
        {
            current_model
        } else if discovered_model_ids
            .iter()
            .any(|model_id| *model_id == profile.default_model)
        {
            profile.default_model.to_string()
        } else {
            response
                .models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or(current_model)
        };
        let next_base_url = config
            .as_ref()
            .map(|config| normalized_or_default(&config.base_url, profile.default_base_url))
            .unwrap_or_else(|| profile.default_base_url.to_string());
        if file.provider_id == provider_id {
            file.base_url = next_base_url.clone();
            file.model = next_model.clone();
        }
        file.provider_configs.insert(
            provider_id.to_string(),
            ProviderConfigSettings {
                base_url: next_base_url,
                model: next_model,
                model_options: response.models,
            },
        );
        self.save_file(&file).await?;
        Ok(true)
    }

    pub async fn load_runtime_settings(&self) -> Result<RuntimeSettingsResponse> {
        let file = self.load_file().await?;
        let web_search = self.web_search_settings_view(&file.web_search)?;
        Ok(RuntimeSettingsResponse {
            default_model: normalized_or_default(&file.runtime_default_model, &file.model),
            default_thinking_mode: file.runtime_default_thinking_mode,
            presets: file.runtime_presets,
            mcp_servers: file.mcp_servers,
            skill_roots: effective_skill_roots(&file.skill_roots),
            web_search,
        })
    }

    pub async fn save_runtime_settings(
        &self,
        request: RuntimeSettingsSaveRequest,
    ) -> Result<RuntimeSettingsResponse> {
        validate_runtime_settings(&request)?;
        let mut file = self.load_file().await?;
        file.runtime_default_model = normalized_or_default(&request.default_model, &file.model);
        file.runtime_default_thinking_mode = request.default_thinking_mode;
        file.runtime_presets = request.presets;
        file.mcp_servers = request.mcp_servers;
        file.skill_roots = request.skill_roots;
        let web_search = normalized_web_search_settings(&request.web_search)?;
        let new_api_key = request
            .web_search
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let has_existing_api_key = if request.web_search.clear_api_key {
            web_search_env_api_key(&web_search.provider).is_some()
        } else {
            self.effective_web_search_api_key(&web_search.provider)?
                .is_some()
        };
        if web_search.enabled && new_api_key.is_none() && !has_existing_api_key {
            anyhow::bail!("Web search API key is required when enabled");
        }
        if request.web_search.clear_api_key {
            self.secrets
                .delete_secret(&web_search_api_key_account(&web_search.provider))?;
        }
        if let Some(api_key) = new_api_key {
            self.secrets
                .set_secret(&web_search_api_key_account(&web_search.provider), api_key)?;
        }
        file.web_search = web_search;
        self.save_file(&file).await?;
        self.load_runtime_settings().await
    }

    pub async fn scan_skill_catalog(
        &self,
        workspace_root: Option<PathBuf>,
    ) -> Result<SkillCatalogScanResponse> {
        let file = self.load_file().await?;
        let skill_config = SkillConfig::default();
        let mut sources = Vec::new();
        let mut skills = Vec::new();
        let mut warnings = Vec::new();
        let mut project_paths_by_name = HashMap::new();
        let mut active_paths_by_name = HashMap::new();

        if let Some(workspace_root) = workspace_root {
            let project_root = workspace_root.join(".agents").join("skills");
            sources.push(SkillSourceView {
                id: "project".to_string(),
                name: "Project skills".to_string(),
                scope: "project".to_string(),
                enabled: true,
                path: project_root.display().to_string(),
                status: skill_source_status(true, &project_root).to_string(),
                skill_count: 0,
                warning_count: 0,
            });

            let catalog = load_skills(&workspace_root, &[], &skill_config);
            let skill_count = catalog.skills.len();
            let warning_count = catalog.warnings.len();
            for warning in &catalog.warnings {
                warnings.push(skill_warning_view(warning, None));
            }
            for skill in catalog.skills {
                project_paths_by_name.insert(skill.name.clone(), skill.path.clone());
                active_paths_by_name.insert(skill.name.clone(), skill.path.clone());
                skills.push(skill_catalog_item_view(&skill, "project", "project"));
            }
            if let Some(source) = sources.last_mut() {
                source.skill_count = skill_count;
                source.warning_count = warning_count;
            }
        }

        let empty_workspace_root = self.path.join("__exagent_empty_skill_catalog_workspace__");
        let skill_roots = effective_skill_roots(&file.skill_roots);
        for (index, root) in skill_roots.iter().enumerate() {
            let source_id = skill_source_id(root, index);
            let source_scope = skill_source_scope(&root.scope);
            let root_path = PathBuf::from(root.path.trim());
            let source_status = skill_source_status(root.enabled, &root_path);
            sources.push(SkillSourceView {
                id: source_id.clone(),
                name: normalized_or_default(&root.name, &source_id),
                scope: source_scope.clone(),
                enabled: root.enabled,
                path: root.path.trim().to_string(),
                status: source_status.to_string(),
                skill_count: 0,
                warning_count: 0,
            });

            if !root.enabled
                || source_status != "ready"
                || !is_runtime_skill_root_scope(&source_scope)
            {
                continue;
            }

            let catalog = load_skills(
                &empty_workspace_root,
                std::slice::from_ref(&root_path),
                &skill_config,
            );
            let skill_count = catalog.skills.len();
            let mut warning_count = catalog.warnings.len();
            for warning in &catalog.warnings {
                warnings.push(skill_warning_view(warning, Some(&source_scope)));
            }
            for skill in catalog.skills {
                let mut item = skill_catalog_item_view(&skill, &source_id, &source_scope);
                if let Some(project_path) = project_paths_by_name.get(&skill.name) {
                    item.status = "shadowed".to_string();
                    item.effective_implicit = false;
                    warning_count = warning_count.saturating_add(1);
                    warnings.push(duplicate_skill_warning_view(
                        &source_scope,
                        &skill.name,
                        &[project_path.clone(), skill.path.clone()],
                    ));
                } else if let Some(existing_path) = active_paths_by_name.get(&skill.name) {
                    item.status = "shadowed".to_string();
                    item.effective_implicit = false;
                    warning_count = warning_count.saturating_add(1);
                    warnings.push(duplicate_skill_warning_view(
                        &source_scope,
                        &skill.name,
                        &[existing_path.clone(), skill.path.clone()],
                    ));
                } else {
                    active_paths_by_name.insert(skill.name.clone(), skill.path.clone());
                }
                skills.push(item);
            }
            if let Some(source) = sources.last_mut() {
                source.skill_count = skill_count;
                source.warning_count = warning_count;
            }
        }

        Ok(SkillCatalogScanResponse {
            sources,
            skills,
            warnings,
        })
    }

    pub async fn test_provider_connection(
        &self,
        request: ProviderConnectionTestRequest,
    ) -> Result<ProviderConnectionTestResponse> {
        let Some(provider) = provider_by_id(&request.provider_id) else {
            return Ok(connection_response(
                ProviderConnectionStatus::UnsupportedProvider,
                format!("Unsupported provider `{}`.", request.provider_id),
            ));
        };
        if !provider.supported {
            return Ok(connection_response(
                ProviderConnectionStatus::UnsupportedProvider,
                format!("{} support is not available yet.", provider.name),
            ));
        }

        let api_key = self.resolve_connection_api_key(&request).await?;
        if auth_required(provider.auth_mode) && api_key.is_none() {
            let response = connection_response(
                ProviderConnectionStatus::MissingCredential,
                format!("{} requires an API key.", provider.name),
            );
            self.persist_connection_status(&request.provider_id, &response)
                .await?;
            return Ok(response);
        }

        let response = match provider.protocol {
            ProviderProtocol::OpenAiChatCompletions => {
                test_openai_compatible_connection(
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    &normalized_or_default(&request.model, &provider.default_model),
                    api_key,
                )
                .await?
            }
            ProviderProtocol::AnthropicMessages => {
                test_anthropic_messages_connection(
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    &normalized_or_default(&request.model, &provider.default_model),
                    api_key.expect("auth_required should ensure Anthropic API key"),
                )
                .await?
            }
            ProviderProtocol::GeminiGenerateContent => {
                test_gemini_generate_content_connection(
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    &normalized_or_default(&request.model, &provider.default_model),
                    api_key.expect("auth_required should ensure Gemini API key"),
                )
                .await?
            }
            _ => connection_response(
                ProviderConnectionStatus::UnsupportedProvider,
                format!("{} support is not available yet.", provider.name),
            ),
        };
        self.persist_connection_status(&request.provider_id, &response)
            .await?;
        Ok(response)
    }

    pub async fn list_provider_models(
        &self,
        request: ProviderModelListRequest,
    ) -> Result<ProviderModelListResponse> {
        let Some(provider) = provider_by_id(&request.provider_id) else {
            return Ok(model_list_response(
                ProviderModelListStatus::UnsupportedProvider,
                format!("Unsupported provider `{}`.", request.provider_id),
                Vec::new(),
            ));
        };
        if !provider.supported {
            return Ok(model_list_response(
                ProviderModelListStatus::UnsupportedProvider,
                provider
                    .unsupported_reason
                    .clone()
                    .unwrap_or_else(|| format!("{} support is not available yet.", provider.name)),
                Vec::new(),
            ));
        }
        if !provider.supports_model_discovery {
            return Ok(model_list_response(
                ProviderModelListStatus::Unavailable,
                format!("{} does not expose model discovery yet.", provider.name),
                Vec::new(),
            ));
        }

        let oauth_bearer = if provider.protocol == ProviderProtocol::CopilotOAuth {
            let file = self.load_file().await?;
            self.saved_provider_oauth_tokens(&file, &provider.id)?
                .map(|(_, tokens)| tokens.access_token)
        } else {
            None
        };
        let api_key = if provider.protocol == ProviderProtocol::CopilotOAuth {
            None
        } else {
            self.resolve_provider_api_key(
                &request.provider_id,
                request.api_key.as_deref(),
                request.use_saved_api_key,
            )
            .await?
        };
        if provider.protocol == ProviderProtocol::CopilotOAuth && oauth_bearer.is_none() {
            return Ok(model_list_response(
                ProviderModelListStatus::MissingCredential,
                format!(
                    "{} requires GitHub Copilot OAuth to discover models.",
                    provider.name
                ),
                Vec::new(),
            ));
        }
        if auth_required(provider.auth_mode)
            && provider.protocol != ProviderProtocol::CopilotOAuth
            && api_key.is_none()
        {
            return Ok(model_list_response(
                ProviderModelListStatus::MissingCredential,
                format!("{} requires an API key to discover models.", provider.name),
                Vec::new(),
            ));
        }

        let mut response = match provider.protocol {
            ProviderProtocol::OpenAiChatCompletions => {
                list_openai_compatible_models(
                    &provider,
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    api_key,
                )
                .await
            }
            ProviderProtocol::AnthropicMessages => {
                list_anthropic_models(
                    &provider,
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    api_key.expect("auth_required should ensure Anthropic API key"),
                )
                .await
            }
            ProviderProtocol::GeminiGenerateContent => {
                list_gemini_models(
                    &provider,
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    api_key.expect("auth_required should ensure Gemini API key"),
                )
                .await
            }
            ProviderProtocol::CopilotOAuth => {
                list_github_copilot_models(
                    &provider,
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    oauth_bearer.expect("Copilot OAuth token should be resolved"),
                )
                .await
            }
        }?;
        if response.status == ProviderModelListStatus::Success {
            response.models = self
                .enrich_provider_models(&provider, response.models)
                .await;
        }
        Ok(response)
    }

    pub async fn runtime_config(&self) -> Result<AgentConfig> {
        let file = self.load_file().await?;
        let mut config = AgentConfig::default();
        config.web_search = None;
        let model_ref = ModelRef::new(file.provider_id.clone(), file.model);
        config.model = self.resolve(&model_ref).await?;
        extend_unique_paths(
            &mut config.skills_user_roots,
            runtime_skill_root_paths(&effective_skill_roots(&file.skill_roots)),
        );
        config.mcp_servers = file
            .mcp_servers
            .into_iter()
            .filter(|server| server.enabled)
            .map(|server| McpServerConfig {
                id: if server.id.trim().is_empty() {
                    McpServerConfig::normalized_id(&server.name)
                } else {
                    McpServerConfig::normalized_id(&server.id)
                },
                display_name: server.name.trim().to_string(),
                command: server.command.trim().to_string(),
                args: server
                    .args
                    .into_iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect(),
                env: server
                    .env
                    .into_iter()
                    .map(|(key, value)| (key.trim().to_string(), value))
                    .filter(|(key, _)| !key.is_empty())
                    .collect::<HashMap<_, _>>(),
                working_directory: server.working_directory.and_then(|value| {
                    let trimmed = value.trim();
                    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
                }),
            })
            .collect();
        if file.web_search.enabled {
            let web_search = normalized_stored_web_search_settings(&file.web_search)?;
            if let Some(api_key) = self.effective_web_search_api_key(&web_search.provider)? {
                config.web_search = Some(WebSearchConfig {
                    provider: web_search.provider,
                    api_key,
                });
            }
        }
        Ok(config)
    }

    fn web_search_settings_view(
        &self,
        settings: &StoredWebSearchSettings,
    ) -> Result<WebSearchSettings> {
        let settings = normalized_stored_web_search_settings(settings)?;
        let has_api_key = self
            .effective_web_search_api_key(&settings.provider)?
            .is_some();
        Ok(WebSearchSettings {
            enabled: settings.enabled,
            provider: settings.provider,
            has_api_key,
            api_key: None,
            clear_api_key: false,
        })
    }

    fn effective_web_search_api_key(&self, provider: &str) -> Result<Option<String>> {
        if let Some(api_key) = self
            .secrets
            .get_secret(&web_search_api_key_account(provider))?
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(api_key));
        }
        Ok(web_search_env_api_key(provider))
    }

    async fn resolve_connection_api_key(
        &self,
        request: &ProviderConnectionTestRequest,
    ) -> Result<Option<String>> {
        self.resolve_provider_api_key(
            &request.provider_id,
            request.api_key.as_deref(),
            request.use_saved_api_key,
        )
        .await
    }

    async fn resolve_provider_api_key(
        &self,
        provider_id: &str,
        api_key: Option<&str>,
        use_saved_api_key: bool,
    ) -> Result<Option<String>> {
        if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
            return Ok(Some(api_key.to_string()));
        }

        if use_saved_api_key {
            let file = self.load_file().await?;
            if let Some(api_key) = self.saved_provider_api_key(&file, provider_id)? {
                return Ok(Some(api_key));
            }
        }

        Ok(provider_env_api_key(provider_id).filter(|value| !value.trim().is_empty()))
    }

    async fn persist_connection_status(
        &self,
        provider_id: &str,
        response: &ProviderConnectionTestResponse,
    ) -> Result<()> {
        let mut file = self.load_file().await?;
        if file.provider_id != provider_id {
            return Ok(());
        }

        file.last_connection_status = Some(response.status);
        file.last_connection_message = Some(response.message.clone());
        file.last_connection_checked_at = Some(unix_timestamp_millis());
        self.save_file(&file).await
    }

    async fn load_file(&self) -> Result<SettingsFile> {
        if !tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            return Ok(SettingsFile::default());
        }
        let contents = tokio::fs::read_to_string(&self.path)
            .await
            .with_context(|| format!("failed to read settings at {}", self.path.display()))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse settings at {}", self.path.display()))
    }

    async fn save_file(&self, file: &SettingsFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let contents = serde_json::to_string_pretty(file)?;
        tokio::fs::write(&self.path, contents).await?;
        Ok(())
    }

    async fn enrich_provider_models(
        &self,
        provider: &ProviderDescriptor,
        models: Vec<ProviderModelView>,
    ) -> Vec<ProviderModelView> {
        let models_dev_catalog = if model_metadata::has_models_dev_provider_alias(&provider.id) {
            model_metadata::load_models_dev_catalog(&self.models_dev_cache_path()).await
        } else {
            None
        };

        models
            .into_iter()
            .map(|model| {
                let metadata = models_dev_catalog
                    .as_ref()
                    .and_then(|catalog| catalog.metadata_for(&provider.id, &model.id));
                enrich_provider_model(provider, model, metadata)
            })
            .collect()
    }

    async fn enrich_active_model_option_from_cache(&self, response: &mut ProviderSettingsResponse) {
        let Some(provider) = response
            .providers
            .iter()
            .find(|provider| provider.id == response.config.provider_id)
            .cloned()
        else {
            return;
        };
        if !model_metadata::has_models_dev_provider_alias(&provider.id) {
            return;
        }
        let models_dev_catalog =
            model_metadata::load_cached_models_dev_catalog(&self.models_dev_cache_path()).await;

        for model in response.model_options.iter_mut().filter(|model| {
            model.provider_id == response.config.provider_id && model.id == response.config.model
        }) {
            let metadata = models_dev_catalog
                .as_ref()
                .and_then(|catalog| catalog.metadata_for(&provider.id, &model.id));
            *model = enrich_provider_model(&provider, model.clone(), metadata);
        }
    }

    fn models_dev_cache_path(&self) -> PathBuf {
        self.path
            .parent()
            .map(|parent| parent.join("models-dev-cache.json"))
            .unwrap_or_else(|| PathBuf::from("models-dev-cache.json"))
    }

    fn response_from_file(
        &self,
        file: SettingsFile,
        active_credential: Option<ProviderCredentialView>,
        active_credential_id: Option<String>,
        credentials: Vec<ProviderCredentialView>,
    ) -> Result<ProviderSettingsResponse> {
        let providers = provider_catalog();
        let provider = provider_by_id(&file.provider_id).unwrap_or_else(|| providers[0].clone());
        let active_credential_kind = active_credential.as_ref().map(|credential| credential.kind);
        let credential_source = active_credential
            .as_ref()
            .map(|credential| credential.source)
            .unwrap_or(CredentialSource::None);
        let has_credential = active_credential.is_some();
        let has_api_key = active_credential_kind == Some(CredentialKind::ApiKey);
        let active_auth_required = auth_required(provider.auth_mode);
        let last_connection = connection_status_from_file(&file);
        let active_credential_summary = active_credential
            .as_ref()
            .map(ProviderCredentialSummary::from_credential)
            .unwrap_or_else(ProviderCredentialSummary::none);
        let active_model =
            supported_model_or_default(&provider, &file.model, active_credential_summary);
        let config = ProviderConfigView {
            provider_id: file.provider_id.clone(),
            base_url: file.base_url.clone(),
            model: active_model,
            has_api_key,
            has_credential,
            credential_kind: active_credential_kind,
            credential_source,
            auth_required: active_auth_required,
        };
        let mut configured_providers = Vec::new();
        let mut model_options = Vec::new();
        for available_provider in &providers {
            let Some(profile) = provider_profile_by_id(&available_provider.id) else {
                continue;
            };
            let Some(provider_config) = effective_provider_config(&file, profile) else {
                continue;
            };
            let credential_summary =
                self.provider_credential_summary(&file, &available_provider.id)?;
            if auth_required(available_provider.auth_mode) && !credential_summary.has_credential {
                continue;
            }
            let model = supported_model_or_default(
                available_provider,
                &provider_config.model,
                credential_summary,
            );
            configured_providers.push(ProviderConfigView {
                provider_id: available_provider.id.clone(),
                base_url: normalized_or_default(
                    &provider_config.base_url,
                    &available_provider.default_base_url,
                ),
                model: model.clone(),
                has_api_key: credential_summary.has_api_key,
                has_credential: credential_summary.has_credential,
                credential_kind: credential_summary.kind,
                credential_source: credential_summary.source,
                auth_required: auth_required(available_provider.auth_mode),
            });
            let catalog_scope = model_catalog_scope(available_provider, credential_summary);
            if provider_config.model_options.is_empty() {
                push_default_model_options(
                    &mut model_options,
                    available_provider,
                    &model,
                    catalog_scope,
                );
            } else {
                let before_provider_models = model_options.len();
                for model_option in provider_config.model_options {
                    if model_allowed_for_catalog_scope(catalog_scope, &model_option.id) {
                        push_model_option(&mut model_options, model_option);
                    }
                }
                let provider_model_was_added =
                    model_options[before_provider_models..]
                        .iter()
                        .any(|model_option| {
                            model_option.provider_id == available_provider.id
                                && model_option.id == model
                        });
                if !provider_model_was_added {
                    push_model_option(
                        &mut model_options,
                        provider_model_view(
                            available_provider,
                            &model,
                            &model,
                            catalog_context_window_for_model(&available_provider.id, &model),
                            None,
                        ),
                    );
                }
            }
        }
        let connected_provider = (provider.supported && (has_credential || !active_auth_required))
            .then(|| ConnectedProviderView {
                id: provider.id.clone(),
                name: provider.name.clone(),
                model: config.model.clone(),
                base_url: config.base_url.clone(),
            });

        Ok(ProviderSettingsResponse {
            providers,
            active_provider_id: config.provider_id.clone(),
            active_credential_id,
            credentials,
            config,
            connected_provider,
            last_connection,
            configured_providers,
            model_options,
        })
    }

    fn apply_credential_save_request(
        &self,
        file: &mut SettingsFile,
        request: &ProviderSettingsSaveRequest,
    ) -> Result<Option<String>> {
        let provider_id = file.provider_id.clone();
        let mut collection = file
            .provider_credentials
            .remove(&provider_id)
            .unwrap_or_default();
        let has_api_key = request
            .api_key
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        let requested_credential_id = request
            .credential_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let selected_credential_id = if let Some(credential_id) = requested_credential_id {
            Some(credential_id.to_string())
        } else if has_api_key || !collection.credentials.is_empty() {
            Some(DEFAULT_CREDENTIAL_ID.to_string())
        } else {
            None
        };

        if let Some(credential_id) = selected_credential_id.as_deref() {
            if has_api_key && !request.clear_api_key {
                collection.credentials.clear();
                ensure_credential_profile(&mut collection, credential_id);
            }
            collection.active_credential_id = Some(credential_id.to_string());
        }

        if request.clear_api_key {
            collection.credentials.clear();
            collection.active_credential_id = None;
        }

        if collection.credentials.is_empty() && collection.active_credential_id.is_none() {
            file.provider_credentials.remove(&provider_id);
        } else {
            file.provider_credentials.insert(provider_id, collection);
        }

        Ok(selected_credential_id)
    }

    fn provider_credentials_for_file(
        &self,
        file: &SettingsFile,
        provider_id: &str,
    ) -> Result<Vec<ProviderCredentialView>> {
        let collection = file
            .provider_credentials
            .get(provider_id)
            .cloned()
            .unwrap_or_default();
        let mut credentials = Vec::new();

        for credential in collection.credentials {
            match credential.kind {
                CredentialKind::ApiKey => {
                    if self
                        .credential_secret(provider_id, &credential.id)?
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty())
                    {
                        credentials.push(ProviderCredentialView {
                            id: credential.id,
                            label: credential.label,
                            source: CredentialSource::Keychain,
                            kind: CredentialKind::ApiKey,
                            status: CredentialStatus::Active,
                            auth_method: None,
                            account_label: None,
                        });
                    }
                }
                CredentialKind::OAuth => {
                    if let Some(tokens) = self.oauth_token_bundle(provider_id, &credential.id)? {
                        credentials.push(ProviderCredentialView {
                            id: credential.id,
                            label: credential.label,
                            source: CredentialSource::Keychain,
                            kind: CredentialKind::OAuth,
                            status: oauth_credential_status(&tokens),
                            auth_method: credential.auth_method,
                            account_label: credential.account_label.or(tokens.account_label),
                        });
                    }
                }
            }
        }

        if credentials.is_empty()
            && self
                .secrets
                .get_secret(&legacy_provider_api_key_account(provider_id))?
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        {
            credentials.push(ProviderCredentialView {
                id: DEFAULT_CREDENTIAL_ID.to_string(),
                label: "API key 1".to_string(),
                source: CredentialSource::Keychain,
                kind: CredentialKind::ApiKey,
                status: CredentialStatus::Active,
                auth_method: None,
                account_label: None,
            });
        }

        if !credentials
            .iter()
            .any(|credential| credential.id == DEFAULT_CREDENTIAL_ID)
            && provider_env_api_key(provider_id)
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        {
            credentials.insert(
                0,
                ProviderCredentialView {
                    id: DEFAULT_CREDENTIAL_ID.to_string(),
                    label: "API key 1".to_string(),
                    source: CredentialSource::Environment,
                    kind: CredentialKind::ApiKey,
                    status: CredentialStatus::Active,
                    auth_method: None,
                    account_label: None,
                },
            );
        }

        Ok(credentials)
    }

    fn credential_secret(&self, provider_id: &str, credential_id: &str) -> Result<Option<String>> {
        let secret = self
            .secrets
            .get_secret(&credential_api_key_account(provider_id, credential_id))?
            .filter(|value| !value.trim().is_empty());
        if secret.is_some() {
            return Ok(secret);
        }
        if credential_id == DEFAULT_CREDENTIAL_ID {
            return self
                .secrets
                .get_secret(&legacy_provider_api_key_account(provider_id))
                .map(|value| value.filter(|secret| !secret.trim().is_empty()));
        }
        Ok(None)
    }

    fn oauth_token_bundle(
        &self,
        provider_id: &str,
        credential_id: &str,
    ) -> Result<Option<OAuthTokenBundle>> {
        let Some(secret) = self
            .secrets
            .get_secret(&credential_oauth_account(provider_id, credential_id))?
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };

        serde_json::from_str(&secret)
            .map(Some)
            .with_context(|| format!("failed to decode OAuth credential `{credential_id}`"))
    }

    fn delete_credential_secrets(&self, provider_id: &str, credential_id: &str) -> Result<()> {
        self.secrets
            .delete_secret(&credential_api_key_account(provider_id, credential_id))?;
        self.secrets
            .delete_secret(&credential_oauth_account(provider_id, credential_id))?;
        if credential_id == DEFAULT_CREDENTIAL_ID {
            self.secrets
                .delete_secret(&legacy_provider_api_key_account(provider_id))?;
        }
        Ok(())
    }

    fn saved_provider_api_key(
        &self,
        file: &SettingsFile,
        provider_id: &str,
    ) -> Result<Option<String>> {
        if let Some(secret) = self
            .secrets
            .get_secret(&legacy_provider_api_key_account(provider_id))?
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(Some(secret));
        }
        if let Some(credential_id) = active_credential_id_for_file(file, provider_id) {
            if let Some(secret) = self.credential_secret(provider_id, &credential_id)? {
                return Ok(Some(secret));
            }
        }
        self.credential_secret(provider_id, DEFAULT_CREDENTIAL_ID)
    }

    fn saved_provider_oauth_tokens(
        &self,
        file: &SettingsFile,
        provider_id: &str,
    ) -> Result<Option<(String, OAuthTokenBundle)>> {
        let Some(collection) = file.provider_credentials.get(provider_id) else {
            return Ok(None);
        };
        let Some(active_credential_id) = collection.active_credential_id.as_deref() else {
            return Ok(None);
        };
        let Some(credential) = collection
            .credentials
            .iter()
            .find(|credential| credential.id == active_credential_id)
        else {
            return Ok(None);
        };
        if credential.kind != CredentialKind::OAuth {
            return Ok(None);
        }
        self.oauth_token_bundle(provider_id, active_credential_id)
            .map(|tokens| tokens.map(|tokens| (active_credential_id.to_string(), tokens)))
    }

    fn provider_credential_summary(
        &self,
        file: &SettingsFile,
        provider_id: &str,
    ) -> Result<ProviderCredentialSummary> {
        let credentials = self.provider_credentials_for_file(file, provider_id)?;
        let active_credential_id = active_credential_id_for_file(file, provider_id)
            .filter(|credential_id| {
                credentials
                    .iter()
                    .any(|credential| credential.id == *credential_id)
            })
            .or_else(|| credentials.first().map(|credential| credential.id.clone()));
        Ok(active_credential_id
            .as_deref()
            .and_then(|credential_id| credentials.iter().find(|item| item.id == credential_id))
            .map(ProviderCredentialSummary::from_credential)
            .unwrap_or_else(ProviderCredentialSummary::none))
    }
}

fn effective_provider_config(
    file: &SettingsFile,
    profile: &ProviderProfile,
) -> Option<ProviderConfigSettings> {
    file.provider_configs
        .get(profile.id)
        .cloned()
        .or_else(|| {
            (file.provider_id == profile.id).then(|| ProviderConfigSettings {
                base_url: normalized_or_default(&file.base_url, profile.default_base_url),
                model: normalized_or_default(&file.model, profile.default_model),
                model_options: Vec::new(),
            })
        })
        .or_else(|| {
            provider_credentials_referenced(file, profile.id).then(|| ProviderConfigSettings {
                base_url: profile.default_base_url.to_string(),
                model: profile.default_model.to_string(),
                model_options: Vec::new(),
            })
        })
}

fn preserve_active_provider_config(file: &mut SettingsFile) {
    if file.provider_configs.contains_key(&file.provider_id) {
        return;
    }
    let Some(profile) = provider_profile_by_id(&file.provider_id) else {
        return;
    };
    file.provider_configs.insert(
        file.provider_id.clone(),
        ProviderConfigSettings {
            base_url: normalized_or_default(&file.base_url, profile.default_base_url),
            model: normalized_or_default(&file.model, profile.default_model),
            model_options: Vec::new(),
        },
    );
}

fn provider_credentials_referenced(file: &SettingsFile, provider_id: &str) -> bool {
    file.provider_credentials
        .get(provider_id)
        .is_some_and(|collection| {
            collection.active_credential_id.is_some() || !collection.credentials.is_empty()
        })
}

fn should_refresh_provider_model_options_on_load(file: &SettingsFile, provider_id: &str) -> bool {
    if file.provider_id != provider_id || !provider_credentials_referenced(file, provider_id) {
        return false;
    }
    let Some(profile) = provider_profile_by_id(provider_id) else {
        return false;
    };
    let Some(config) = effective_provider_config(file, profile) else {
        return false;
    };
    let provider_models = config
        .model_options
        .iter()
        .filter(|model| model.provider_id == provider_id)
        .collect::<Vec<_>>();
    let current_model = normalized_or_default(&config.model, profile.default_model);
    provider_models.is_empty()
        || !provider_models
            .iter()
            .any(|model| model.id == current_model)
        || provider_models
            .iter()
            .all(|model| model.id == profile.default_model)
}

#[async_trait]
impl ModelResolver for DesktopSettingsStore {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig> {
        let profile = provider_profile_by_id(&model_ref.provider_id)
            .with_context(|| format!("unknown provider `{}`", model_ref.provider_id))?;
        let file = self.load_file().await?;
        let provider_config = effective_provider_config(&file, profile);
        let oauth_tokens = self.saved_provider_oauth_tokens(&file, profile.id)?;
        let requested_model_id = normalized_or_default(
            &model_ref.model_id,
            provider_config
                .as_ref()
                .map(|config| config.model.as_str())
                .unwrap_or(profile.default_model),
        );
        let model_id = if profile.id == "openai"
            && oauth_tokens.is_some()
            && !model_allowed_for_catalog_scope(
                ProviderModelCatalogScope::ChatGptCodex,
                &requested_model_id,
            ) {
            "gpt-5.5".to_string()
        } else {
            requested_model_id
        };
        let base_url = provider_config
            .as_ref()
            .map(|config| normalized_or_default(&config.base_url, profile.default_base_url))
            .or_else(|| {
                (!profile.default_base_url.is_empty()).then(|| profile.default_base_url.to_string())
            });
        let keychain_api_key = if oauth_tokens.is_none() {
            self.saved_provider_api_key(&file, profile.id)?
        } else {
            None
        };
        let mut resolved = resolve_from_profile(
            profile,
            &model_id,
            base_url,
            keychain_api_key.or_else(|| provider_env_api_key(profile.id)),
            model_context_window_from_env(),
        );
        if let Some((credential_id, tokens)) = oauth_tokens {
            resolved.credential = if profile.id == "openai" {
                exagent::resolved::ResolvedCredential::ChatGptOAuth {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                    expires_at_ms: tokens.expires_at_ms,
                    account_id: tokens.account_id,
                    credential_id: Some(credential_id),
                }
            } else {
                exagent::resolved::ResolvedCredential::BearerToken(tokens.access_token)
            };
        }
        let provider = provider_descriptor(profile);
        let models_dev_catalog = if model_metadata::has_models_dev_provider_alias(profile.id) {
            model_metadata::load_cached_models_dev_catalog(&self.models_dev_cache_path()).await
        } else {
            None
        };
        let metadata = models_dev_catalog
            .as_ref()
            .and_then(|catalog| catalog.metadata_for(profile.id, &model_id));
        let capabilities = capabilities_with_metadata(
            &provider,
            &provider_model_view(
                &provider,
                &model_id,
                &model_id,
                resolved.capabilities.context_window,
                metadata.and_then(|metadata| metadata.supports_tools),
            ),
            metadata,
        );
        resolved.capabilities.supports_tools = capabilities.supports_tools;
        resolved.capabilities.reasoning = capabilities
            .reasoning
            .map(Into::into)
            .unwrap_or_else(ReasoningCapabilities::unsupported);
        resolved.capabilities.context_window = metadata
            .and_then(|metadata| metadata.context_window)
            .or(resolved.capabilities.context_window)
            .or_else(|| catalog_context_window_for_model(profile.id, &model_id));
        Ok(resolved)
    }
}

#[async_trait]
impl ChatGptCodexTokenRefreshSink for DesktopSettingsStore {
    async fn save_chatgpt_codex_tokens(&self, update: ChatGptCodexTokenUpdate) -> Result<()> {
        let credential_id = update
            .credential_id
            .as_deref()
            .unwrap_or(DEFAULT_CREDENTIAL_ID);
        self.save_refreshed_oauth_credential(
            "openai",
            credential_id,
            CredentialAuthMethod::ChatGptOAuth,
            OAuthTokenBundle {
                access_token: update.access_token,
                refresh_token: update.refresh_token,
                expires_at_ms: update.expires_at_ms,
                account_id: update.account_id,
                account_label: None,
                raw_id_token: None,
            },
        )
        .await
    }
}

pub fn provider_catalog() -> Vec<ProviderDescriptor> {
    provider_profiles()
        .iter()
        .map(provider_descriptor)
        .collect()
}

fn provider_descriptor(profile: &ProviderProfile) -> ProviderDescriptor {
    ProviderDescriptor {
        id: profile.id.to_string(),
        name: profile.name.to_string(),
        description: profile.description.to_string(),
        recommended: profile.recommended,
        supported: profile.supported,
        auth_mode: profile.auth_mode,
        protocol: profile.protocol,
        default_base_url: profile.default_base_url.to_string(),
        default_model: profile.default_model.to_string(),
        supports_model_discovery: profile.supports_model_discovery,
        supports_tools: profile.supports_tools,
        unsupported_reason: profile.unsupported_reason.map(str::to_string),
    }
}

fn provider_model_view(
    provider: &ProviderDescriptor,
    model_id: &str,
    display_name: &str,
    context_window: Option<i64>,
    supports_tools: Option<bool>,
) -> ProviderModelView {
    ProviderModelView {
        provider_id: provider.id.clone(),
        id: model_id.to_string(),
        display_name: display_name.to_string(),
        context_window,
        supports_tools,
        capabilities: capabilities_for_model(
            &provider.id,
            model_id,
            provider.supports_tools,
            supports_tools,
        ),
    }
}

fn push_default_model_options(
    model_options: &mut Vec<ProviderModelView>,
    provider: &ProviderDescriptor,
    selected_model: &str,
    catalog_scope: ProviderModelCatalogScope,
) {
    let mut saw_catalog_model = false;
    let mut saw_selected_model = false;
    for catalog_model in catalog_models_for_provider(&provider.id) {
        if !model_allowed_for_catalog_scope(catalog_scope, catalog_model.id) {
            continue;
        }
        saw_catalog_model = true;
        saw_selected_model |= catalog_model.id == selected_model;
        push_model_option(
            model_options,
            provider_model_view(
                provider,
                catalog_model.id,
                catalog_model.display_name,
                catalog_model.context_window,
                None,
            ),
        );
    }

    if !saw_catalog_model || !saw_selected_model {
        push_model_option(
            model_options,
            provider_model_view(
                provider,
                selected_model,
                selected_model,
                catalog_context_window_for_model(&provider.id, selected_model),
                None,
            ),
        );
    }
}

fn supported_model_or_default(
    provider: &ProviderDescriptor,
    model: &str,
    credential_summary: ProviderCredentialSummary,
) -> String {
    let normalized = normalized_or_default(model, &provider.default_model);
    let catalog_scope = model_catalog_scope(provider, credential_summary);
    if model_allowed_for_catalog_scope(catalog_scope, &normalized) {
        normalized
    } else {
        default_model_for_catalog_scope(provider, catalog_scope).to_string()
    }
}

fn model_catalog_scope(
    provider: &ProviderDescriptor,
    credential_summary: ProviderCredentialSummary,
) -> ProviderModelCatalogScope {
    if provider.id == "openai" && credential_summary.kind == Some(CredentialKind::OAuth) {
        ProviderModelCatalogScope::ChatGptCodex
    } else {
        ProviderModelCatalogScope::ProviderApi
    }
}

fn default_model_for_catalog_scope(
    provider: &ProviderDescriptor,
    catalog_scope: ProviderModelCatalogScope,
) -> &str {
    match catalog_scope {
        ProviderModelCatalogScope::ChatGptCodex => "gpt-5.5",
        ProviderModelCatalogScope::ProviderApi => &provider.default_model,
    }
}

fn model_allowed_for_catalog_scope(
    catalog_scope: ProviderModelCatalogScope,
    model_id: &str,
) -> bool {
    match catalog_scope {
        ProviderModelCatalogScope::ProviderApi => true,
        ProviderModelCatalogScope::ChatGptCodex => {
            matches!(model_id, "gpt-5.5" | "gpt-5.4" | "gpt-5.4-mini")
        }
    }
}

fn enrich_provider_model(
    provider: &ProviderDescriptor,
    mut model: ProviderModelView,
    metadata: Option<&ModelsDevModelMetadata>,
) -> ProviderModelView {
    model.context_window = metadata
        .and_then(|metadata| metadata.context_window)
        .or(model.context_window)
        .or_else(|| catalog_context_window_for_model(&provider.id, &model.id));

    if model.supports_tools.is_none() {
        model.supports_tools = metadata.and_then(|metadata| metadata.supports_tools);
    }

    model.capabilities = capabilities_with_metadata(provider, &model, metadata);
    model
}

fn catalog_context_window_for_model(provider_id: &str, model_id: &str) -> Option<i64> {
    catalog_models_for_provider(provider_id)
        .find(|model| model.id == model_id)
        .and_then(|model| model.context_window)
}

fn capabilities_with_metadata(
    provider: &ProviderDescriptor,
    model: &ProviderModelView,
    metadata: Option<&ModelsDevModelMetadata>,
) -> ModelCapabilities {
    let mut capabilities = capabilities_for_model(
        &provider.id,
        &model.id,
        provider.supports_tools,
        model.supports_tools,
    );

    let catalog_has_model =
        catalog_models_for_provider(&provider.id).any(|entry| entry.id == model.id);
    if !catalog_has_model && capabilities.reasoning.is_none() {
        capabilities.reasoning = provider_reasoning_for_catalog_model(provider, &model.id);
    }

    if let Some(reasoning) = metadata.and_then(|metadata| metadata.reasoning) {
        let can_enable_reasoning_from_metadata =
            !catalog_has_model || capabilities.reasoning.is_some();
        if reasoning && can_enable_reasoning_from_metadata {
            if capabilities.reasoning.is_none() {
                capabilities.reasoning = provider_reasoning_for_catalog_model(provider, &model.id);
            }
            if !capabilities.thinking.supported {
                capabilities.thinking = capabilities
                    .reasoning
                    .as_ref()
                    .filter(|reasoning| !reasoning.supported_modes.is_empty())
                    .map(|reasoning| ThinkingCapability {
                        supported: true,
                        modes: reasoning.supported_modes.clone(),
                    })
                    .unwrap_or_else(|| ThinkingCapability {
                        supported: true,
                        modes: vec![ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::High],
                    });
            }
        } else if !reasoning {
            capabilities.thinking = ThinkingCapability::default();
            capabilities.reasoning = None;
        }
    }

    capabilities
}

fn provider_reasoning_for_catalog_model(
    provider: &ProviderDescriptor,
    model_id: &str,
) -> Option<CatalogReasoning> {
    provider_profile_by_id(&provider.id).and_then(|profile| {
        CatalogReasoning::from_capabilities(&profile.reasoning_capabilities_for_model(model_id))
    })
}

fn push_model_option(options: &mut Vec<ProviderModelView>, option: ProviderModelView) {
    if options
        .iter()
        .any(|existing| existing.provider_id == option.provider_id && existing.id == option.id)
    {
        return;
    }
    options.push(option);
}

fn provider_by_id(provider_id: &str) -> Option<ProviderDescriptor> {
    provider_profile_by_id(provider_id).map(provider_descriptor)
}

fn default_provider_profile() -> &'static ProviderProfile {
    provider_profile_by_id(DEFAULT_PROVIDER_ID).expect("default provider profile must exist")
}

fn default_runtime_model() -> String {
    default_provider_profile().default_model.to_string()
}

fn default_web_search_provider() -> String {
    DEFAULT_WEB_SEARCH_PROVIDER.to_string()
}

fn effective_skill_roots(roots: &[SkillRootSettings]) -> Vec<SkillRootSettings> {
    if roots.is_empty() {
        return default_desktop_skill_roots();
    }
    roots.to_vec()
}

fn default_desktop_skill_roots() -> Vec<SkillRootSettings> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    let candidates = [
        (
            "user-skills",
            "User skills",
            home.join(".agents").join("skills"),
        ),
        (
            "codex-skills",
            "Codex skills",
            home.join(".codex").join("skills"),
        ),
    ];

    candidates
        .into_iter()
        .filter_map(|(id, name, path)| {
            path.is_dir().then(|| SkillRootSettings {
                id: id.to_string(),
                name: name.to_string(),
                enabled: true,
                path: path.display().to_string(),
                scope: "global".to_string(),
            })
        })
        .collect()
}

fn active_credential_id_for_file(file: &SettingsFile, provider_id: &str) -> Option<String> {
    file.provider_credentials
        .get(provider_id)
        .and_then(|collection| collection.active_credential_id.clone())
}

fn ensure_credential_profile(collection: &mut ProviderCredentialCollection, credential_id: &str) {
    if collection
        .credentials
        .iter()
        .any(|credential| credential.id == credential_id)
    {
        return;
    }
    collection.credentials.push(ProviderCredentialSettings {
        id: credential_id.to_string(),
        label: credential_label(credential_id, collection.credentials.len() + 1),
        kind: CredentialKind::ApiKey,
        auth_method: None,
        account_label: None,
    });
}

fn credential_label(credential_id: &str, fallback_index: usize) -> String {
    credential_id
        .strip_prefix("key-")
        .and_then(|value| value.parse::<usize>().ok())
        .map(|index| format!("API key {index}"))
        .unwrap_or_else(|| format!("API key {fallback_index}"))
}

fn provider_env_api_key(provider_id: &str) -> Option<String> {
    exagent::resolver::provider_env_api_key(provider_id)
}

fn web_search_api_key_account(provider: &str) -> String {
    format!(
        "web_search:{}:api_key",
        provider.trim().to_ascii_lowercase()
    )
}

fn web_search_env_api_key(provider: &str) -> Option<String> {
    (normalize_web_search_provider(provider).ok()? == DEFAULT_WEB_SEARCH_PROVIDER).then(|| {
        std::env::var("EXAGENT_WEB_SEARCH_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })?
}

fn normalize_web_search_provider(provider: &str) -> Result<String> {
    let normalized =
        normalized_or_default(provider, DEFAULT_WEB_SEARCH_PROVIDER).to_ascii_lowercase();
    if normalized != DEFAULT_WEB_SEARCH_PROVIDER {
        anyhow::bail!("Unsupported web search provider `{normalized}`");
    }
    Ok(normalized)
}

fn normalized_web_search_settings(settings: &WebSearchSettings) -> Result<StoredWebSearchSettings> {
    Ok(StoredWebSearchSettings {
        enabled: settings.enabled,
        provider: normalize_web_search_provider(&settings.provider)?,
    })
}

fn normalized_stored_web_search_settings(
    settings: &StoredWebSearchSettings,
) -> Result<StoredWebSearchSettings> {
    Ok(StoredWebSearchSettings {
        enabled: settings.enabled,
        provider: normalize_web_search_provider(&settings.provider)?,
    })
}

fn normalized_or_default(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn runtime_skill_root_paths(roots: &[SkillRootSettings]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| root.enabled && is_runtime_skill_root_scope(&root.scope))
        .filter_map(|root| {
            let trimmed = root.path.trim();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        })
        .collect()
}

fn extend_unique_paths(target: &mut Vec<PathBuf>, paths: impl IntoIterator<Item = PathBuf>) {
    for path in paths {
        if !target.contains(&path) {
            target.push(path);
        }
    }
}

fn is_runtime_skill_root_scope(scope: &str) -> bool {
    matches!(
        scope.trim().to_ascii_lowercase().as_str(),
        "global" | "user"
    )
}

fn skill_source_id(root: &SkillRootSettings, index: usize) -> String {
    let trimmed = root.id.trim();
    if trimmed.is_empty() {
        format!("skill-root-{index}")
    } else {
        trimmed.to_string()
    }
}

fn skill_source_scope(scope: &str) -> String {
    scope.trim().to_ascii_lowercase()
}

fn skill_source_status(enabled: bool, path: &Path) -> &'static str {
    if !enabled {
        return "disabled";
    }
    if path.as_os_str().is_empty() || !path.is_dir() {
        return "missing";
    }
    "ready"
}

fn skill_catalog_item_view(
    skill: &SkillMetadata,
    source_id: &str,
    scope: &str,
) -> SkillCatalogItemView {
    SkillCatalogItemView {
        name: skill.name.clone(),
        scope: scope.to_string(),
        description: skill.description.clone(),
        path: skill.path.display().to_string(),
        source_id: source_id.to_string(),
        allow_implicit_invocation: skill.allow_implicit_invocation,
        effective_implicit: skill.allow_implicit_invocation,
        status: if skill.allow_implicit_invocation {
            "active"
        } else {
            "explicit_only"
        }
        .to_string(),
    }
}

fn skill_warning_view(
    warning: &SkillWarning,
    scope_override: Option<&str>,
) -> SkillCatalogWarningView {
    SkillCatalogWarningView {
        kind: skill_warning_kind_view(&warning.kind).to_string(),
        scope: scope_override
            .map(str::to_string)
            .unwrap_or_else(|| skill_scope_view(warning.scope).to_string()),
        name: warning.name.clone(),
        paths: warning
            .paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

fn duplicate_skill_warning_view(
    scope: &str,
    name: &str,
    paths: &[PathBuf],
) -> SkillCatalogWarningView {
    SkillCatalogWarningView {
        kind: "duplicate_name".to_string(),
        scope: scope.to_string(),
        name: name.to_string(),
        paths: paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

fn skill_warning_kind_view(kind: &SkillWarningKind) -> &'static str {
    match kind {
        SkillWarningKind::DuplicateName => "duplicate_name",
        SkillWarningKind::InvalidMetadata => "invalid_metadata",
        SkillWarningKind::ReadError => "read_error",
    }
}

fn skill_scope_view(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::Repo => "project",
        SkillScope::User => "user",
    }
}

fn auth_required(auth_mode: ProviderAuthMode) -> bool {
    matches!(
        auth_mode,
        ProviderAuthMode::ApiKeyRequired | ProviderAuthMode::OAuthRequired
    )
}

fn validate_runtime_settings(request: &RuntimeSettingsSaveRequest) -> Result<()> {
    normalize_web_search_provider(&request.web_search.provider)?;

    for preset in &request.presets {
        if preset.name.trim().is_empty() {
            anyhow::bail!("Runtime preset name is required");
        }
        if preset.model.trim().is_empty() {
            anyhow::bail!("Runtime preset model is required");
        }
    }

    let mut enabled_mcp_ids = HashSet::new();
    for server in &request.mcp_servers {
        if server.name.trim().is_empty() {
            anyhow::bail!("MCP server name is required");
        }
        if server.command.trim().is_empty() {
            anyhow::bail!("MCP command is required");
        }
        for (key, _) in &server.env {
            if key.trim().is_empty() {
                anyhow::bail!("MCP environment variable name is required");
            }
        }
        if server.enabled {
            let effective_id = if server.id.trim().is_empty() {
                &server.name
            } else {
                &server.id
            };
            let normalized_id = McpServerConfig::normalized_id(effective_id);
            if normalized_id.is_empty() {
                anyhow::bail!(
                    "MCP server id must contain an ASCII letter, number, underscore, or hyphen"
                );
            }
            if !enabled_mcp_ids.insert(normalized_id.clone()) {
                anyhow::bail!("Duplicate enabled MCP server id `{normalized_id}`");
            }
        }
    }

    for root in &request.skill_roots {
        if root.name.trim().is_empty() {
            anyhow::bail!("Skill root name is required");
        }
        if root.enabled && root.path.trim().is_empty() {
            anyhow::bail!("Enabled skill root path is required");
        }
        if root.scope.trim().is_empty() {
            anyhow::bail!("Skill root scope is required");
        }
    }

    Ok(())
}

async fn test_openai_compatible_connection(
    base_url: &str,
    model: &str,
    api_key: Option<String>,
) -> Result<ProviderConnectionTestResponse> {
    let request_body = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": "Respond with ok."
        }]
    });
    let mut request_builder = reqwest::Client::new()
        .post(chat_completions_endpoint(base_url))
        .json(&request_body);
    if let Some(api_key) = api_key.as_deref() {
        request_builder = request_builder.bearer_auth(api_key);
    }

    let response = match request_builder.send().await {
        Ok(response) => response,
        Err(error) => {
            return Ok(connection_response(
                ProviderConnectionStatus::NetworkError,
                format!("Failed to reach provider: {error}"),
            ));
        }
    };

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(connection_response(
            status_to_connection_status(status, &body),
            provider_error_message(status, &body),
        ));
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return Ok(connection_response(
                ProviderConnectionStatus::ProviderError,
                format!("Provider returned invalid JSON: {error}"),
            ));
        }
    };
    if let Err(error) = OpenAiCompatibleLlm::parse_response(value) {
        return Ok(connection_response(
            ProviderConnectionStatus::ProviderError,
            format!("Provider response was not OpenAI-compatible: {error}"),
        ));
    }

    Ok(connection_response(
        ProviderConnectionStatus::Success,
        "Connection succeeded.".to_string(),
    ))
}

async fn test_anthropic_messages_connection(
    base_url: &str,
    model: &str,
    api_key: String,
) -> Result<ProviderConnectionTestResponse> {
    let endpoint = if base_url.trim_end_matches('/').ends_with("/messages") {
        base_url.trim_end_matches('/').to_string()
    } else {
        format!("{}/messages", base_url.trim_end_matches('/'))
    };
    let client = reqwest::Client::new();
    let response = client
        .post(endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": model,
            "max_tokens": 8,
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": "Return a short connection test response." }]
            }]
        }))
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => Ok(connection_response(
            ProviderConnectionStatus::Success,
            "Connection succeeded.",
        )),
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let response_status =
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    ProviderConnectionStatus::AuthenticationFailed
                } else if status == StatusCode::NOT_FOUND {
                    ProviderConnectionStatus::ModelNotFound
                } else {
                    ProviderConnectionStatus::ProviderError
                };
            Ok(connection_response(
                response_status,
                provider_error_message(status, &body),
            ))
        }
        Err(error) => Ok(connection_response(
            ProviderConnectionStatus::NetworkError,
            format!("Failed to reach provider: {error}"),
        )),
    }
}

async fn test_gemini_generate_content_connection(
    base_url: &str,
    model: &str,
    api_key: String,
) -> Result<ProviderConnectionTestResponse> {
    let endpoint = if base_url.trim_end_matches('/').ends_with(":generateContent") {
        base_url.trim_end_matches('/').to_string()
    } else {
        format!(
            "{}/models/{model}:generateContent",
            base_url.trim_end_matches('/')
        )
    };
    let client = reqwest::Client::new();
    let response = client
        .post(endpoint)
        .header("x-goog-api-key", api_key)
        .json(&serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Return a short connection test response." }]
            }]
        }))
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => Ok(connection_response(
            ProviderConnectionStatus::Success,
            "Connection succeeded.",
        )),
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let response_status =
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    ProviderConnectionStatus::AuthenticationFailed
                } else if status == StatusCode::NOT_FOUND {
                    ProviderConnectionStatus::ModelNotFound
                } else {
                    ProviderConnectionStatus::ProviderError
                };
            Ok(connection_response(
                response_status,
                provider_error_message(status, &body),
            ))
        }
        Err(error) => Ok(connection_response(
            ProviderConnectionStatus::NetworkError,
            format!("Failed to reach provider: {error}"),
        )),
    }
}

async fn list_openai_compatible_models(
    provider: &ProviderDescriptor,
    base_url: &str,
    api_key: Option<String>,
) -> Result<ProviderModelListResponse> {
    let mut request_builder = reqwest::Client::new().get(models_endpoint(base_url));
    if let Some(api_key) = api_key.as_deref() {
        request_builder = request_builder.bearer_auth(api_key);
    }

    let response = match request_builder.send().await {
        Ok(response) => response,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::NetworkError,
                format!("Failed to reach provider models endpoint: {error}"),
                Vec::new(),
            ));
        }
    };

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(model_list_response(
            status_to_model_list_status(status),
            provider_error_message(status, &body),
            Vec::new(),
        ));
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned invalid JSON: {error}"),
                Vec::new(),
            ));
        }
    };
    let models = match parse_openai_model_list(provider, value) {
        Ok(models) => models,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned an invalid model list: {error}"),
                Vec::new(),
            ));
        }
    };

    Ok(model_list_response(
        ProviderModelListStatus::Success,
        "Model discovery succeeded.".to_string(),
        models,
    ))
}

async fn list_anthropic_models(
    provider: &ProviderDescriptor,
    base_url: &str,
    api_key: String,
) -> Result<ProviderModelListResponse> {
    let response = match reqwest::Client::new()
        .get(anthropic_models_endpoint(base_url))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::NetworkError,
                format!("Failed to reach {} models endpoint: {error}", provider.name),
                Vec::new(),
            ));
        }
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(model_list_response(
            status_to_model_list_status(status),
            provider_error_message(status, &body),
            Vec::new(),
        ));
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned invalid JSON: {error}"),
                Vec::new(),
            ));
        }
    };
    let models = match parse_anthropic_model_list(provider, value) {
        Ok(models) => models,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned an invalid model list: {error}"),
                Vec::new(),
            ));
        }
    };

    Ok(model_list_response(
        ProviderModelListStatus::Success,
        "Model discovery succeeded.".to_string(),
        models,
    ))
}

async fn list_gemini_models(
    provider: &ProviderDescriptor,
    base_url: &str,
    api_key: String,
) -> Result<ProviderModelListResponse> {
    let request_builder = reqwest::Client::new()
        .get(models_endpoint(base_url))
        .header("x-goog-api-key", api_key);

    let response = match request_builder.send().await {
        Ok(response) => response,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::NetworkError,
                format!("Failed to reach {} models endpoint: {error}", provider.name),
                Vec::new(),
            ));
        }
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(model_list_response(
            status_to_model_list_status(status),
            provider_error_message(status, &body),
            Vec::new(),
        ));
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned invalid JSON: {error}"),
                Vec::new(),
            ));
        }
    };
    let models = match parse_gemini_model_list(provider, value) {
        Ok(models) => models,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned an invalid model list: {error}"),
                Vec::new(),
            ));
        }
    };

    Ok(model_list_response(
        ProviderModelListStatus::Success,
        "Model discovery succeeded.".to_string(),
        models,
    ))
}

async fn list_github_copilot_models(
    provider: &ProviderDescriptor,
    base_url: &str,
    oauth_token: String,
) -> Result<ProviderModelListResponse> {
    let response = match reqwest::Client::new()
        .get(copilot_models_endpoint(base_url))
        .bearer_auth(oauth_token)
        .copilot_headers()
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::NetworkError,
                format!("Failed to reach {} models endpoint: {error}", provider.name),
                Vec::new(),
            ));
        }
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(model_list_response(
            status_to_model_list_status(status),
            provider_error_message(status, &body),
            Vec::new(),
        ));
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned invalid JSON: {error}"),
                Vec::new(),
            ));
        }
    };
    let models = match parse_copilot_model_list(provider, value) {
        Ok(models) => models,
        Err(error) => {
            return Ok(model_list_response(
                ProviderModelListStatus::ProviderError,
                format!("Provider returned an invalid model list: {error}"),
                Vec::new(),
            ));
        }
    };

    Ok(model_list_response(
        ProviderModelListStatus::Success,
        "Model discovery succeeded.".to_string(),
        models,
    ))
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/models") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/models")
    }
}

fn anthropic_models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/models") {
        trimmed.to_string()
    } else if let Some(root) = trimmed.strip_suffix("/messages") {
        format!("{root}/models")
    } else {
        format!("{trimmed}/models")
    }
}

fn parse_openai_model_list(
    provider: &ProviderDescriptor,
    value: Value,
) -> Result<Vec<ProviderModelView>> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .context("missing `data` array")?;
    let mut models = Vec::with_capacity(data.len());
    for model in data {
        let id = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("model entry is missing `id`")?;
        models.push(provider_model_view(provider, id, id, None, None));
    }
    Ok(models)
}

fn parse_anthropic_model_list(
    provider: &ProviderDescriptor,
    value: Value,
) -> Result<Vec<ProviderModelView>> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .context("missing `data` array")?;
    let mut models = Vec::with_capacity(data.len());
    for model in data {
        let id = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("model entry is missing `id`")?;
        let display_name = model
            .get("display_name")
            .or_else(|| model.get("displayName"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        models.push(provider_model_view(provider, id, display_name, None, None));
    }
    Ok(models)
}

fn parse_gemini_model_list(
    provider: &ProviderDescriptor,
    value: Value,
) -> Result<Vec<ProviderModelView>> {
    let data = value
        .get("models")
        .and_then(Value::as_array)
        .context("missing `models` array")?;
    let mut models = Vec::new();
    for model in data {
        let methods = model
            .get("supportedGenerationMethods")
            .and_then(Value::as_array)
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|method| method == "generateContent")
            })
            .unwrap_or(true);
        if !methods {
            continue;
        }
        let raw_id = model
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("model entry is missing `name`")?;
        let id = raw_id.strip_prefix("models/").unwrap_or(raw_id);
        let display_name = model
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        let context_window = model.get("inputTokenLimit").and_then(Value::as_i64);
        models.push(provider_model_view(
            provider,
            id,
            display_name,
            context_window,
            Some(true),
        ));
    }
    Ok(models)
}

fn parse_copilot_model_list(
    provider: &ProviderDescriptor,
    value: Value,
) -> Result<Vec<ProviderModelView>> {
    let data = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .context("missing `data` array")?;
    let mut models = Vec::with_capacity(data.len());
    for model in data {
        if model.get("model_picker_enabled").and_then(Value::as_bool) == Some(false)
            || model.get("available").and_then(Value::as_bool) == Some(false)
        {
            continue;
        }

        let id = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .context("model entry is missing `id`")?;
        let display_name = ["name", "display_name", "displayName", "label"]
            .iter()
            .filter_map(|key| model.get(key).and_then(Value::as_str))
            .map(str::trim)
            .find(|name| !name.is_empty())
            .unwrap_or(id);
        let context_window = first_nested_i64(
            model,
            &[
                &["capabilities", "limits", "max_context_window_tokens"],
                &["capabilities", "limits", "max_prompt_tokens"],
                &["limits", "max_context_window_tokens"],
                &["limits", "max_prompt_tokens"],
                &["context_window"],
            ],
        );
        let supports_tools = first_nested_bool(
            model,
            &[
                &["capabilities", "supports", "tool_calls"],
                &["supports", "tool_calls"],
                &["tool_calls"],
            ],
        )
        .or(Some(provider.supports_tools));
        models.push(provider_model_view(
            provider,
            id,
            display_name,
            context_window,
            supports_tools,
        ));
    }
    Ok(models)
}

fn first_nested_i64(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    paths
        .iter()
        .find_map(|path| nested_value(value, path)?.as_i64())
}

fn first_nested_bool(value: &Value, paths: &[&[&str]]) -> Option<bool> {
    paths
        .iter()
        .find_map(|path| nested_value(value, path)?.as_bool())
}

fn nested_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn status_to_connection_status(status: StatusCode, body: &str) -> ProviderConnectionStatus {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        ProviderConnectionStatus::AuthenticationFailed
    } else if looks_like_model_error(body) {
        ProviderConnectionStatus::ModelNotFound
    } else {
        ProviderConnectionStatus::ProviderError
    }
}

fn status_to_model_list_status(status: StatusCode) -> ProviderModelListStatus {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        ProviderModelListStatus::AuthenticationFailed
    } else if matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) {
        ProviderModelListStatus::Unavailable
    } else {
        ProviderModelListStatus::ProviderError
    }
}

fn looks_like_model_error(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("model")
        && (lower.contains("not found")
            || lower.contains("does not exist")
            || lower.contains("unknown"))
}

fn provider_error_message(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("Provider request failed with status {status}.")
    } else {
        format!("Provider request failed with status {status}: {body}")
    }
}

fn connection_response(
    status: ProviderConnectionStatus,
    message: impl Into<String>,
) -> ProviderConnectionTestResponse {
    ProviderConnectionTestResponse {
        status,
        message: message.into(),
    }
}

fn model_list_response(
    status: ProviderModelListStatus,
    message: impl Into<String>,
    models: Vec<ProviderModelView>,
) -> ProviderModelListResponse {
    ProviderModelListResponse {
        status,
        message: message.into(),
        models,
    }
}

fn connection_status_from_file(file: &SettingsFile) -> Option<ProviderConnectionStatusView> {
    Some(ProviderConnectionStatusView {
        status: file.last_connection_status?,
        message: file.last_connection_message.clone()?,
        checked_at: file.last_connection_checked_at.clone()?,
    })
}

fn unix_timestamp_millis() -> String {
    unix_timestamp_millis_value().to_string()
}

fn unix_timestamp_millis_value() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn oauth_credential_status(tokens: &OAuthTokenBundle) -> CredentialStatus {
    if tokens.is_expired_at(unix_timestamp_millis_value()) {
        CredentialStatus::Expired
    } else {
        CredentialStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopSecretStore;

    impl SecretStore for NoopSecretStore {
        fn get_secret(&self, _account: &str) -> Result<Option<String>> {
            Ok(None)
        }

        fn set_secret(&self, _account: &str, _secret: &str) -> Result<()> {
            Ok(())
        }

        fn delete_secret(&self, _account: &str) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemorySecretStore {
        values: std::sync::Mutex<HashMap<String, String>>,
    }

    impl SecretStore for MemorySecretStore {
        fn get_secret(&self, account: &str) -> Result<Option<String>> {
            Ok(self.values.lock().unwrap().get(account).cloned())
        }

        fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
            self.values
                .lock()
                .unwrap()
                .insert(account.to_string(), secret.to_string());
            Ok(())
        }

        fn delete_secret(&self, account: &str) -> Result<()> {
            self.values.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[test]
    fn file_secret_store_round_trips_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("auth.json"));

        assert_eq!(store.get_secret("provider:openai:api_key").unwrap(), None);

        store
            .set_secret("provider:openai:api_key", "sk-one")
            .unwrap();
        store
            .set_secret("provider:anthropic:api_key", "sk-two")
            .unwrap();

        assert_eq!(
            store
                .get_secret("provider:openai:api_key")
                .unwrap()
                .as_deref(),
            Some("sk-one")
        );
        assert_eq!(
            store
                .get_secret("provider:anthropic:api_key")
                .unwrap()
                .as_deref(),
            Some("sk-two")
        );

        store.delete_secret("provider:openai:api_key").unwrap();
        assert_eq!(store.get_secret("provider:openai:api_key").unwrap(), None);
        // Deleting one secret leaves the others intact.
        assert_eq!(
            store
                .get_secret("provider:anthropic:api_key")
                .unwrap()
                .as_deref(),
            Some("sk-two")
        );
        // Deleting a missing key is a no-op.
        store.delete_secret("provider:openai:api_key").unwrap();
    }

    #[test]
    fn file_secret_store_persists_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        FileSecretStore::new(path.clone())
            .set_secret("provider:openai:api_key", "sk-persist")
            .unwrap();

        let reopened = FileSecretStore::new(path);
        assert_eq!(
            reopened
                .get_secret("provider:openai:api_key")
                .unwrap()
                .as_deref(),
            Some("sk-persist")
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_secret_store_writes_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        FileSecretStore::new(path.clone())
            .set_secret("provider:openai:api_key", "sk-perm")
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    fn runtime_request_with_mcp_servers(
        mcp_servers: Vec<McpServerSettings>,
    ) -> RuntimeSettingsSaveRequest {
        RuntimeSettingsSaveRequest {
            default_model: "gpt-4.1-mini".into(),
            default_thinking_mode: None,
            presets: Vec::new(),
            mcp_servers,
            skill_roots: Vec::new(),
            web_search: WebSearchSettings::default(),
        }
    }

    fn mcp_server(id: &str, name: &str, enabled: bool) -> McpServerSettings {
        McpServerSettings {
            id: id.into(),
            name: name.into(),
            enabled,
            command: "node".into(),
            args: Vec::new(),
            env: Vec::new(),
            working_directory: None,
        }
    }

    fn provider_save_request(
        provider_id: &str,
        base_url: &str,
        model: &str,
        api_key: &str,
    ) -> ProviderSettingsSaveRequest {
        ProviderSettingsSaveRequest {
            provider_id: provider_id.into(),
            base_url: base_url.into(),
            model: model.into(),
            api_key: Some(api_key.into()),
            clear_api_key: false,
            credential_id: None,
            create_credential: false,
            model_options: Vec::new(),
        }
    }

    #[tokio::test]
    async fn provider_settings_keep_one_config_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let settings = DesktopSettingsStore::with_secret_store(
            dir.path().join("settings.json"),
            Arc::new(MemorySecretStore::default()),
        );

        settings
            .save_provider_settings(provider_save_request(
                "openai",
                "https://api.openai.com/v1",
                "gpt-5.5",
                "sk-openai",
            ))
            .await
            .unwrap();
        let response = settings
            .save_provider_settings(provider_save_request(
                "deepseek",
                "https://api.deepseek.com",
                "deepseek-v4-flash",
                "sk-deepseek",
            ))
            .await
            .unwrap();

        assert!(response
            .configured_providers
            .iter()
            .any(|provider| provider.provider_id == "openai" && provider.model == "gpt-5.5"));
        assert!(response.configured_providers.iter().any(|provider| {
            provider.provider_id == "deepseek" && provider.model == "deepseek-v4-flash"
        }));
        assert!(response
            .model_options
            .iter()
            .any(|model| model.provider_id == "openai" && model.id == "gpt-5.5"));
        assert!(response
            .model_options
            .iter()
            .any(|model| model.provider_id == "deepseek" && model.id == "deepseek-v4-flash"));
    }

    #[tokio::test]
    async fn model_resolver_uses_provider_specific_saved_key() {
        let dir = tempfile::tempdir().unwrap();
        let settings = DesktopSettingsStore::with_secret_store(
            dir.path().join("settings.json"),
            Arc::new(MemorySecretStore::default()),
        );

        settings
            .save_provider_settings(provider_save_request(
                "openai",
                "https://api.openai.com/v1",
                "gpt-5.5",
                "sk-openai",
            ))
            .await
            .unwrap();
        settings
            .save_provider_settings(provider_save_request(
                "deepseek",
                "https://api.deepseek.com",
                "deepseek-v4-flash",
                "sk-deepseek",
            ))
            .await
            .unwrap();

        let resolved = settings
            .resolve(&ModelRef::new("openai", "gpt-5.5"))
            .await
            .unwrap();

        assert_eq!(resolved.identity.provider_id, "openai");
        assert_eq!(
            resolved.endpoint.base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            resolved.credential,
            exagent::resolved::ResolvedCredential::ApiKey("sk-openai".into())
        );
    }

    #[test]
    fn runtime_settings_reject_enabled_mcp_server_empty_normalized_id() {
        let request = runtime_request_with_mcp_servers(vec![mcp_server(" !!!é ", "Valid", true)]);

        let error = validate_runtime_settings(&request).unwrap_err();

        assert!(error
            .to_string()
            .contains("MCP server id must contain an ASCII letter, number, underscore, or hyphen"));
    }

    #[test]
    fn runtime_settings_reject_enabled_mcp_server_duplicate_normalized_ids() {
        let request = runtime_request_with_mcp_servers(vec![
            mcp_server("", "foo bar", true),
            mcp_server("foo_bar", "Other", true),
        ]);

        let error = validate_runtime_settings(&request).unwrap_err();

        assert!(error
            .to_string()
            .contains("Duplicate enabled MCP server id `foo_bar`"));
    }

    #[test]
    fn runtime_settings_ignore_disabled_mcp_server_ids_for_normalized_validation() {
        let request = runtime_request_with_mcp_servers(vec![
            mcp_server("foo_bar", "Enabled", true),
            mcp_server("foo bar", "Disabled duplicate", false),
            mcp_server(" !!!é ", "Disabled empty", false),
        ]);

        validate_runtime_settings(&request).unwrap();
    }

    #[tokio::test]
    async fn runtime_config_includes_enabled_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let settings = DesktopSettingsStore::with_secret_store(
            dir.path().join("settings.json"),
            Arc::new(NoopSecretStore),
        );

        settings
            .save_runtime_settings(RuntimeSettingsSaveRequest {
                default_model: "gpt-4.1-mini".into(),
                default_thinking_mode: None,
                presets: Vec::new(),
                mcp_servers: vec![
                    McpServerSettings {
                        id: " filesystem ".into(),
                        name: " Filesystem ".into(),
                        enabled: true,
                        command: " npx ".into(),
                        args: vec![
                            " -y ".into(),
                            " @modelcontextprotocol/server-filesystem ".into(),
                            " ".into(),
                        ],
                        env: vec![(" MCP_LOG_LEVEL ".into(), "warn".into())],
                        working_directory: Some(" /tmp ".into()),
                    },
                    McpServerSettings {
                        id: " ".into(),
                        name: " Local Files ".into(),
                        enabled: true,
                        command: " node ".into(),
                        args: vec![" server.js ".into()],
                        env: Vec::new(),
                        working_directory: Some(" ".into()),
                    },
                    McpServerSettings {
                        id: "disabled".into(),
                        name: "Disabled".into(),
                        enabled: false,
                        command: "node".into(),
                        args: vec!["server.js".into()],
                        env: Vec::new(),
                        working_directory: None,
                    },
                ],
                skill_roots: Vec::new(),
                web_search: WebSearchSettings::default(),
            })
            .await
            .unwrap();

        let config = settings.runtime_config().await.unwrap();

        assert_eq!(config.mcp_servers.len(), 2);
        assert_eq!(config.mcp_servers[0].id, "filesystem");
        assert_eq!(config.mcp_servers[0].display_name, "Filesystem");
        assert_eq!(config.mcp_servers[0].command, "npx");
        assert_eq!(
            config.mcp_servers[0].args,
            vec!["-y", "@modelcontextprotocol/server-filesystem"]
        );
        assert_eq!(
            config.mcp_servers[0].env.get("MCP_LOG_LEVEL").unwrap(),
            "warn"
        );
        assert_eq!(
            config.mcp_servers[0].working_directory.as_deref().unwrap(),
            std::path::Path::new("/tmp")
        );

        assert_eq!(config.mcp_servers[1].id, "Local_Files");
        assert_eq!(config.mcp_servers[1].working_directory, None);
    }

    #[tokio::test]
    async fn runtime_config_includes_enabled_web_search_with_saved_key() {
        let dir = tempfile::tempdir().unwrap();
        let settings = DesktopSettingsStore::with_secret_store(
            dir.path().join("settings.json"),
            Arc::new(MemorySecretStore::default()),
        );

        let response = settings
            .save_runtime_settings(RuntimeSettingsSaveRequest {
                default_model: "gpt-4.1-mini".into(),
                default_thinking_mode: None,
                presets: Vec::new(),
                mcp_servers: Vec::new(),
                skill_roots: Vec::new(),
                web_search: WebSearchSettings {
                    enabled: true,
                    provider: " brave ".into(),
                    has_api_key: false,
                    api_key: Some(" search-key ".into()),
                    clear_api_key: false,
                },
            })
            .await
            .unwrap();

        assert!(response.web_search.enabled);
        assert_eq!(response.web_search.provider, "brave");
        assert!(response.web_search.has_api_key);
        assert_eq!(response.web_search.api_key, None);

        let config = settings.runtime_config().await.unwrap();
        let web_search = config.web_search.expect("web search config");
        assert_eq!(web_search.provider, "brave");
        assert_eq!(web_search.api_key, "search-key");
    }

    #[tokio::test]
    async fn runtime_config_does_not_register_web_search_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecretStore::default());
        secrets
            .set_secret("web_search:brave:api_key", "search-key")
            .unwrap();
        let settings =
            DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

        let config = settings.runtime_config().await.unwrap();

        assert!(config.web_search.is_none());
    }

    #[tokio::test]
    async fn runtime_settings_reject_enabled_web_search_without_key() {
        let dir = tempfile::tempdir().unwrap();
        let settings = DesktopSettingsStore::with_secret_store(
            dir.path().join("settings.json"),
            Arc::new(MemorySecretStore::default()),
        );

        let error = settings
            .save_runtime_settings(RuntimeSettingsSaveRequest {
                default_model: "gpt-4.1-mini".into(),
                default_thinking_mode: None,
                presets: Vec::new(),
                mcp_servers: Vec::new(),
                skill_roots: Vec::new(),
                web_search: WebSearchSettings {
                    enabled: true,
                    provider: "brave".into(),
                    has_api_key: false,
                    api_key: None,
                    clear_api_key: false,
                },
            })
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("Web search API key is required when enabled"));
    }
}
