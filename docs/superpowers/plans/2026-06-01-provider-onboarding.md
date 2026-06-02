# Provider Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the desktop Providers settings page from a configuration form into a real provider connection surface that can use GUI credentials, fall back to environment credentials, test connectivity, and grow into provider-specific adapters.

**Architecture:** Keep runtime execution behind `LlmClient` and keep desktop UI behind Tauri commands. Provider settings own catalog/profile metadata, credential storage, connection testing, and runtime config materialization; runtime adapters own provider protocol details such as OpenAI-compatible Chat Completions or Anthropic Messages.

**Tech Stack:** Rust, Tauri v2 commands, Keychain through `keyring`, React, TypeScript, Vite, Tailwind CSS, Radix/shadcn primitives, Vitest, Tokio tests, Axum-based local HTTP test servers.

---

## Current State

- CLI/runtime can use `OPENAI_API_KEY`, `OPENAI_BASE_URL`, and `OPENAI_MODEL` through `AgentConfig::default()`.
- Desktop Settings can save provider config to app data and API keys to Keychain.
- Saving provider settings rebuilds the desktop `DesktopFacade`, so later turns use the saved config.
- The current UI still feels incomplete because `Connect` only selects a row, `Save provider` does not test the connection, and missing Keychain credentials can shadow otherwise valid environment credentials.
- `OpenAI` and `OpenAI Compatible` are listed as supported, but both currently rely on the same OpenAI-compatible Chat Completions adapter.
- `Anthropic`, `Google`, and `GitHub Copilot` are disabled catalog entries, meaning they are visible roadmap items but cannot be connected yet.

## Source Documents

- `docs/superpowers/specs/2026-06-01-provider-onboarding.md`
- `docs/architecture/adr/0010-use-provider-profiles-and-thin-http-adapters.md`
- `docs/superpowers/plans/2026-06-01-provider-runtime-core-refactor.md`
- OpenAI SDKs and CLI: https://platform.openai.com/docs/libraries
- Anthropic Client SDKs: https://platform.claude.com/docs/en/api/client-sdks
- Gemini API libraries: https://ai.google.dev/gemini-api/docs/libraries

## Phase Split Note

The core `AgentConfig` field migration is split into `docs/superpowers/plans/2026-06-01-provider-runtime-core-refactor.md` because it must change root runtime config, desktop `runtime_config()`, and desktop provider settings tests together. This provider onboarding plan remains the broader product/backend/UI plan for connection testing, model discovery, provider settings UX, and later provider support.

## Phase 1 Acceptance Criteria

- Environment credentials still work in desktop when no GUI key is saved.
- GUI-saved Keychain credentials take precedence over environment credentials.
- OpenAI-compatible local gateways can be configured without an API key.
- The Settings page has a `Test connection` action that uses unsaved form values.
- `Test connection` reports success, authentication failure, model not found, network failure, and unsupported provider states with clear messages.
- `Connect` starts a real connection flow by selecting the provider and focusing its form; `Save connection` persists it.
- Disabled providers cannot be saved or tested and show an explicit "Coming soon" reason.
- New tests cover runtime config materialization, optional auth, provider test command behavior, and UI test states.

## Long-Term Acceptance Criteria

- Provider catalog is represented as structured profiles with auth mode, protocol, default endpoint, default model, model discovery support, and capability flags.
- Anthropic has a real `LlmClient` adapter and can run turns with text and tool calls.
- Model discovery can populate a model selector for providers that expose a model list endpoint.
- Users can store multiple credentials/profiles per provider and select one globally or per project.
- OAuth-capable providers have a separate account connection flow that does not reuse API key fields.
- Last connection status is persisted and displayed in Settings without requiring a retest on every open.

## File Structure

Root crate:

- Modify `src/config.rs`: add provider/runtime config fields only if needed for provider-specific dispatch.
- Modify `src/model/llm.rs`: allow optional bearer auth for OpenAI-compatible requests and add connection-probe support.
- Create `src/model/provider.rs`: provider protocol, auth mode, connection status, model info, and test result types.
- Create `src/model/openai_compatible.rs` later if `llm.rs` becomes too broad.
- Create `src/model/anthropic.rs` in Phase 2 for the Messages API adapter.
- Modify `src/app_server/thread_manager.rs`: dispatch `LlmClient` construction by provider protocol when Phase 2 begins.
- Add tests in `tests/provider_runtime_config.rs`.
- Add tests in `tests/openai_compatible_llm.rs`.

