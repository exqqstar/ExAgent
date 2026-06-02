# Model Module

## Responsibility

The model module defines LLM-facing data, provider profiles, resolved runtime
model config, provider adapters, and the factory used to build `LlmClient`
instances.

It does not decide runtime flow, tool execution policy, approval handling, or
rollout persistence.

## State

The module owns data definitions and adapter construction helpers, not durable
runtime state.

Important values:

- `ConversationMessage`
- `AssistantTurn`
- `LlmCompletion`
- `ToolCall`
- `ToolResult`
- `TokenUsage`
- `ProviderProfile`
- `ModelRef`
- `ResolvedModelConfig`
- `ResolvedCredential`

## File Map

- `src/model/types.rs`: core message, id, tool, completion, and token usage types.
- `src/model/provider.rs`: static provider profile catalog and protocol metadata.
- `src/model/resolved.rs`: durable `ModelRef` and runtime-only resolved model config.
- `src/model/resolver.rs`: async model resolver trait plus env-backed resolver.
- `src/model/factory.rs`: provider-protocol factory for `Box<dyn LlmClient>`.
- `src/model/llm.rs`: `LlmClient`, `MockLlm`, and compatibility exports.
- `src/model/openai_compatible.rs`: OpenAI-compatible chat-completions adapter.

## Key Flows

1. Protocol and persisted turn context carry `ModelRef { provider_id, model_id }`.
2. `ThreadManager` resolves `ModelRef` into `ResolvedModelConfig` before runtime submission.
3. `ThreadSession` rebuilds the `Agent`/`LlmClient` before a user turn if the resolved model changed.
4. Runtime builds `Vec<ConversationMessage>`.
5. `Agent` calls `LlmClient::complete`.
6. The selected adapter maps provider JSON into `LlmCompletion`.

## Connections

- `config::AgentConfig.model` stores `ResolvedModelConfig`.
- `app_server::protocol` carries `ModelRef`, never credentials.
- `app_server::thread_manager` owns `ModelResolver` and `LlmClientFactory`.
- `runtime::thread_session` freezes the resolved model for a running turn.
- `state::session::TurnContextItem` persists only `ModelRef`.

## Extension Points

- Add provider-specific adapters in separate files such as `anthropic.rs` or `gemini.rs`.
- Add provider protocol cases in `ProviderProtocol` and `DefaultLlmClientFactory`.
- Add model discovery or richer capability resolution behind `ModelResolver`.
- Keep credentials out of protocol, rollout JSONL, SQLite, and debug output.

## Model Selection

`AgentConfig.model` is the runtime source of truth for the resolved model used by
an `Agent`. The durable identity is still `ModelRef`; `ResolvedModelConfig`
includes endpoint, protocol, credential, and capabilities only for runtime use.

Provider/model switching is between user turns only. A running turn freezes its
resolved model for the whole tool loop; a later turn can select a different
`ModelRef`, which is resolved before the runtime actor receives the turn.
