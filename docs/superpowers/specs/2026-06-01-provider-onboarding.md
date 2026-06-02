# Provider Onboarding Specification

## Goal

Make ExAgent Desktop provider setup explicit, testable, and durable enough for daily use while keeping runtime provider logic isolated behind `LlmClient`.

The first supported path is OpenAI-compatible chat completions because the current runtime already speaks that protocol. The design must still leave clean room for provider-native Anthropic, Gemini, and OAuth/account providers.

## Current Behavior

- `AgentConfig::default()` reads `OPENAI_MODEL`, `OPENAI_BASE_URL`, and `OPENAI_API_KEY`.
- Desktop Settings can save provider fields and API keys, but the connection is not tested before use.
- Saved desktop settings can shadow environment credentials when no Keychain credential exists.
- `OpenAI` and `OpenAI Compatible` share the same OpenAI-compatible adapter.
- Anthropic, Google, and GitHub Copilot are visible as unsupported catalog entries.

## Product Requirements

- Users can open Settings > Providers and understand whether ExAgent is connected through Keychain, environment variables, or no credential.
- Users can test provider connectivity before saving.
- Users can save a provider connection and run a real desktop session without starting or configuring a separate terminal.
- Users can configure a local OpenAI-compatible gateway without an API key.
- Unsupported providers are clearly marked as unavailable and cannot start a fake connection flow.
- Provider errors are shown in the Settings page with actionable messages.

## Non-Goals

- No OAuth provider implementation in Phase 1.
- No Anthropic/Gemini native runtime adapter in Phase 1.
- No multi-profile credential manager in Phase 1.
- No provider marketplace or plugin system.
- No Node.js sidecar solely to use JavaScript SDKs.

## Provider Model

Provider metadata should be represented as structured profile/catalog data:

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

Provider protocols:

- `OpenAiChatCompletions`
- `AnthropicMessages`
- `GeminiGenerateContent`
- `CopilotOAuth`

Auth modes:

- `ApiKeyRequired`
- `ApiKeyOptional`
- `OAuthPlanned`

Initial catalog:

| Provider | Protocol | Auth | Phase 1 status |
| --- | --- | --- | --- |
| OpenAI | OpenAI Chat Completions compatible | API key required | Supported |
| OpenAI Compatible | OpenAI Chat Completions compatible | API key optional | Supported |
| Anthropic | Anthropic Messages | API key required | Coming soon |
| Google | Gemini Generate Content | API key required | Coming soon |
| GitHub Copilot | Account/OAuth | OAuth planned | Coming soon |

## Credential Resolution

Runtime credential resolution order:

1. Current form value for connection tests.
2. Keychain value for saved runtime execution.
3. Environment value from `AgentConfig::default()`.
4. No credential.

GUI-saved Keychain credentials must take precedence over environment credentials. Environment credentials must remain usable when no GUI credential is saved.

Credential source values exposed to the UI:

- `keychain`
- `environment`
- `none`

## Connection Test

Settings must expose a `Test connection` action. It uses current form values and does not require saving first.

Request:

```rust
pub struct ProviderConnectionTestRequest {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub use_saved_api_key: bool,
}
```

Response:

```rust
pub enum ProviderConnectionStatus {
    Success,
    UnsupportedProvider,
    MissingCredential,
    AuthenticationFailed,
    ModelNotFound,
    NetworkError,
    ProviderError,
}

pub struct ProviderConnectionTestResponse {
    pub status: ProviderConnectionStatus,
    pub message: String,
}
```

OpenAI-compatible testing should use the runtime path shape, not only `/models`, because it validates auth, base URL, model, JSON shape, and chat completion compatibility together. Model discovery can be added separately.

## UI Behavior

Provider list:

- Supported providers show `Configure`.
- Unsupported providers show `Soon`.
- Selecting a supported provider opens/focuses its form.
- Unsupported providers display the unsupported reason without attempting network calls.

Provider form:

- Primary action: `Save connection`.
- Secondary action: `Test connection`.
- Show credential source.
- Show API key optional state for OpenAI Compatible.
- Show last connection result after testing.
- Disable save when a provider is unsupported.

## Runtime Adapter Strategy

The Rust runtime owns provider calls. The frontend must not call model provider APIs directly.

OpenAI-compatible support remains a thin Rust HTTP adapter using `reqwest` and private request/response DTOs. This avoids a Node.js sidecar, avoids unofficial Rust SDK dependency risk, and keeps provider-specific request mapping inside the model module.

Provider-native adapters should be added when a provider's protocol diverges enough that compatibility mode is insufficient:

- Anthropic: native Messages adapter.
- Gemini: native Generate Content adapter, unless OpenAI compatibility is sufficient for the first slice.
- Copilot/ChatGPT account providers: separate OAuth/account flow before runtime adapter enablement.

## SDK Decision

Official SDKs should be used as protocol references and test-case references, but should not be introduced into the Rust runtime unless there is an official, maintained Rust SDK for that provider.

Current official SDK landscape checked on 2026-06-01:

- OpenAI lists official SDKs for JavaScript, Python, .NET, Java, Go, Ruby, and CLI. Rust is listed under community libraries, not official libraries.
- Anthropic lists official SDKs for Python, TypeScript, Java, Go, Ruby, C#, PHP, and CLI. Rust is not listed.
- Google GenAI SDK is available for Python, JavaScript/TypeScript, Go, Java, and C#. Rust is not listed.

Decision:

- Use `reqwest` + typed DTOs in Rust runtime.
- Keep provider request/response types private to each adapter.
- Add small shared helpers for HTTP status/error normalization.
- Use official SDK repositories as reference material when implementing provider-native adapters.
- Accept external reference repos from the user only as examples; do not copy architecture blindly.

Official references:

- OpenAI SDKs and CLI: https://platform.openai.com/docs/libraries
- Anthropic Client SDKs: https://platform.claude.com/docs/en/api/client-sdks
- Gemini API libraries: https://ai.google.dev/gemini-api/docs/libraries

## Persistence

Phase 1 can keep provider settings in app data `settings.json` and secrets in Keychain.

Persisted non-secret data:

- provider id
- base URL
- model
- last connection status
- last connection message
- last connection checked timestamp

Persisted secret data:

- API key in Keychain, keyed by provider id in Phase 1.

Future multi-profile support may move non-secret provider profiles to SQLite if listing, searching, per-project selection, or status queries become necessary.

## Acceptance Criteria

- Desktop can run a session using only environment credentials.
- Desktop can run a session using GUI-saved Keychain credentials.
- GUI-saved credentials win over environment credentials.
- OpenAI Compatible can test and save with no API key.
- OpenAI shows missing credential before saving/testing when neither Keychain nor env key exists.
- Unsupported providers cannot be saved or tested.
- Connection test statuses are displayed in Settings.
- `cargo test`, `cargo fmt --check`, `npm test --prefix apps/desktop`, `npm run build --prefix apps/desktop`, and `npm run tauri:build --prefix apps/desktop -- --debug` pass.

## Related Plan

- `docs/superpowers/plans/2026-06-01-provider-runtime-core-refactor.md`
- `docs/superpowers/plans/2026-06-01-provider-onboarding.md`