Desktop backend:

- Modify `apps/desktop/src-tauri/src/settings.rs`: provider catalog profiles, env fallback, optional auth, saved status.
- Modify `apps/desktop/src-tauri/src/commands.rs`: add `provider_connection_test`.
- Modify `apps/desktop/src-tauri/src/lib.rs`: register `provider_connection_test`.
- Add tests in `apps/desktop/src-tauri/tests/provider_settings.rs`.
- Add tests in `apps/desktop/src-tauri/tests/provider_connection.rs`.

Desktop frontend:

- Modify `apps/desktop/src/types.ts`: provider auth mode, connection test request/response, credential source, last status.
- Modify `apps/desktop/src/api/exagentClient.ts`: add `testProviderConnection`.
- Modify `apps/desktop/src/components/SettingsDialog.tsx`: make provider connection flow explicit.
- Modify `apps/desktop/src/App.test.tsx`: provider settings UI behavior.

Docs:

- Update `docs/architecture/adr/0009-use-project-local-rollout-with-desktop-sqlite-index.md` only if provider status moves into SQLite.
- Create a new ADR if provider profiles become a durable cross-project data model.

---

## Task 1: Runtime Config and Environment Fallback

**Files:**
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `apps/desktop/src-tauri/tests/provider_settings.rs`

- [ ] **Step 1: Add failing tests for env fallback and Keychain precedence**

Add tests that assert:

```rust
#[tokio::test]
async fn runtime_config_uses_env_key_when_keychain_is_empty() {
    std::env::set_var("OPENAI_API_KEY", "sk-env");
    std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    std::env::set_var("OPENAI_MODEL", "env-model");

    let dir = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    let config = store.runtime_config().await.unwrap();

    assert_eq!(
        config.model.credential,
        ResolvedCredential::ApiKey("sk-env".to_string())
    );
    assert_eq!(
        config.model.endpoint.base_url.as_deref(),
        Some("https://env.example/v1")
    );
    assert_eq!(config.model.identity.model_id, "env-model");

    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");
}

#[tokio::test]
async fn runtime_config_prefers_keychain_key_over_env_key() {
    std::env::set_var("OPENAI_API_KEY", "sk-env");

    let dir = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let store = DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);

    store
        .save_provider_settings(ProviderSettingsSaveRequest {
            provider_id: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1".into(),
            api_key: Some("sk-keychain".into()),
            clear_api_key: false,
        })
        .await
        .unwrap();

    let config = store.runtime_config().await.unwrap();

    assert_eq!(
        config.model.credential,
        ResolvedCredential::ApiKey("sk-keychain".to_string())
    );

    std::env::remove_var("OPENAI_API_KEY");
}
```

Run:

```bash
cargo test -p exagent-desktop --test provider_settings
```

Expected: FAIL until fallback logic is implemented.

- [ ] **Step 2: Implement fallback without losing GUI precedence**

Change `DesktopSettingsStore::runtime_config()` so it starts from `AgentConfig::default()` and overlays saved settings only when a settings file exists. Credential selection order:

1. Keychain credential for the selected provider.
2. Environment credential from `AgentConfig::default()`.
3. `None`.

For default UI display, `load_provider_settings()` may still show provider defaults, but runtime config must not erase env values just because no settings file exists.

- [ ] **Step 3: Add credential source to the response**

Extend `ProviderConfigView` with:

```rust
pub credential_source: CredentialSource,
pub auth_required: bool,
```

Add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    Keychain,
    Environment,
    None,
}
```

The UI needs this to distinguish "connected through environment" from "connected through saved API key".

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_settings
cargo test
cargo fmt --check
```

Expected: all pass.

## Task 2: Optional Auth for OpenAI-Compatible Providers

