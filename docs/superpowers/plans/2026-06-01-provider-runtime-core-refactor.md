# Provider Runtime Core Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move ExAgent from OpenAI-shaped runtime config to provider-neutral model selection so a thread can switch provider/model between user turns while each in-flight user turn stays frozen on one resolved model config.

**Architecture:** Protocol payloads carry only `ModelRef { provider_id, model_id }`. `ThreadManager` owns an injected async `ModelResolver` and resolves `ModelRef` into runtime-only `ResolvedModelConfig` before submitting a turn to the runtime actor. `AgentConfig.model` stores that resolved model, `LlmClientFactory` builds adapters from it, and rollout/event history persists only `ModelRef`, never credentials.

**Tech Stack:** Rust, `async-trait`, `reqwest`, `serde`, `anyhow`, Tauri desktop settings, Keychain through the existing desktop secret store, React/TypeScript for model selection payloads, Tokio tests.

---

## Scope

This plan implements the Phase 1 provider-neutral runtime boundary:

- Replace OpenAI-shaped `AgentConfig` fields with `ResolvedModelConfig`.
- Introduce `ModelRef` as the single durable provider/model identity.
- Introduce async `ModelResolver` and keep Keychain I/O outside the runtime actor.
- Upgrade turn context protocol from `model: Option<String>` to `model: Option<ModelRef>`.
- Allow provider/model switching between user turns by rebuilding `Agent`/`LlmClient` before the next turn starts.

This plan does not implement Anthropic, Gemini, OAuth, SQLite settings, multi-profile credentials, or a full model picker redesign. Those can follow once this runtime boundary is in place.

## Goal Feature Execution Contract

Use this plan as a single goal-feature execution target. The goal is complete only when the implementation satisfies the acceptance criteria at the end of this document and the listed verification commands have been run with results recorded.

Execution rules:

- Execute tasks in order. Task 1 defines core types used by every later task. Task 2 depends on Task 1. Task 3 depends on Task 1 and partially on Task 2. Task 4 depends on Task 1 and Task 3. Task 5 depends on Tasks 1, 3, and 4. Task 6 is the final integration and verification gate.
- Do not expand scope to Anthropic/Gemini adapters, OAuth, SQLite settings, multi-profile credentials, or a full redesigned model picker. Unsupported providers may remain unsupported after this plan.
- Keep every task shippable on its own: add failing tests first, implement the minimal code needed, run the task verification command, then update the checklist.
- Preserve existing test seams. `ThreadManager::with_llm` and mock/shared LLM paths must continue to work, and any new resolver dependency must have an env/default implementation plus a test/static implementation.
- Treat secret handling as a hard invariant. If any implementation path serializes `ResolvedCredential`, API keys, bearer tokens, or OAuth tokens into rollout, event replay, SQLite, desktop settings JSON, logs, or debug output, stop and fix that before continuing.
- Treat in-flight turn immutability as a hard invariant. Provider/model changes selected in the GUI while a turn is running must not alter the active turn. They can affect only a later `turn_start`.

Recommended goal-feature checkpoints:

- Checkpoint A after Task 2: core model types, config shape, adapter extraction, and factory compile with root tests.
- Checkpoint B after Task 3: env resolver and desktop resolver both materialize `ResolvedModelConfig`; `runtime_config()` uses the resolver path.
- Checkpoint C after Task 4: protocol and desktop payloads carry `ModelRef`, not a bare string.
- Checkpoint D after Task 5: runtime can rebuild `Agent`/`LlmClient` between turns and persists only model identity.
- Checkpoint E after Task 6: full verification commands have been run and any residual failures are explained with file-level causes.

Each checkpoint report should include:

```text
Checkpoint: <A/B/C/D/E>
Tasks completed: <task numbers>
Files changed: <short file list>
Verification run: <commands and pass/fail>
Known gaps: <none or exact remaining issue>
Scope deviations: <none or exact reason>
```

## File Structure

Root crate:

- Create `src/model/resolved.rs`: `ModelRef`, `ProviderEndpoint`, `ResolvedCredential`, `ModelCapabilities`, `ResolvedModelConfig`, redacted debug behavior, default OpenAI-compatible model config.
- Create `src/model/resolver.rs`: async `ModelResolver`, `EnvModelResolver`, and test-friendly static resolver helpers.
- Create `src/model/openai_compatible.rs`: move `OpenAiCompatibleLlm` and OpenAI-specific request/response DTOs out of `src/model/llm.rs`.
- Create `src/model/factory.rs`: `LlmClientFactory`, `DefaultLlmClientFactory`, and `SharedLlmFactory`.
- Modify `src/model/provider.rs`: keep `PROVIDER_PROFILES` as the only static provider catalog.
- Modify `src/model/llm.rs`: keep `LlmClient`, `LlmRequestOptions`, `MockLlm`, compatibility re-export for `OpenAiCompatibleLlm`, and provider-specific error classification hook.
- Modify `src/model/mod.rs` and `src/lib.rs`: export `resolved`, `resolver`, `factory`, and `openai_compatible`.
- Modify `src/config.rs`: replace `model: String`, `openai_base_url`, `openai_api_key`, and `model_context_window` with `model: ResolvedModelConfig`.
- Modify `src/app_server/protocol.rs`: change `TurnContextOverrides.model` to `Option<ModelRef>`.
- Modify `src/app_server/thread_manager.rs`: inject `ModelResolver`, resolve turn model refs in async app-server code, and pass resolved models to runtime.
- Modify `src/app_server/service.rs`: add constructors that accept model resolver injection and an env-backed production constructor.
- Modify `src/runtime/thread_runtime.rs`: change `ThreadTurnContext` to carry runtime-only `resolved_model: Option<ResolvedModelConfig>`.
- Modify `src/runtime/thread_session/mod.rs`: retain `AgentFactory` in `ThreadSession` so the session can rebuild its `Agent` between turns.
- Modify `src/runtime/thread_session/turn.rs`: freeze the resolved model per user turn and rebuild `Agent` before the turn when resolved provider config changes.
- Modify `src/runtime/context.rs` and `src/state/session.rs`: persist/display `ModelRef`, not credentials.
- Modify `src/entrypoints/api.rs`, `src/main.rs`, and `src/entrypoints/cli_adapter.rs` as needed to use env-backed resolver/bootstrap.

Desktop:

- Modify `apps/desktop/src-tauri/src/settings.rs`: implement a Keychain-backed model resolver and make `runtime_config()` resolve the default `ModelRef` through that resolver.
- Modify `apps/desktop/src-tauri/src/lib.rs`: pass the desktop resolver into `AppServerService`.
- Modify `apps/desktop/src-tauri/src/commands.rs`: accept `ModelRef` for `turn_start`.
- Modify `apps/desktop/src/types.ts`, `apps/desktop/src/api/exagentClient.ts`, `apps/desktop/src/stores/workbenchStore.ts`, and `apps/desktop/src/components/Composer.tsx`: send a model selection as `ModelRef` instead of a bare string.

Tests:

- Modify `tests/llm_http.rs`: replace `from_env`/`from_env_with_model` coverage with explicit `from_parts` and resolver/bootstrap coverage.
- Add `tests/llm_factory.rs`: verify adapter factory dispatch and shared factory behavior.
- Add `tests/model_resolver.rs`: verify env resolver and redacted credential behavior.
- Modify `tests/app_server_boundary.rs`, `tests/thread_runtime.rs`, `tests/override_policy.rs`, and runtime tests for `ModelRef` turn overrides.
- Modify `apps/desktop/src-tauri/tests/provider_settings.rs`: assert desktop resolver behavior, Keychain-over-env precedence, and default runtime config materialization.

---

## Task 1: ModelRef, ResolvedModelConfig, and AgentConfig

**Files:**
- Create: `src/model/resolved.rs`
- Modify: `src/model/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/config.rs`
- Test: `src/model/resolved.rs`
- Test: `src/config.rs`

- [ ] **Step 1: Add failing unit tests for identity and secret redaction**

Add tests before implementation:

```rust
#[test]
fn model_ref_display_uses_provider_and_model() {
    let model = ModelRef {
        provider_id: "openai".to_string(),
        model_id: "gpt-4.1".to_string(),
    };

    assert_eq!(model.display(), "openai:gpt-4.1");
}

#[test]
fn resolved_credential_debug_redacts_secret_values() {
    let credential = ResolvedCredential::ApiKey("sk-secret".to_string());
    let debug = format!("{credential:?}");

    assert!(debug.contains("***"));
    assert!(!debug.contains("sk-secret"));
}

#[test]
fn default_agent_config_has_resolved_openai_model_without_env_secret() {
    std::env::set_var("OPENAI_API_KEY", "sk-env");

    let config = AgentConfig::default();

    assert_eq!(config.model.identity.provider_id, "openai");
    assert_eq!(config.model.identity.model_id, "gpt-4.1");
    assert_eq!(config.model.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(config.model.credential, ResolvedCredential::None);

    std::env::remove_var("OPENAI_API_KEY");
}
```

Run:

```bash
cargo test -p exagent model_ref_display_uses_provider_and_model
cargo test -p exagent resolved_credential_debug_redacts_secret_values
cargo test -p exagent default_agent_config_has_resolved_openai_model_without_env_secret
```

Expected: FAIL until the new types and config shape exist.

- [ ] **Step 2: Create resolved model types**

Create `src/model/resolved.rs`:

```rust
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::provider::{provider_profile_by_id, ProviderProtocol};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }

    pub fn display(&self) -> String {
        format!("{}:{}", self.provider_id, self.model_id)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub base_url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ResolvedCredential {
    None,
    ApiKey(String),
    BearerToken(String),
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolvedCredential::None => formatter.write_str("None"),
            ResolvedCredential::ApiKey(_) => formatter.write_str("ApiKey(***)"),
            ResolvedCredential::BearerToken(_) => formatter.write_str("BearerToken(***)"),
        }
    }
}

impl Default for ResolvedCredential {
    fn default() -> Self {
        ResolvedCredential::None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub context_window: Option<i64>,
    pub supports_tools: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedModelConfig {
    pub identity: ModelRef,
    pub protocol: ProviderProtocol,
    pub endpoint: ProviderEndpoint,
    pub credential: ResolvedCredential,
    pub capabilities: ModelCapabilities,
}

impl ResolvedModelConfig {
    pub fn from_provider_profile(
        provider_id: &str,
        model_id: impl Into<String>,
        base_url: Option<String>,
        credential: ResolvedCredential,
        context_window: Option<i64>,
    ) -> Self {
        let profile = provider_profile_by_id(provider_id)
            .unwrap_or_else(|| provider_profile_by_id("openai").expect("openai profile exists"));
        Self {
            identity: ModelRef::new(profile.id, model_id),
            protocol: profile.protocol,
            endpoint: ProviderEndpoint {
                base_url: base_url.or_else(|| Some(profile.default_base_url.to_string())),
            },
            credential,
            capabilities: ModelCapabilities {
                context_window,
                supports_tools: profile.supports_tools,
            },
        }
    }
}

impl Default for ResolvedModelConfig {
    fn default() -> Self {
        let profile = provider_profile_by_id("openai").expect("openai profile exists");
        Self::from_provider_profile(
            profile.id,
            profile.default_model,
            Some(profile.default_base_url.to_string()),
            ResolvedCredential::None,
            None,
        )
    }
}
```

- [ ] **Step 3: Export resolved model types**

Update `src/model/mod.rs`:

```rust
pub mod factory;
pub mod llm;
pub mod openai_compatible;
pub mod provider;
pub mod resolved;
pub mod resolver;
pub mod types;
```

Update `src/lib.rs`:

```rust
pub use model::resolved;
pub use model::resolver;
```

- [ ] **Step 4: Change AgentConfig shape**

Update `src/config.rs`:

```rust
use crate::resolved::ResolvedModelConfig;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: ResolvedModelConfig,
    pub thinking_mode: Option<ThinkingMode>,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub policy_mode: PolicyMode,
    pub auto_compact_token_limit: Option<i64>,
}
```

Update compaction lookup:

```rust
pub fn resolved_auto_compact_token_limit(&self) -> Option<i64> {
    let context_limit = self.model.capabilities.context_window.map(ninety_percent);

    match (self.auto_compact_token_limit, context_limit) {
        (Some(configured), Some(context_limit)) => Some(configured.min(context_limit)),
        (Some(configured), None) => Some(configured),
        (None, Some(context_limit)) => Some(context_limit),
        (None, None) => None,
    }
}
```

Update `Default` so it no longer reads `OPENAI_MODEL`, `OPENAI_BASE_URL`, `OPENAI_API_KEY`, or `EXAGENT_MODEL_CONTEXT_WINDOW`:

```rust
model: ResolvedModelConfig::default(),
thinking_mode: parse_optional_thinking_mode_env("EXAGENT_THINKING_MODE"),
```

- [ ] **Step 5: Update config tests**

Replace direct `model_context_window` writes with:

```rust
let mut config = AgentConfig {
    auto_compact_token_limit: Some(95_000),
    ..AgentConfig::default()
};
config.model.capabilities.context_window = Some(100_000);
```

Run:

```bash
cargo test -p exagent config
```

Expected: PASS.

## Task 2: OpenAI-Compatible Adapter and LLM Factory

**Files:**
- Create: `src/model/openai_compatible.rs`
- Create: `src/model/factory.rs`
- Modify: `src/model/llm.rs`
- Modify: `src/model/mod.rs`
- Modify: `tests/llm_http.rs`
- Add: `tests/llm_factory.rs`

- [ ] **Step 1: Add failing factory tests**

Create `tests/llm_factory.rs`:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use exagent::llm::{LlmClient, LlmRequestOptions};
use exagent::model::factory::{DefaultLlmClientFactory, LlmClientFactory, SharedLlmFactory};
use exagent::provider::ProviderProtocol;
use exagent::resolved::{ResolvedCredential, ResolvedModelConfig};
use exagent::types::{AssistantTurn, ConversationMessage, LlmCompletion};

struct StaticLlm;

