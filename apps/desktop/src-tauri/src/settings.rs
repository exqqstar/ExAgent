use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use exagent::config::{AgentConfig, ThinkingMode};
use exagent::llm::OpenAiCompatibleLlm;
use exagent::provider::{provider_profile_by_id, provider_profiles, ProviderProfile};
use exagent::resolved::{ModelRef, ResolvedModelConfig};
use exagent::resolver::{model_context_window_from_env, resolve_from_profile, ModelResolver};
use keyring::Entry;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use exagent::provider::{ProviderAuthMode, ProviderProtocol};

const SECRET_SERVICE: &str = "dev.exagent.desktop";
const DEFAULT_PROVIDER_ID: &str = "openai";

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
    pub config: ProviderConfigView,
    pub connected_provider: Option<ConnectedProviderView>,
    pub last_connection: Option<ProviderConnectionStatusView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsSaveRequest {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub clear_api_key: bool,
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
    pub id: String,
    pub display_name: String,
    pub context_window: Option<i64>,
    pub supports_tools: Option<bool>,
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
pub struct RuntimeSettingsResponse {
    pub default_model: String,
    pub default_thinking_mode: Option<ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingsSaveRequest {
    pub default_model: String,
    pub default_thinking_mode: Option<ThinkingMode>,
    pub presets: Vec<RuntimePresetSettings>,
    pub mcp_servers: Vec<McpServerSettings>,
    pub skill_roots: Vec<SkillRootSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SettingsFile {
    provider_id: String,
    base_url: String,
    model: String,
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
            last_connection_status: None,
            last_connection_message: None,
            last_connection_checked_at: None,
            runtime_default_model: profile.default_model.to_string(),
            runtime_default_thinking_mode: None,
            runtime_presets: Vec::new(),
            mcp_servers: Vec::new(),
            skill_roots: Vec::new(),
        }
    }
}

impl DesktopSettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self::with_secret_store(path, Arc::new(KeyringSecretStore))
    }

    pub fn with_secret_store(path: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        Self { path, secrets }
    }

    pub async fn load_provider_settings(&self) -> Result<ProviderSettingsResponse> {
        let file = self.load_file().await?;
        let keychain_api_key = self
            .secrets
            .get_secret(&secret_account(&file.provider_id))?;
        let env_api_key = provider_env_api_key(&file.provider_id);
        let credential_source = credential_source(keychain_api_key.as_ref(), env_api_key.as_ref());
        Ok(self.response_from_file(file, credential_source))
    }

    pub async fn save_provider_settings(
        &self,
        request: ProviderSettingsSaveRequest,
    ) -> Result<ProviderSettingsResponse> {
        let provider = provider_by_id(&request.provider_id)
            .filter(|provider| provider.supported)
            .with_context(|| format!("unsupported provider `{}`", request.provider_id))?;
        let existing = self.load_file().await?;
        let file = SettingsFile {
            provider_id: provider.id.clone(),
            base_url: normalized_or_default(&request.base_url, &provider.default_base_url),
            model: normalized_or_default(&request.model, &provider.default_model),
            last_connection_status: None,
            last_connection_message: None,
            last_connection_checked_at: None,
            runtime_default_model: existing.runtime_default_model,
            runtime_default_thinking_mode: existing.runtime_default_thinking_mode,
            runtime_presets: existing.runtime_presets,
            mcp_servers: existing.mcp_servers,
            skill_roots: existing.skill_roots,
        };

        if request.clear_api_key {
            self.secrets
                .delete_secret(&secret_account(&file.provider_id))?;
        } else if let Some(api_key) = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.secrets
                .set_secret(&secret_account(&file.provider_id), api_key)?;
        }

        self.save_file(&file).await?;
        self.load_provider_settings().await
    }

    pub async fn load_runtime_settings(&self) -> Result<RuntimeSettingsResponse> {
        let file = self.load_file().await?;
        Ok(RuntimeSettingsResponse {
            default_model: normalized_or_default(&file.runtime_default_model, &file.model),
            default_thinking_mode: file.runtime_default_thinking_mode,
            presets: file.runtime_presets,
            mcp_servers: file.mcp_servers,
            skill_roots: file.skill_roots,
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
        self.save_file(&file).await?;
        self.load_runtime_settings().await
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

        let api_key = self
            .resolve_provider_api_key(
                &request.provider_id,
                request.api_key.as_deref(),
                request.use_saved_api_key,
            )
            .await?;
        if auth_required(provider.auth_mode) && api_key.is_none() {
            return Ok(model_list_response(
                ProviderModelListStatus::MissingCredential,
                format!("{} requires an API key to discover models.", provider.name),
                Vec::new(),
            ));
        }

        match provider.protocol {
            ProviderProtocol::OpenAiChatCompletions => {
                list_openai_compatible_models(
                    &normalized_or_default(&request.base_url, &provider.default_base_url),
                    api_key,
                )
                .await
            }
            _ => Ok(model_list_response(
                ProviderModelListStatus::UnsupportedProvider,
                format!("{} model discovery is not implemented yet.", provider.name),
                Vec::new(),
            )),
        }
    }

    pub async fn runtime_config(&self) -> Result<AgentConfig> {
        let file = self.load_file().await?;
        let mut config = AgentConfig::default();
        let model_ref = ModelRef::new(file.provider_id, file.model);
        config.model = self.resolve(&model_ref).await?;
        Ok(config)
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
            if let Some(api_key) = self
                .secrets
                .get_secret(&secret_account(provider_id))?
                .filter(|value| !value.trim().is_empty())
            {
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

    fn response_from_file(
        &self,
        file: SettingsFile,
        credential_source: CredentialSource,
    ) -> ProviderSettingsResponse {
        let providers = provider_catalog();
        let provider = provider_by_id(&file.provider_id).unwrap_or_else(|| providers[0].clone());
        let has_api_key = credential_source != CredentialSource::None;
        let auth_required = auth_required(provider.auth_mode);
        let last_connection = connection_status_from_file(&file);
        let connected_provider =
            (provider.supported && (has_api_key || !auth_required)).then(|| {
                ConnectedProviderView {
                    id: provider.id.clone(),
                    name: provider.name,
                    model: file.model.clone(),
                    base_url: file.base_url.clone(),
                }
            });

        ProviderSettingsResponse {
            providers,
            active_provider_id: file.provider_id.clone(),
            config: ProviderConfigView {
                provider_id: file.provider_id,
                base_url: file.base_url,
                model: file.model,
                has_api_key,
                credential_source,
                auth_required,
            },
            connected_provider,
            last_connection,
        }
    }
}

#[async_trait]
impl ModelResolver for DesktopSettingsStore {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig> {
        let profile = provider_profile_by_id(&model_ref.provider_id)
            .with_context(|| format!("unknown provider `{}`", model_ref.provider_id))?;
        let file = self.load_file().await?;
        let (model_id, base_url) = if file.provider_id == profile.id {
            (
                normalized_or_default(&model_ref.model_id, &file.model),
                Some(normalized_or_default(
                    &file.base_url,
                    profile.default_base_url,
                )),
            )
        } else {
            (
                normalized_or_default(&model_ref.model_id, profile.default_model),
                (!profile.default_base_url.is_empty())
                    .then(|| profile.default_base_url.to_string()),
            )
        };
        let keychain_api_key = self
            .secrets
            .get_secret(&secret_account(profile.id))?
            .filter(|value| !value.trim().is_empty());
        Ok(resolve_from_profile(
            profile,
            &model_id,
            base_url,
            keychain_api_key.or_else(|| provider_env_api_key(profile.id)),
            model_context_window_from_env(),
        ))
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

fn provider_by_id(provider_id: &str) -> Option<ProviderDescriptor> {
    provider_profile_by_id(provider_id).map(provider_descriptor)
}

fn default_provider_profile() -> &'static ProviderProfile {
    provider_profile_by_id(DEFAULT_PROVIDER_ID).expect("default provider profile must exist")
}

fn default_runtime_model() -> String {
    default_provider_profile().default_model.to_string()
}

fn secret_account(provider_id: &str) -> String {
    format!("provider:{provider_id}:api_key")
}

fn provider_env_api_key(provider_id: &str) -> Option<String> {
    exagent::resolver::provider_env_api_key(provider_id)
}

fn normalized_or_default(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn auth_required(auth_mode: ProviderAuthMode) -> bool {
    matches!(auth_mode, ProviderAuthMode::ApiKeyRequired)
}

fn validate_runtime_settings(request: &RuntimeSettingsSaveRequest) -> Result<()> {
    for preset in &request.presets {
        if preset.name.trim().is_empty() {
            anyhow::bail!("Runtime preset name is required");
        }
        if preset.model.trim().is_empty() {
            anyhow::bail!("Runtime preset model is required");
        }
    }

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

async fn list_openai_compatible_models(
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
    let models = match parse_openai_model_list(value) {
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

fn parse_openai_model_list(value: Value) -> Result<Vec<ProviderModelView>> {
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
        models.push(ProviderModelView {
            id: id.to_string(),
            display_name: id.to_string(),
            context_window: None,
            supports_tools: None,
        });
    }
    Ok(models)
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn credential_source(
    keychain_api_key: Option<&String>,
    env_api_key: Option<&String>,
) -> CredentialSource {
    if keychain_api_key
        .map(String::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        CredentialSource::Keychain
    } else if env_api_key
        .map(String::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        CredentialSource::Environment
    } else {
        CredentialSource::None
    }
}