**Files:**
- Modify: `src/model/llm.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `tests/openai_compatible_llm.rs`
- Test: `apps/desktop/src-tauri/tests/provider_settings.rs`

- [ ] **Step 1: Add failing tests for no-auth local gateway support**

Create a root-crate test with an Axum server that asserts no `Authorization` header is required:

```rust
#[tokio::test]
async fn openai_compatible_llm_can_call_gateway_without_api_key() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(|| async {
            axum::Json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "ok"
                    }
                }]
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let llm = OpenAiCompatibleLlm::from_parts("local-model", format!("http://{addr}/v1"), None)
        .unwrap();

    let completion = llm
        .complete(
            &[ConversationMessage::user("hello")],
            &[],
            &LlmRequestOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(completion.turn.text.as_deref(), Some("ok"));
}
```

Run:

```bash
cargo test --test openai_compatible_llm
```

Expected: FAIL until `from_parts` accepts optional auth.

- [ ] **Step 2: Implement optional bearer auth**

Change `OpenAiCompatibleLlm`:

```rust
api_key: Option<String>,
```

Change `from_config` and `from_parts` signatures to accept `Option<String>` for API key. Keep model and base URL validation. In `complete()`, build the request like:

```rust
let mut request_builder = self.client.post(&self.endpoint).json(&request);
if let Some(api_key) = self.api_key.as_deref().filter(|value| !value.trim().is_empty()) {
    request_builder = request_builder.bearer_auth(api_key);
}
let response = request_builder.send().await?;
```

- [ ] **Step 3: Mark auth mode per provider profile**

In `settings.rs`, define:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthMode {
    ApiKeyRequired,
    ApiKeyOptional,
    OAuthPlanned,
}
```

Set:

- `openai`: `ApiKeyRequired`
- `openai_compatible`: `ApiKeyOptional`
- `anthropic`: `ApiKeyRequired`, unsupported until adapter exists
- `google`: `ApiKeyRequired`, unsupported until adapter exists
- `github_copilot`: `OAuthPlanned`, unsupported until OAuth exists

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test openai_compatible_llm
cargo test -p exagent-desktop --test provider_settings
cargo test
```

Expected: all pass.

## Task 3: Provider Connection Test Command

**Files:**
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Test: `apps/desktop/src-tauri/tests/provider_connection.rs`

- [ ] **Step 1: Add test request and response types**

Define:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionTestRequest {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub use_saved_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
```

- [ ] **Step 2: Add failing command tests**

Cover:

- unsupported provider returns `unsupported_provider`
- OpenAI required auth with no GUI/keychain/env key returns `missing_credential`
- OpenAI-compatible optional auth can reach a local test server without key
- 401 maps to `authentication_failed`
- 404 or provider model error maps to `model_not_found` when the body indicates unknown model

Run:

```bash
cargo test -p exagent-desktop --test provider_connection
```

Expected: FAIL until the command exists.

- [ ] **Step 3: Implement provider testing**

Implement a `DesktopSettingsStore::test_provider_connection(request)` method. It should:

1. Resolve provider profile.
2. Reject unsupported providers before network calls.
3. Resolve credentials from form API key, saved Keychain key, then env key.
4. Enforce required credentials only for providers with `ApiKeyRequired`.
5. For OpenAI-compatible protocol, make a minimal Chat Completions probe using the same request shape as runtime.
6. Map errors into `ProviderConnectionTestResponse` without panicking.

- [ ] **Step 4: Register Tauri command**

Add:

```rust
#[tauri::command]
pub async fn provider_connection_test(
    state: State<'_, DesktopState>,
    request: ProviderConnectionTestRequest,
) -> CommandResult<ProviderConnectionTestResponse> {
    state
        .settings
        .test_provider_connection(request)
        .await
        .map_err(error_string)
}
```

