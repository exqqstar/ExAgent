# Thinking Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provider-neutral thinking mode selection so runtime config and model requests agree on thinking depth.

**Architecture:** Store generic `ThinkingMode` on `AgentConfig` and per-turn `ThreadTurnContext`. Thread execution passes it into `LlmRequestOptions`; the OpenAI-compatible adapter translates low/medium/high into provider request JSON. `Auto` and unset values omit provider-specific fields.

**Tech Stack:** Rust, serde, async_trait, Axum DTOs, rollout `TurnContextItem`, OpenAI-compatible `/chat/completions` adapter.

---

## File Structure

- Modify `src/config.rs`: define `ThinkingMode`, parse `EXAGENT_THINKING_MODE`, and store it on `AgentConfig`.
- Modify `src/model/llm.rs`: add `LlmRequestOptions`, extend `LlmClient::complete`, and serialize provider-specific thinking fields.
- Modify `src/app_server/protocol.rs`: accept optional `thinking_mode` in `RunParams` and `TurnContextOverrides`.
- Modify `src/app_server/thread_manager.rs`: keep thread-level thinking mode on `AgentConfig` and map request-scoped overrides into runtime turn context.
- Modify `src/runtime/thread_runtime.rs`: carry optional turn-level thinking mode in `ThreadTurnContext`.
- Modify `src/runtime/thread_session/turn.rs`: pass selected thinking mode to assistant sampling.
- Modify `src/runtime/context.rs` and `src/state/session.rs`: persist selected thinking mode in `TurnContextItem` for replay/debugging without using it as a prompt hack.
- Test `tests/api_server.rs`: request DTOs deserialize `thinking_mode`.
- Test `tests/app_server_boundary.rs`: turn override reaches persisted turn context and does not mutate later turns.
- Test `src/model/llm.rs`: adapter serializes/omits `reasoning_effort`.
- Test `src/config.rs`: env parsing accepts known modes and ignores invalid values.

## Tasks

### Task 1: Config And Protocol

- [x] Add failing tests for known and invalid thinking mode values.
- [x] Add failing API test for `turn_context.thinking_mode`.
- [x] Implement `ThinkingMode`, env parsing, and protocol fields.
- [x] Run `cargo test config::tests::thinking_mode_values_accept_known_modes` and `cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt`.

### Task 2: LLM Request Options

- [x] Add failing unit tests for OpenAI-compatible request serialization.
- [x] Add `LlmRequestOptions` and extend `LlmClient::complete`.
- [x] Map `ThinkingMode::Low | Medium | High` to `reasoning_effort`; omit `None` and `Auto`.
- [x] Update all mock LLM implementations to accept options.
- [x] Run `cargo test model::llm::tests::chat_completion_request_serializes_reasoning_effort_when_thinking_mode_is_set`.

### Task 3: Runtime Propagation

- [x] Add failing runtime/boundary test proving per-turn thinking mode reaches `TurnContextItem`.
- [x] Add `thinking_mode` to `ThreadTurnContext`.
- [x] Apply turn override only to the current turn's LLM request options.
- [x] Persist selected mode on `TurnContextItem`.
- [x] Run `cargo test --test app_server_boundary turn_context_thinking_mode_reaches_llm_without_mutating_later_turns`.

### Task 4: Verification

- [x] Run `cargo fmt --check`.
- [x] Run `git diff --check`.
- [x] Run `cargo test`.

## Notes

- Do not make the adapter reread thinking mode from provider-specific env vars.
- Do not inject thinking instructions into prompt text.
- Keep provider-specific keys out of `AgentConfig`; `AgentConfig` stores intent, adapters translate intent.
