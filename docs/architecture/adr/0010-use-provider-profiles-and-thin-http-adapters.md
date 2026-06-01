# ADR-0010: Use Provider Profiles, Resolved Model Config, and Thin HTTP Adapters

## Status

Accepted

## Context

ExAgent Desktop needs a provider setup flow that works for local daily use and can grow beyond one OpenAI-compatible endpoint.

The runtime is written in Rust and already abstracts model calls behind `LlmClient`. The current OpenAI-compatible adapter is a small `reqwest` client with private request/response DTOs. Provider setup now needs additional behavior: environment fallback, Keychain precedence, optional API keys for local gateways, connection testing, provider-specific error mapping, future model discovery, native Anthropic/Gemini adapters, and OAuth providers.

The codebase already has the start of this boundary:

- `src/model/provider.rs` owns `ProviderProfile`, `ProviderProtocol`, `ProviderAuthMode`, and the static `PROVIDER_PROFILES` catalog.
- `src/runtime/agent.rs` already depends on `Box<dyn LlmClient>`, so the Agent does not need to know provider details.
- `src/app_server/thread_manager.rs` has a private `LlmFactory` and a `SharedLlmFactory` test seam, but the default factory still hardcodes `OpenAiCompatibleLlm`.
- `src/config.rs` still stores `model: String`, `openai_base_url`, `openai_api_key`, and `model_context_window`, so provider state is split across provider metadata, runtime config, and desktop settings.

Official SDK availability does not line up with the runtime language. OpenAI documents official SDKs for JavaScript, Python, .NET, Java, Go, Ruby, and CLI, while Rust is listed under community libraries. Anthropic documents official SDKs for Python, TypeScript, Java, Go, Ruby, C#, PHP, and CLI. Google GenAI documents official SDKs for Python, JavaScript/TypeScript, Go, Java, and C#. There is no official Rust SDK path across these providers.

Adding a Node.js or Go sidecar only to use official SDKs would make the desktop app heavier, add process lifecycle complexity, and split runtime behavior across languages.

## Decision

Represent providers as structured provider profiles:

- provider id
- protocol
- auth mode
- default base URL
- default model
- model discovery support
- tool support
- support status
- unsupported reason

`src/model/provider.rs` is the single source of truth for static provider profile data. Do not add a second profile catalog in another module. A future `catalog` or resolver module may validate and materialize settings, but it must not duplicate `PROVIDER_PROFILES`.

Represent the concrete model used by a thread as a resolved runtime value:

- `ResolvedModelConfig`
- `ModelRef`
- `ProviderEndpoint`
- `ResolvedCredential`
- `ModelCapabilities`

`ModelRef` is the durable model identity. It contains only `provider_id` and `model_id`. The same type is used in API protocol payloads, `ResolvedModelConfig.identity`, and persisted turn context. Do not introduce a separate `ModelIdentity` type with the same fields.

`AgentConfig` remains the runtime's top-level configuration, but its model fields collapse into `AgentConfig.model: ResolvedModelConfig`. The runtime should keep receiving `AgentConfig` because policy, workspace, timeouts, thinking mode, and compaction all still belong to one turn/session config.

`TurnStartParams` and `TurnContextOverrides` carry only `ModelRef`. They must never carry `ResolvedModelConfig`, API keys, OAuth tokens, provider endpoints with secrets, or other credential material.

Move model materialization behind an injected async resolver:

```rust
#[async_trait]
pub trait ModelResolver: Send + Sync {
    async fn resolve(&self, model_ref: &ModelRef) -> anyhow::Result<ResolvedModelConfig>;
}
```

`ThreadManager` owns `Arc<dyn ModelResolver>`. It resolves `ModelRef` into runtime-only `ResolvedModelConfig` in the async app-server path before submitting a user turn to the runtime actor. This keeps blocking or host-specific credential lookup out of the actor loop and out of core runtime internals.

Core ships an environment-backed resolver for CLI/API/server tests. Desktop ships a Keychain-backed resolver that uses desktop settings, Keychain, and environment fallback. Tests may use static resolvers. The resolver owns the full model materialization step: provider protocol, endpoint, credential, and capabilities.

Move provider-specific LLM construction behind a shared factory:

```rust
pub trait LlmClientFactory: Send + Sync {
    fn build(&self, model: &ResolvedModelConfig) -> anyhow::Result<Box<dyn LlmClient>>;
}
```

`ThreadManager` calls the factory with `&config.model` when creating or rebuilding an `Agent`. It must not switch on provider protocol itself. `ThreadManager::with_llm` and the shared mock factory stay available for tests.