#[async_trait]
impl LlmClient for StaticLlm {
    async fn complete(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
        _options: &LlmRequestOptions,
    ) -> anyhow::Result<LlmCompletion> {
        Ok(AssistantTurn {
            text: Some("ok".to_string()),
            tool_calls: Vec::new(),
        }
        .into_completion())
    }
}

#[test]
fn default_factory_builds_openai_compatible_client() {
    let model = ResolvedModelConfig::from_provider_profile(
        "openai_compatible",
        "local-model",
        Some("http://127.0.0.1:11434/v1".to_string()),
        ResolvedCredential::None,
        None,
    );

    let client = DefaultLlmClientFactory.build(&model);

    assert!(client.is_ok());
}

#[test]
fn default_factory_rejects_unsupported_protocols() {
    let mut model = ResolvedModelConfig::from_provider_profile(
        "anthropic",
        "claude-sonnet",
        Some("https://api.anthropic.com/v1".to_string()),
        ResolvedCredential::ApiKey("secret".to_string()),
        None,
    );
    model.protocol = ProviderProtocol::AnthropicMessages;

    let err = DefaultLlmClientFactory.build(&model).unwrap_err().to_string();

    assert!(err.contains("not implemented"));
}

#[test]
fn shared_factory_ignores_model_config() {
    let factory = SharedLlmFactory::new(Arc::new(StaticLlm));
    let model = ResolvedModelConfig::default();

    assert!(factory.build(&model).is_ok());
}
```

Run:

```bash
cargo test --test llm_factory
```

Expected: FAIL until the factory module exists.

- [ ] **Step 2: Move OpenAI-compatible adapter**

Move `OpenAiCompatibleLlm`, OpenAI request DTOs, response DTOs, parsing helpers, and endpoint helpers from `src/model/llm.rs` into `src/model/openai_compatible.rs`.

Keep this compatibility re-export in `src/model/llm.rs`:

```rust
pub use super::openai_compatible::OpenAiCompatibleLlm;
```

Remove `from_env` and `from_env_with_model` from the adapter. Keep:

```rust
pub fn from_config(model: &ResolvedModelConfig) -> Result<Self>
pub fn from_parts(
    model: impl Into<String>,
    base_url: impl Into<String>,
    api_key: Option<impl Into<String>>,
) -> Result<Self>
```

`from_config` extracts API key material without logging it:

```rust
let api_key = match &model.credential {
    ResolvedCredential::ApiKey(value) | ResolvedCredential::BearerToken(value) => {
        Some(value.clone())
    }
    ResolvedCredential::None => None,
};
```

- [ ] **Step 3: Add provider-specific error classification**

Change `LlmClient` in `src/model/llm.rs`:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion>;

    fn is_context_window_error(&self, _err: &anyhow::Error) -> bool {
        false
    }
}
```

Move the current OpenAI context-window string matching into the OpenAI-compatible adapter implementation. Keep a temporary compatibility function only for existing tests:

```rust
pub fn is_context_window_error(err: &anyhow::Error) -> bool {
    super::openai_compatible::is_openai_context_window_error(err)
}
```

- [ ] **Step 4: Implement LLM factory**

Create `src/model/factory.rs` with:

```rust
use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;

use crate::llm::{LlmClient, LlmRequestOptions, OpenAiCompatibleLlm};
use crate::provider::ProviderProtocol;
use crate::resolved::ResolvedModelConfig;
use crate::types::{ConversationMessage, LlmCompletion};

pub trait LlmClientFactory: Send + Sync {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>>;
}

pub struct DefaultLlmClientFactory;

impl LlmClientFactory for DefaultLlmClientFactory {
    fn build(&self, model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>> {
        match model.protocol {
            ProviderProtocol::OpenAiChatCompletions => {
                Ok(Box::new(OpenAiCompatibleLlm::from_config(model)?))
            }
            ProviderProtocol::AnthropicMessages => {
                bail!("Anthropic Messages adapter is not implemented")
            }
            ProviderProtocol::GeminiGenerateContent => {
                bail!("Gemini Generate Content adapter is not implemented")
            }
            ProviderProtocol::CopilotOAuth => {
                bail!("GitHub Copilot OAuth runtime adapter is not implemented")
            }
        }
    }
}

pub struct SharedLlmFactory {
    llm: Arc<dyn LlmClient>,
}

impl SharedLlmFactory {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

impl LlmClientFactory for SharedLlmFactory {
    fn build(&self, _model: &ResolvedModelConfig) -> Result<Box<dyn LlmClient>> {
        Ok(Box::new(SharedLlmClient {
            llm: self.llm.clone(),
        }))
    }
}

struct SharedLlmClient {
    llm: Arc<dyn LlmClient>,
}

#[async_trait]
impl LlmClient for SharedLlmClient {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.llm.complete(messages, tools, options).await
    }

    fn is_context_window_error(&self, err: &anyhow::Error) -> bool {
        self.llm.is_context_window_error(err)
    }
}
```