Register it in `tauri::generate_handler!`.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_connection
cargo test -p exagent-desktop --test provider_settings
cargo test
cargo fmt --check
```

Expected: all pass.

## Task 4: Settings UI Connection Flow

**Files:**
- Modify: `apps/desktop/src/types.ts`
- Modify: `apps/desktop/src/api/exagentClient.ts`
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add frontend types and API client**

Add TypeScript equivalents for:

- `ProviderAuthMode`
- `CredentialSource`
- `ProviderConnectionTestRequest`
- `ProviderConnectionTestResponse`
- `ProviderConnectionStatus`

Add:

```ts
export async function testProviderConnection(
  request: ProviderConnectionTestRequest
): Promise<ProviderConnectionTestResponse> {
  if (!isTauriRuntime()) {
    return {
      status: "success",
      message: "Mock provider connection succeeded"
    };
  }
  return invokeCommand<ProviderConnectionTestResponse>("provider_connection_test", { request });
}
```

- [ ] **Step 2: Add failing UI tests**

Extend `App.test.tsx` to assert:

- Clicking `Connect OpenAI` opens/selects the provider form.
- The form has `Test connection` and `Save connection`.
- Unsupported providers show `Coming soon` and cannot be tested.
- Test success message renders after `Test connection`.

Run:

```bash
npm test --prefix apps/desktop
```

Expected: FAIL until UI changes exist.

- [ ] **Step 3: Update Settings UI**

Change labels and flow:

- List row button:
  - supported providers: `Configure`
  - unsupported providers: `Soon`
- Form submit button: `Save connection`
- Add secondary button: `Test connection`
- Show connection result inline near the buttons.
- Show credential source:
  - `Using Keychain credential`
  - `Using environment credential`
  - `No credential saved`
  - `API key optional for this provider`
- For `openai_compatible`, allow an empty API key.
- For `openai`, warn if no key is saved or available from env.

- [ ] **Step 4: Verify**

Run:

```bash
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass.

## Task 5: Persist Last Connection Status

**Files:**
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src/types.ts`
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src-tauri/tests/provider_connection.rs`
- Test: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Extend settings file carefully**

Add optional status fields to `SettingsFile`:

```rust
last_connection_status: Option<ProviderConnectionStatus>,
last_connection_message: Option<String>,
last_connection_checked_at: Option<String>,
```

Use `#[serde(default)]` so existing `settings.json` files keep loading.

- [ ] **Step 2: Save status after test**

When `test_provider_connection` runs for the active provider, persist the returned status and timestamp. Do not persist API keys as part of testing.

- [ ] **Step 3: Display saved status**

In the Settings page, show:

- last success/failure status
- last checked timestamp
- latest error message if present

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_connection
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 6: Provider Profiles

**Files:**
- Create: `src/model/provider.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src/types.ts`
- Test: `apps/desktop/src-tauri/tests/provider_settings.rs`

- [x] **Step 1: Define provider profile model**

Use one profile shape for backend and serialized UI DTOs:

```rust
pub struct ProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub protocol: ProviderProtocol,
    pub auth_mode: ProviderAuthMode,
    pub default_base_url: &'static str,
    pub default_model: &'static str,
    pub supports_model_discovery: bool,
    pub supports_tools: bool,
    pub supported: bool,
    pub unsupported_reason: Option<&'static str>,
}
```

Protocols:

```rust
pub enum ProviderProtocol {
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    CopilotOAuth,
}
```

- [x] **Step 2: Replace ad hoc catalog construction**

Replace repeated provider descriptor construction with profile-driven DTO mapping.

- [x] **Step 3: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_settings
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 7: Model Discovery

**Files:**
- Modify: `src/model/llm.rs` or create `src/model/openai_compatible.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src-tauri/tests/provider_models.rs`
- Test: `apps/desktop/src/App.test.tsx`

- [x] **Step 1: Add backend DTOs**

```rust
pub struct ProviderModelListRequest {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub use_saved_api_key: bool,
}

pub struct ProviderModelView {
    pub id: String,
    pub display_name: String,
    pub context_window: Option<i64>,
    pub supports_tools: Option<bool>,
}
```

- [x] **Step 2: Implement OpenAI-compatible `/models` listing**

Call `{base_url}/models`. Map returned IDs into `ProviderModelView`. If `/models` is unsupported, return a typed "model discovery unavailable" response and keep manual model entry enabled.

- [x] **Step 3: Add UI model selector**

Use a manual input by default. When model discovery succeeds, show a selectable list and still allow manual override.

- [x] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_models
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass.

## Task 8: Anthropic Adapter

