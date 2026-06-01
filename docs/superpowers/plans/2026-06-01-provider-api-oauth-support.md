# Provider API and OAuth Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real provider-native API support and account/OAuth connection flows while preserving the current Rust runtime boundary and Keychain-based desktop credential storage.

**Architecture:** API-key providers use provider-native Rust `LlmClient` adapters selected by provider profile metadata. OAuth/account providers use a desktop auth module that owns PKCE, callback handling, token refresh, Keychain storage, and profile status; runtime adapters receive only resolved tokens or API keys.

**Tech Stack:** Rust, Tauri v2 commands, `reqwest`, `serde`, Keychain through `keyring`, React, TypeScript, Tailwind CSS, Radix/shadcn primitives, Vitest, Tokio tests, Axum local callback/test servers.

---

## Current Provider Facts

- OpenAI Platform API authentication uses API keys sent as bearer tokens: `https://platform.openai.com/docs/api-reference/authentication`.
- OpenAI Codex has a separate ChatGPT sign-in flow that creates local credentials and distinguishes the OAuth grant from generated API keys: `https://help.openai.com/en/articles/11381614-api-codex-cli-and-sign-in-with-chatgpt`.
- Anthropic direct API authentication supports API keys via `x-api-key` and Workload Identity Federation: `https://platform.claude.com/docs/en/api/authentication/overview`.
- Gemini API can use API keys for the simplest path and OAuth when stricter access controls are needed: `https://ai.google.dev/gemini-api/docs/oauth`.
- GitHub Copilot SDK supports GitHub signed-in user, OAuth GitHub App, environment variables, and BYOK modes, but the SDK is currently a technical preview: `https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/authenticate`.

## Acceptance Criteria

- Provider profiles describe available connection methods instead of hardcoding OpenAI, Copilot, and API-key pages in React conditionals.
- Anthropic can be enabled only after a native Messages adapter passes text, tool-call, tool-result, and error mapping tests.
- Gemini can be enabled only after a native Generate Content adapter passes text, tool-call, tool-result, safety/error, and model discovery tests.
- OAuth flows store refresh/access tokens in Keychain and persist only non-secret profile metadata in desktop settings.
- OpenAI ChatGPT/Codex sign-in, Google OAuth, and GitHub Copilot OAuth use the same desktop OAuth lifecycle, but each provider owns its authorization URLs, scopes, token exchange, refresh, and runtime token materialization.
- OAuth/account providers are not marked `supported: true` until a real runtime adapter and credential refresh path exist.

## File Structure

- Modify `src/model/provider.rs`: provider profile, connection method, auth kind, protocol capability metadata.
- Create `src/model/anthropic.rs`: Anthropic Messages API adapter.
- Create `src/model/gemini.rs`: Gemini Generate Content API adapter.
- Modify `src/model/mod.rs`: export provider-native adapters.
- Modify `src/app_server/thread_manager.rs`: construct the adapter selected by provider protocol.
- Modify `apps/desktop/src-tauri/src/settings.rs`: save provider profile metadata and materialize runtime config.
- Create `apps/desktop/src-tauri/src/oauth.rs`: PKCE, callback server, token exchange, refresh, revoke, Keychain accounts.
- Modify `apps/desktop/src-tauri/src/commands.rs`: OAuth start/callback/status/revoke commands.
- Modify `apps/desktop/src-tauri/src/lib.rs`: register OAuth commands.
- Modify `apps/desktop/src/types.ts`: provider connection methods and OAuth command DTOs.
- Modify `apps/desktop/src/components/SettingsDialog.tsx`: render connection pages from method metadata.
- Add tests in `tests/anthropic_llm.rs`, `tests/gemini_llm.rs`, `apps/desktop/src-tauri/tests/provider_oauth.rs`, `apps/desktop/src/App.test.tsx`.

---

## Task 1: Provider Connection Method Metadata

**Files:**
- Modify: `src/model/provider.rs`
- Modify: `apps/desktop/src/types.ts`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `apps/desktop/src-tauri/tests/provider_settings.rs`
- Test: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add failing metadata tests**

Add backend assertions that every provider exposes at least one connection method:

```rust
#[test]
fn provider_profiles_expose_connection_methods() {
    let profiles = provider_profiles();

    let openai = profiles.iter().find(|profile| profile.id == "openai").unwrap();
    assert!(openai.connection_methods.iter().any(|method| method.id == "api_key"));
    assert!(openai.connection_methods.iter().any(|method| method.id == "chatgpt_browser"));

    let anthropic = profiles.iter().find(|profile| profile.id == "anthropic").unwrap();
    assert_eq!(anthropic.connection_methods[0].id, "api_key");

    let github = profiles.iter().find(|profile| profile.id == "github_copilot").unwrap();
    assert!(github.connection_methods.iter().any(|method| method.id == "github_oauth"));
}
```

Run:

```bash
cargo test -p exagent-desktop --test provider_settings provider_profiles_expose_connection_methods
```

Expected: FAIL until connection method metadata exists.

- [ ] **Step 2: Add metadata types**

Add these profile fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConnectionKind {
    ApiKey,
    OAuthPkce,
    OAuthDevice,
    ExternalAccount,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConnectionMethod {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: ProviderConnectionKind,
    pub enabled: bool,
    pub disabled_reason: Option<&'static str>,
}
```

Populate initial methods:

- OpenAI: `chatgpt_browser`, `chatgpt_headless`, `api_key`; only `api_key` is enabled until ChatGPT/Codex auth is implemented.
- OpenAI Compatible: `api_key`; enabled and optional.
- Anthropic: `api_key`; disabled until `AnthropicLlm` exists.
- Google: `api_key`, `google_oauth`; disabled until `GeminiLlm` and OAuth exist.
- GitHub Copilot: `github_oauth`; disabled until Copilot auth and runtime protocol are verified.

- [ ] **Step 3: Render UI from metadata**

Update `SettingsDialog.tsx` so provider-specific pages map over `provider.connection_methods`. Keep provider-specific detail panels only for method bodies:

```ts
type ProviderConnectionMethod = {
  id: string;
  label: string;
  description: string;
  kind: "api_key" | "oauth_pkce" | "oauth_device" | "external_account";
  enabled: boolean;
  disabled_reason: string | null;
};
```

UI tests should assert that OpenAI, Google, and GitHub Copilot render choices from metadata rather than hardcoded branches.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_settings
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass.

## Task 2: Anthropic Messages Adapter

**Files:**
- Create: `src/model/anthropic.rs`
- Modify: `src/model/mod.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `tests/anthropic_llm.rs`

- [ ] **Step 1: Write adapter mapping tests**

Create tests that cover:

- system prompt mapping to Anthropic `system`
- user and assistant text messages
- tool calls emitted from `content[].type == "tool_use"`
- tool results emitted as `tool_result`
- `x-api-key`, `anthropic-version`, and `content-type` headers
- 401 and 429 error mapping

Run:

```bash
cargo test --test anthropic_llm
```

Expected: FAIL until `AnthropicLlm` exists.

- [ ] **Step 2: Implement `AnthropicLlm`**

Implement `LlmClient` against `POST /v1/messages` using `reqwest`. Keep request/response DTOs private inside `src/model/anthropic.rs`. Set the required headers:

```rust
request_builder
    .header("x-api-key", api_key)
    .header("anthropic-version", "2023-06-01")
    .header("content-type", "application/json");
```

- [ ] **Step 3: Enable Anthropic after tests**

Change the Anthropic profile to `supported: true` only after adapter tests and desktop connection tests pass.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test anthropic_llm
cargo test
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 3: Gemini Generate Content Adapter

**Files:**
- Create: `src/model/gemini.rs`
- Modify: `src/model/mod.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `tests/gemini_llm.rs`

- [ ] **Step 1: Write adapter tests**

Create tests that cover:

- user and assistant content mapping
- tool declaration mapping
- function call parsing
- function response mapping
- safety block and quota error messages
- API-key query/header materialization

Run:

```bash
cargo test --test gemini_llm
```

Expected: FAIL until `GeminiLlm` exists.

- [ ] **Step 2: Implement `GeminiLlm`**

Implement `LlmClient` against Gemini Generate Content with private DTOs in `src/model/gemini.rs`. Keep OAuth and API-key credential resolution outside the adapter; the adapter receives a resolved credential object.

- [ ] **Step 3: Enable Gemini API-key path**

Enable Google only for the API-key method first. Keep `google_oauth` disabled until Task 5 is complete.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test --test gemini_llm
cargo test
npm test --prefix apps/desktop
```

Expected: all pass.

## Task 4: Desktop OAuth Core

**Files:**
- Create: `apps/desktop/src-tauri/src/oauth.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Test: `apps/desktop/src-tauri/tests/provider_oauth.rs`

- [ ] **Step 1: Add OAuth DTOs and failing tests**

Add DTOs:

```rust
pub struct ProviderOAuthStartRequest {
    pub provider_id: String,
    pub method_id: String,
}

pub struct ProviderOAuthStartResponse {
    pub state_id: String,
    pub authorization_url: String,
}

pub struct ProviderOAuthStatusResponse {
    pub provider_id: String,
    pub method_id: String,
    pub connected: bool,
    pub account_label: Option<String>,
    pub expires_at: Option<String>,
}
```

Tests should cover:

- state IDs are unguessable
- PKCE verifier is stored only in memory during pending auth
- callback with wrong state is rejected
- successful token exchange stores tokens in Keychain
- refresh updates Keychain without changing profile metadata
- revoke removes Keychain tokens and marks the profile disconnected

Run:

```bash
cargo test -p exagent-desktop --test provider_oauth
```

Expected: FAIL until OAuth core exists.

- [ ] **Step 2: Implement PKCE and callback server**

Implement:

- `start_provider_oauth(request)` returning `authorization_url`
- a loopback callback listener bound to `127.0.0.1:0`
- PKCE verifier/challenge generation
- one pending auth record per state ID
- provider-specific token endpoint configuration through provider metadata

- [ ] **Step 3: Store tokens in Keychain**

Use Keychain accounts shaped as:

```text
provider:<provider_id>:method:<method_id>:profile:<profile_id>:oauth
```

Persist only:

- provider id
- method id
- profile id
- account label
- token expiry timestamp
- connection status

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_oauth
cargo test -p exagent-desktop --test provider_settings
```

Expected: all pass.

## Task 5: Provider-Specific OAuth Enablement

**Files:**
- Modify: `apps/desktop/src-tauri/src/oauth.rs`
- Modify: `apps/desktop/src-tauri/src/settings.rs`
- Modify: `apps/desktop/src/components/SettingsDialog.tsx`
- Test: `apps/desktop/src-tauri/tests/provider_oauth.rs`
- Test: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Enable Google OAuth after Gemini adapter**

Implement Google desktop OAuth config with scopes required for Gemini API access. Tests use a mocked authorization server and token endpoint; no real Google account is required.

- [ ] **Step 2: Add OpenAI ChatGPT/Codex sign-in research gate**

Keep OpenAI `chatgpt_browser` and `chatgpt_headless` disabled until the implementation can reproduce the supported Codex sign-in flow without relying on browser session scraping. The desktop flow must store only supported credentials and must document how disconnect and generated API key revocation work.

- [ ] **Step 3: Add GitHub Copilot OAuth research gate**

Keep GitHub Copilot disabled until the runtime path is selected:

- Option A: Rust HTTP adapter, only if the protocol is documented and stable enough.
- Option B: small sidecar using the official Copilot SDK, only if the operational cost is acceptable.

Do not enable Copilot in catalog until tests prove auth, token refresh, request execution, and error mapping.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p exagent-desktop --test provider_oauth
cargo test
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
```

Expected: all pass.

## Implementation Order

1. Task 1: provider connection method metadata.
2. Task 2: Anthropic adapter.
3. Task 3: Gemini API-key adapter.
4. Task 4: desktop OAuth core.
5. Task 5: Google OAuth, then OpenAI ChatGPT/Codex and GitHub Copilot gates.

## Verification Gate

Run before marking the feature complete:

```bash
cargo fmt --check
cargo test
cargo test -p exagent-desktop --test provider_settings
cargo test -p exagent-desktop --test provider_oauth
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
npm run tauri:build --prefix apps/desktop -- --debug
git diff --check
```

## Done Means

Provider API support is complete when OpenAI-compatible, Anthropic, and Gemini can each run a real turn through the Rust runtime with their own adapter tests. OAuth support is complete when at least one OAuth provider can connect, refresh, disconnect, and run a real turn without copying an API key into the UI.