- [ ] **Step 5: Verify adapter and factory**

Run:

```bash
cargo test --test llm_http
cargo test --test llm_factory
cargo fmt --check
```

Expected: all pass.

## Task 3: ModelResolver and Runtime Config Materialization

**Files:**
- Create: `src/model/resolver.rs`
- Modify: `src/model/mod.rs`
- Modify: `src/app_server/service.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Add: `tests/model_resolver.rs`
- Modify: `apps/desktop/src-tauri/tests/provider_settings.rs`

- [ ] **Step 1: Add failing resolver tests**

Create `tests/model_resolver.rs`:

```rust
use exagent::model::resolver::{EnvModelResolver, ModelResolver};
use exagent::provider::ProviderProtocol;
use exagent::resolved::{ModelRef, ResolvedCredential};

#[tokio::test]
async fn env_model_resolver_materializes_openai_model() {
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    std::env::set_var("EXAGENT_MODEL_CONTEXT_WINDOW", "100000");

    let resolver = EnvModelResolver;
    let resolved = resolver
        .resolve(&ModelRef::new("openai", "env-model"))
        .await
        .unwrap();

    assert_eq!(resolved.identity, ModelRef::new("openai", "env-model"));
    assert_eq!(resolved.protocol, ProviderProtocol::OpenAiChatCompletions);
    assert_eq!(
        resolved.endpoint.base_url.as_deref(),
        Some("https://env.example/v1")
    );
    assert!(matches!(
        resolved.credential,
        ResolvedCredential::ApiKey(ref value) if value == "sk-env"
    ));
    assert_eq!(resolved.capabilities.context_window, Some(100_000));

    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("EXAGENT_MODEL_CONTEXT_WINDOW");
}

#[tokio::test]
async fn env_model_resolver_rejects_unknown_provider() {
    let resolver = EnvModelResolver;
    let err = resolver
        .resolve(&ModelRef::new("missing", "model"))
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("unknown provider"));
}
```

Run:

```bash
cargo test --test model_resolver
```

Expected: FAIL until resolver exists.

- [ ] **Step 2: Implement core resolver**

Create `src/model/resolver.rs`:

```rust
use anyhow::{bail, Result};
use async_trait::async_trait;

use crate::provider::{provider_profile_by_id, ProviderAuthMode};
use crate::resolved::{ModelRef, ResolvedCredential, ResolvedModelConfig};

#[async_trait]
pub trait ModelResolver: Send + Sync {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig>;
}

pub struct EnvModelResolver;

#[async_trait]
impl ModelResolver for EnvModelResolver {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<ResolvedModelConfig> {
        let profile = provider_profile_by_id(&model_ref.provider_id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider {}", model_ref.provider_id))?;
        let base_url = provider_env_base_url(profile.id)
            .or_else(|| Some(profile.default_base_url.to_string()))
            .filter(|value| !value.trim().is_empty());
        let credential = provider_env_api_key(profile.id)
            .filter(|value| !value.trim().is_empty())
            .map(ResolvedCredential::ApiKey)
            .unwrap_or(ResolvedCredential::None);
        if profile.auth_mode == ProviderAuthMode::ApiKeyRequired
            && matches!(credential, ResolvedCredential::None)
        {
            bail!("{} requires an API key", profile.name);
        }
        let context_window = std::env::var("EXAGENT_MODEL_CONTEXT_WINDOW")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|value| *value > 0);

        Ok(ResolvedModelConfig::from_provider_profile(
            profile.id,
            model_ref.model_id.clone(),
            base_url,
            credential,
            context_window,
        ))
    }
}

fn provider_env_api_key(provider_id: &str) -> Option<String> {
    match provider_id {
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
        "google" => std::env::var("GOOGLE_API_KEY").ok(),
        _ => None,
    }
}