Keep runtime model calls in Rust behind `LlmClient`. `Agent` only knows `Box<dyn LlmClient>`.

Use thin Rust HTTP adapters with `reqwest` for provider protocols:

- OpenAI-compatible Chat Completions for Phase 1.
- Native Anthropic Messages adapter when Anthropic support is enabled.
- Native Gemini Generate Content adapter when Gemini support is enabled.
- Separate OAuth/account connection flow before enabling Copilot or account-based providers.

Use official SDKs and official docs as reference material, but do not add unofficial Rust SDKs to the runtime path for Phase 1.

Move OpenAI-specific process environment handling out of `AgentConfig::default()`. Default config must remain usable for tests without reading `OPENAI_*`. Environment materialization is explicit:

- Desktop uses a Keychain-backed `ModelResolver`; `apps/desktop/src-tauri/src/settings.rs::runtime_config()` resolves the default `ModelRef` through that same resolver.
- CLI/API entrypoints use the core environment-backed `ModelResolver` to convert process env and model refs into `ResolvedModelConfig`.

API keys and OAuth tokens must not be persisted in SQLite or JSON. Secrets stay in Keychain or process environment. `ResolvedCredential` must use redacting `Debug` output.

Provider/model switching is allowed only between user turns. A running user turn freezes the resolved model config for the whole agent loop. If the GUI selects a new `ModelRef` while a turn is running, that selection affects only a later turn after the current turn completes or is interrupted. At the next `turn_start`, `ThreadManager` resolves the selected `ModelRef`; if provider, protocol, endpoint, credential, or capabilities changed, the runtime rebuilds `Agent`/`LlmClient` before the turn starts.

## Consequences

- ExAgent keeps one runtime language and one desktop process model.
- Provider-specific request mapping remains explicit and testable.
- We avoid depending on community SDK correctness or release cadence for core runtime behavior.
- We must maintain request/response DTOs, status mapping, retries, streaming support, and feature flags ourselves.
- Official SDK repositories remain useful as reference implementations for edge cases.
- If an official Rust SDK appears later and proves mature, this ADR can be revisited.
- `AgentConfig::default()` no longer makes a production runtime ready by itself. Production paths must call the explicit desktop or CLI/API bootstrap.
- Desktop `runtime_config()` and its tests must change in the same implementation slice as the `AgentConfig` field migration; otherwise the workspace will not compile.
- Rollout, event, and token usage schemas keep storing historical `model_context_window` values. Only configuration lookup moves to `config.model.capabilities.context_window`.
- Rollout and event replay persist only `ModelRef`; they never persist `ResolvedCredential`.
- Provider switching between user turns requires rebuilding the `Agent`/`LlmClient` when the resolved runtime model changes.

## Non-Goals

- Do not implement SQLite-backed provider settings in Phase 1.
- Do not implement provider plugin hot loading in Phase 1.
- Do not implement in-flight provider switching inside a running user turn.
- Do not implement Anthropic, Gemini, OpenAI ChatGPT sign-in, Google OAuth, or GitHub Copilot OAuth in the core refactor slice.
- Do not move API keys or OAuth tokens into rollout JSONL, SQLite, or desktop settings JSON.

## Affected Modules

- `src/model/llm.rs`
- `src/model/provider.rs`
- `src/model/resolved.rs`
- `src/model/resolver.rs`
- `src/model/factory.rs`
- `src/model/openai_compatible.rs`
- future `src/model/anthropic.rs`
- future `src/model/gemini.rs`
- `src/config.rs`
- `src/app_server/thread_manager.rs`
- `src/app_server/protocol.rs`
- `src/app_server/service.rs`
- `src/entrypoints/api.rs`
- `src/entrypoints/cli_adapter.rs`
- `apps/desktop/src-tauri/src/settings.rs`
- `apps/desktop/src-tauri/src/commands.rs`
- `apps/desktop/src/components/SettingsDialog.tsx`
- `apps/desktop/src/components/Composer.tsx`
- `apps/desktop/src/stores/workbenchStore.ts`
- `apps/desktop/src/api/exagentClient.ts`
- `tests/llm_http.rs`
- `apps/desktop/src-tauri/tests/provider_settings.rs`

## Related Docs

- `docs/superpowers/specs/2026-06-01-provider-onboarding.md`
- `docs/superpowers/plans/2026-06-01-provider-onboarding.md`
- `docs/superpowers/plans/2026-06-01-provider-runtime-core-refactor.md`
- OpenAI SDKs and CLI: https://platform.openai.com/docs/libraries
- Anthropic Client SDKs: https://platform.claude.com/docs/en/api/client-sdks
- Gemini API libraries: https://ai.google.dev/gemini-api/docs/libraries