**Files:**
- Create: `src/model/anthropic.rs`
- Modify: `src/model/mod.rs` or current exports
- Modify: `src/app_server/thread_manager.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `tests/anthropic_llm.rs`

- [ ] **Step 1: Add Anthropic request/response mapping tests**

Tests must cover:

- user/system/assistant message mapping
- text completion parsing
- tool call parsing
- tool result message mapping
- HTTP error mapping

- [ ] **Step 2: Implement `AnthropicLlm`**

Implement `LlmClient` against `/v1/messages`. Keep it isolated from OpenAI-compatible request types.

- [ ] **Step 3: Dispatch by provider protocol**

`ThreadManager` or an injected LLM factory should select:

- `OpenAiCompatibleLlm` for OpenAI-compatible protocol
- `AnthropicLlm` for Anthropic protocol

- [ ] **Step 4: Enable Anthropic in catalog**

Change `anthropic.supported` to `true` only after adapter tests pass.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test --test anthropic_llm
cargo test
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 9: Multiple Credentials and Provider Profiles

**Files:**
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Consider: `src/index_db` or app data SQLite if profile state should be queryable
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src-tauri/tests/provider_settings.rs`

- [ ] **Step 1: Define durable profile record**

Profile records should include:

```rust
pub struct SavedProviderProfile {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    pub model: String,
    pub credential_account: String,
    pub created_at: String,
    pub updated_at: String,
}
```

- [ ] **Step 2: Store secrets per profile**

Use Keychain account IDs like:

```text
provider:<provider_id>:profile:<profile_id>
```

Do not overwrite provider-level credentials when adding a second profile.

- [ ] **Step 3: Add profile selector UI**

Settings should support:

- active profile
- add profile
- rename profile
- delete profile
- choose profile for new sessions

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_settings
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 10: OAuth Providers

**Files:**
- Create: `apps/desktop/src-tauri/src/oauth.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src-tauri/tests/provider_oauth.rs`

- [ ] **Step 1: Define OAuth state model**

Represent:

- pending auth request
- provider id
- PKCE verifier
- callback result
- encrypted/Keychain token storage account

- [ ] **Step 2: Implement OAuth start command**

Command returns an authorization URL and opaque state ID. The frontend opens the system browser or a Tauri webview.

- [ ] **Step 3: Implement OAuth callback handling**

Use a local callback server or platform callback URI. Store refresh/access tokens in Keychain.

- [ ] **Step 4: Add Copilot or account-based provider only after protocol research**

Do not enable `github_copilot` until auth and request protocol are verified against real provider behavior.

- [ ] **Step 5: Verify**

Run provider OAuth unit tests with mocked token endpoints. Do not require real account credentials in CI.

## Implementation Order

1. Task 1: env fallback and credential source.
2. Task 2: optional auth for OpenAI-compatible local gateways.
3. Task 3: backend `Test connection`.
4. Task 4: Settings UI connection flow.
5. Task 5: persisted connection status.
6. Task 6: provider profiles.
7. Task 7: model discovery.
8. Task 8: Anthropic adapter.
9. Task 9: multiple credentials/profiles.
10. Task 10: OAuth providers.

## Verification Gates

Run after Phase 1:

```bash
cargo fmt --check
cargo test
cargo test -p exagent-desktop --test provider_settings
cargo test -p exagent-desktop --test provider_connection
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
npm run tauri:build --prefix apps/desktop -- --debug
git diff --check
```

Manual acceptance:

- Start desktop with only env credentials set; run a real turn.
- Start desktop with no env credentials; save GUI key; run a real turn.
- Configure a local OpenAI-compatible gateway without key; test connection succeeds and a turn can run.
- Configure OpenAI without key; test connection returns a missing credential message before runtime turn execution.
- Try Anthropic; UI shows unsupported/coming soon and does not attempt a network request.

## Risks and Tradeoffs

- Allowing no API key in `OpenAiCompatibleLlm` makes official OpenAI missing-key failures happen at HTTP time unless Settings preflights them. This is acceptable because provider profiles enforce required auth before testing and saving.
- Chat Completions probing costs a tiny amount of tokens. A `/models` probe is cheaper but less representative of the real runtime path. Prefer runtime-path probing for `Test connection`; add `/models` discovery separately.
- Persisting provider status in JSON is fine for one active profile. Move it to SQLite when profile count and query needs grow.
- OAuth should not be squeezed into the API-key form. It needs a separate account connection flow and token lifecycle model.

## Done Means

Phase 1 is done when a user can open Settings, configure OpenAI or an OpenAI-compatible endpoint, test it before saving, save it, see whether the credential came from Keychain or env, and run a real desktop session without relying on hidden terminal setup.

The full provider platform is done when provider-specific adapters, model discovery, multiple profiles, persisted health state, and OAuth providers are implemented behind stable provider profile metadata.