fn provider_env_base_url(provider_id: &str) -> Option<String> {
    match provider_id {
        "openai" => std::env::var("OPENAI_BASE_URL").ok(),
        "anthropic" => std::env::var("ANTHROPIC_BASE_URL").ok(),
        "google" => std::env::var("GOOGLE_BASE_URL").ok(),
        _ => None,
    }
}
```

- [ ] **Step 3: Inject resolver into app server**

Update `AppServerService` and `ThreadManager` constructors:

```rust
pub fn with_config_and_model_resolver(
    config: AgentConfig,
    model_resolver: Arc<dyn ModelResolver>,
) -> Self
```

`AppServerService::new()` and `ThreadManager::from_env()` should use:

```rust
Arc::new(EnvModelResolver)
```

`with_llm` should keep using a shared LLM factory and use `EnvModelResolver` unless a test-specific resolver is passed through a new `with_llm_and_model_resolver` constructor.

- [ ] **Step 4: Implement desktop Keychain resolver**

In `apps/desktop/src-tauri/src/settings.rs`, implement `ModelResolver` for `DesktopSettingsStore` or a small wrapper type around it:

```rust
#[async_trait::async_trait]
impl exagent::resolver::ModelResolver for DesktopSettingsStore {
    async fn resolve(
        &self,
        model_ref: &exagent::resolved::ModelRef,
    ) -> anyhow::Result<exagent::resolved::ResolvedModelConfig> {
        let file = self.load_file().await?;
        let provider_id = model_ref.provider_id.as_str();
        let provider = exagent::provider::provider_profile_by_id(provider_id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider {provider_id}"))?;
        let base_url = if file.provider_id == provider_id {
            Some(file.base_url)
        } else {
            Some(provider.default_base_url.to_string())
        };
        let keychain_api_key = self.secrets.get_secret(&secret_account(provider_id))?;
        let credential = keychain_api_key
            .or_else(|| provider_env_api_key(provider_id))
            .filter(|value| !value.trim().is_empty())
            .map(exagent::resolved::ResolvedCredential::ApiKey)
            .unwrap_or(exagent::resolved::ResolvedCredential::None);

        Ok(exagent::resolved::ResolvedModelConfig::from_provider_profile(
            provider.id,
            model_ref.model_id.clone(),
            base_url,
            credential,
            file.context_window,
        ))
    }
}
```

Use the existing desktop provider/env helpers instead of duplicating provider env lookup logic if those helpers already exist.

- [ ] **Step 5: Make runtime_config resolve the default ModelRef**

Change `DesktopSettingsStore::runtime_config()` so it no longer has its own credential/base URL materialization path:

```rust
let file = self.load_file().await?;
let mut config = AgentConfig::default();
let model_ref = ModelRef::new(file.provider_id, file.model);
config.model = self.resolve(&model_ref).await?;
Ok(config)
```

This keeps default runtime config and per-turn model selection on the same resolution path.

- [ ] **Step 6: Wire desktop resolver into AppServerService**

In `apps/desktop/src-tauri/src/lib.rs`, build the service with the same settings store used by the settings commands:

```rust
let settings = settings::DesktopSettingsStore::new(app_data_dir.join("settings.json"));
let config = tauri::async_runtime::block_on(settings.runtime_config())?;
let model_resolver: std::sync::Arc<dyn exagent::resolver::ModelResolver> =
    std::sync::Arc::new(settings.clone());
let facade = exagent::app_server::desktop_facade::DesktopFacade::new(
    exagent::app_server::AppServerService::with_config_and_model_resolver(
        config,
        model_resolver,
    ),
    index.clone(),
);
```

If `DesktopSettingsStore` is not cloneable, make it cloneable by sharing its path and secret store through cloneable fields, or create a cloneable `KeychainModelResolver` wrapper.

- [ ] **Step 7: Verify resolver behavior**

Run:

```bash
cargo test --test model_resolver
cargo test -p exagent-desktop --test provider_settings
cargo fmt --check
```

Expected: all pass.

## Task 4: Protocol and GUI ModelRef Payload

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/override_policy.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src/types.ts`
- Modify: `apps/desktop/src/api/exagentClient.ts`
- Modify: `apps/desktop/src/stores/workbenchStore.ts`
- Modify: `apps/desktop/src/components/Composer.tsx`
- Modify: `tests/app_server_boundary.rs`
- Modify: `tests/override_policy.rs`
- Modify: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add failing protocol tests**

Update or add an app-server boundary test that sends a model ref:

```rust
turn_context: Some(TurnContextOverrides {
    cwd: None,
    model: Some(ModelRef::new("openai_compatible", "local-model")),
    thinking_mode: None,
}),
```

Assert that the turn context persisted to rollout contains only the identity:

```rust
assert_eq!(
    turn_context.model,
    Some(ModelRef::new("openai_compatible", "local-model"))
);
```

Run:

```bash
cargo test --test app_server_boundary model_ref
```

Expected: FAIL until protocol and state structs use `ModelRef`.

- [ ] **Step 2: Change protocol DTO**

In `src/app_server/protocol.rs`:

```rust
use crate::resolved::ModelRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnContextOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
}
```

Keep `IgnoredOverrideField::Provider`; it remains useful for resume paths that reject provider-level overrides.

- [ ] **Step 3: Update desktop Tauri command**

In `apps/desktop/src-tauri/src/commands.rs`, change:

```rust
model: Option<String>,
```

to:

```rust
model: Option<exagent::resolved::ModelRef>,
```

Build `TurnContextOverrides` from a trimmed valid ref:

```rust
let model = model.filter(|model| {
    !model.provider_id.trim().is_empty() && !model.model_id.trim().is_empty()
});
```

- [ ] **Step 4: Update TypeScript model selection type**

In `apps/desktop/src/types.ts`:

```ts
export type ModelRef = {
  provider_id: string;
  model_id: string;
};
```

Change selected model state from `string | null` to `ModelRef | null`.

In `apps/desktop/src/api/exagentClient.ts`, change `startTurn` to send:

```ts
model: model?.provider_id && model?.model_id ? model : null,
```

In `workbenchStore.ts` and `Composer.tsx`, keep the UI minimal if the picker is not ready: convert the existing input into a `ModelRef` using the active/default provider, or pass through an already selected `ModelRef` from provider settings. The payload crossing the Tauri boundary must be `ModelRef`, not a bare string.

- [ ] **Step 5: Verify protocol and UI payload**

Run:

```bash
cargo test --test app_server_boundary
cargo test --test override_policy
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass.

## Task 5: ThreadManager Resolution and Runtime Between-Turn Rebuild

**Files:**
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/context.rs`
- Modify: `src/state/session.rs`
- Modify: `src/state/rollout.rs` if rollout deserialization needs compatibility
- Modify: `tests/thread_runtime.rs`
- Modify: runtime tests in `src/runtime/thread_session/turn.rs`

- [ ] **Step 1: Update ThreadTurnContext**

In `src/runtime/thread_runtime.rs`, change:

```rust
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
    pub resolved_model: Option<ResolvedModelConfig>,
    pub thinking_mode: Option<ThinkingMode>,
}
```

This type is runtime-only. It can carry credentials in memory, but it must never be serialized into rollout items.

- [ ] **Step 2: Resolve model refs before actor submission**

Change `resolve_turn_context` in `src/app_server/thread_manager.rs` from sync to async:

```rust
async fn resolve_turn_context(
    model_resolver: &dyn ModelResolver,
    snapshot: &ThreadSnapshot,
    overrides: Option<TurnContextOverrides>,
) -> Result<Option<ThreadTurnContext>>
```

Inside it:

```rust
let resolved_model = match overrides.model.as_ref() {
    Some(model_ref) => Some(model_resolver.resolve(model_ref).await?),
    None => None,
};
```

Update async call sites such as `turn_start_direct`, `turn_start_and_wait`, and background turn start paths to `.await` model resolution before submitting to `ThreadRuntime`.

- [ ] **Step 3: Retain AgentFactory in ThreadSession**

In `src/runtime/thread_session/mod.rs`, add:

```rust
agent_factory: AgentFactory,
```

to `ThreadSession`, and store it in `ThreadSession::new`:

```rust
agent_factory,
```

This lets the session rebuild the `Agent` between user turns without putting resolver logic inside the actor.

- [ ] **Step 4: Rebuild Agent before a turn when resolved model changes**

In `src/runtime/thread_session/turn.rs`, build a turn config before recording context:

```rust
fn config_for_turn(
    config: &AgentConfig,
    resolved_model: Option<&ResolvedModelConfig>,
    turn_thinking_mode: Option<ThinkingMode>,
) -> AgentConfig {
    let mut config = config.clone();
    if let Some(model) = resolved_model {
        config.model = model.clone();
    }
    if let Some(thinking_mode) = turn_thinking_mode {
        config.thinking_mode = Some(thinking_mode);
    }
    config
}
```

At the start of `handle_user_input_inner`, after extracting `turn_context` and before `record_user_turn_start`, rebuild only when needed:

```rust
let turn_config = config_for_turn(
    self.agent.config(),
    turn_context
        .as_ref()
        .and_then(|context| context.resolved_model.as_ref()),
    turn_thinking_mode,
);

if self.agent.config().model != turn_config.model {
    self.agent = (self.agent_factory)(turn_config.clone())?;
}
```

The rebuilt agent uses the resolved model frozen for this user turn. Later GUI model selection changes cannot affect the current running turn.

- [ ] **Step 5: Persist only ModelRef**

Update `PromptContext::for_turn` and `TurnContextItem` so stored turn context uses:

```rust
model: Some(config.model.identity.clone())
```

or equivalent optional semantics that match current context behavior.

If existing rollout files store a string model, add read compatibility with a custom deserializer that maps an old string to:

```rust
ModelRef::new("openai", old_model_string)
```

New writes must serialize `ModelRef`, not a display string and not `ResolvedModelConfig`.

- [ ] **Step 6: Preserve context-window and token usage behavior**

Replace:

```rust
agent.config().model_context_window
```

with:

```rust
agent.config().model.capabilities.context_window
```

Do not change `TokenUsageInfo.model_context_window`; it remains a historical numeric field in events and rollout state.

- [ ] **Step 7: Verify runtime behavior**

Run:

```bash
cargo test --test thread_runtime
cargo test runtime::thread_session::turn
cargo test --test app_server_boundary
cargo fmt --check
```

Expected: all pass.

## Task 6: Env Bootstrap, Desktop Runtime Config, and Full Verification

**Files:**
- Modify: `src/main.rs`
- Modify: `src/entrypoints/api.rs`
- Modify: `src/app_server/service.rs`
- Modify: `tests/cli_adapter.rs`
- Modify: `tests/api_server.rs`
- Modify: `apps/desktop/src-tauri/tests/provider_settings.rs`
- Inspect only: `src/model/types.rs`
- Inspect only: `src/state/events.rs`
- Inspect only: `src/state/rollout.rs`

- [ ] **Step 1: Use env resolver in CLI/API startup**

Production CLI/API startup should materialize config by resolving the default env model ref:

```rust
let resolver = std::sync::Arc::new(exagent::resolver::EnvModelResolver);
let model_ref = exagent::resolved::ModelRef::new(
    "openai",
    std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string()),
);
let mut config = exagent::config::AgentConfig::default();
config.model = resolver.resolve(&model_ref).await?;
let service = exagent::app_server::AppServerService::with_config_and_model_resolver(
    config,
    resolver,
);
```

If this logic is shared, put it behind:

```rust
pub async fn agent_config_from_process_env() -> anyhow::Result<AgentConfig>
```

and make it call `EnvModelResolver::resolve`.

- [ ] **Step 2: Update CLI/API tests**

Replace assertions against removed fields:

```rust
config.openai_api_key
config.openai_base_url
config.model == "..."
```

with:

```rust
config.model.identity == ModelRef::new("openai", "...")
config.model.endpoint.base_url.as_deref() == Some("...")
matches!(config.model.credential, ResolvedCredential::ApiKey(_))
```

- [ ] **Step 3: Confirm serialized token usage schema is unchanged**

Run:

```bash
cargo test token_usage_info
cargo test event_token_usage_serializes_model_context_window
cargo test rollout
```

Expected: tests that assert `model_context_window` in serialized event/rollout data still pass without schema changes.

- [ ] **Step 4: Run workspace verification**

Run:

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p exagent-desktop
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass. If `cargo clippy` reports pre-existing warnings unrelated to this migration, record the exact warnings in the implementation summary and keep `cargo test` and desktop build green.

## Acceptance Criteria

- `ModelRef { provider_id, model_id }` is the only durable provider/model identity type.
- `ResolvedModelConfig.identity` is a `ModelRef`.
- `TurnStartParams` and `TurnContextOverrides` carry `ModelRef`, never `ResolvedModelConfig`.
- `AgentConfig` no longer exposes `model: String`, `openai_base_url`, `openai_api_key`, or `model_context_window`.
- `AgentConfig::default()` no longer reads `OPENAI_MODEL`, `OPENAI_BASE_URL`, `OPENAI_API_KEY`, or `EXAGENT_MODEL_CONTEXT_WINDOW`.
- `ModelResolver` is async and object-safe through `async_trait`.
- Core has an env-backed `ModelResolver`; desktop has a Keychain-backed resolver.
- `DesktopSettingsStore::runtime_config()` resolves the default `ModelRef` through the same desktop resolver used for per-turn selections.
- `ThreadManager` resolves `ModelRef` before submitting a turn to the runtime actor.
- Runtime actor code never performs Keychain I/O.
- A running user turn freezes its `ResolvedModelConfig`.
- A thread can switch provider/model between user turns by rebuilding `Agent`/`LlmClient` before the next turn starts.
- `ThreadManager` does not match provider protocol to build clients; only `DefaultLlmClientFactory` does.
- Rollout/event replay persists only `ModelRef` for model identity and never persists `ResolvedCredential`.
- Context-window retry/compaction reads `config.model.capabilities.context_window`.
- `TokenUsageInfo.model_context_window` remains a historical numeric field in events and rollout state.
- Secrets are not printed by `Debug` and are not persisted into JSONL, SQLite, or desktop settings JSON.

## Goal Completion Gate

The goal feature may mark this implementation complete only after all of the following are true:

- Every task checkbox in this plan is complete, or any skipped checkbox has a written reason tied to an explicit non-goal.
- The full verification block in Task 6 has been run:

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p exagent-desktop
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

- Any failure in the verification block is either fixed or documented as pre-existing with the exact failing command, failing test/lint/build line, and reason it is outside this goal.
- A final implementation summary states how the code enforces these four invariants:
  - protocol carries only `ModelRef`;
  - resolver materializes runtime-only `ResolvedModelConfig` before actor submission;
  - running turns freeze their resolved model config;
  - persisted history never stores credentials.
