# ADR-0004: Use Runtime Thinking Mode Config

## Status

Accepted

## Context

Some model families require an explicit thinking or reasoning setting before they can be used reliably in tests and runtime flows. ExAgent already treats `AgentConfig.model` as the source of truth and passes it into the OpenAI-compatible adapter instead of letting the adapter reread model configuration from the environment.

Thinking mode has the same source-of-truth risk. If the runtime and the model adapter both read independent environment variables, the thread can record one setting while the provider request uses another.

## Decision

Represent thinking mode as provider-neutral runtime intent on `AgentConfig` and turn-scoped request options. The app-server protocol can override it per turn through `TurnContextOverrides`. Runtime code passes the selected value into `LlmRequestOptions`; provider adapters translate that generic value into provider-specific JSON.

For the current OpenAI-compatible chat-completions adapter, `ThinkingMode::Low | Medium | High` serializes as top-level `reasoning_effort`. `None` and `Auto` omit the provider field and leave provider defaults in place.

## Consequences

- Runtime state and provider requests use the same thinking selection.
- Provider-specific request keys stay inside the LLM adapter.
- Future adapters can map the same `ThinkingMode` to their own request shape without changing app-server or runtime code.
- Per-turn overrides can be added without mutating thread-global config for later turns.

## Affected Modules

- `src/config.rs`
- `src/model/llm.rs`
- `src/app_server/protocol.rs`
- `src/app_server/thread_manager.rs`
- `src/runtime/thread_runtime.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/context.rs`
- `src/state/session.rs`

## Related Docs

- `docs/superpowers/plans/2026-05-29-thinking-mode.md`
