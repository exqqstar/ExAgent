# File Map

This page is for locating code. It intentionally does not explain every implementation detail.

## Top Level

- `src/main.rs`: binary startup and CLI/API branching.
- `src/lib.rs`: crate exports and default tool registry.
- `src/config.rs`: runtime configuration and environment defaults.
- `src/workspace.rs`: workspace path canonicalization and boundary checks.

## Entrypoints

- `src/entrypoints/cli.rs`: CLI argument parsing.
- `src/entrypoints/cli_adapter.rs`: CLI command to app-server boundary calls.
- `src/entrypoints/api.rs`: Axum routes, JSON responses, and SSE event streaming.

## App Server

- `src/app_server/protocol.rs`: request/response DTOs and protocol enums.
- `src/app_server/service.rs`: `AppServerBoundary` trait and service facade.
- `src/app_server/thread_manager.rs`: thread lifecycle, turn lifecycle, runtime loading, event replay, view building.
- `src/app_server/override_policy.rs`: workspace/cwd override validation.
- `src/app_server/error.rs`: boundary-level error types.

## Runtime

- `src/runtime/agent.rs`: agent wrapper around config, LLM, tools, exec sessions, and policy.
- `src/runtime/thread_runtime.rs`: per-thread actor facade and operation queue.
- `src/runtime/thread_session/mod.rs`: thread session construction, rollout restore, live view, interrupt handling.
- `src/runtime/thread_session/turn.rs`: user-input turn execution and LLM/tool loop.
- `src/runtime/thread_session/events.rs`: event id allocation, rollout append, live state update, broadcast.
- `src/runtime/thread_session/overlay.rs`: live-only exec session and approval overlay.
- `src/runtime/context.rs`: model-visible conversation history, context injection, token tracking.
- `src/runtime/tool_call_runtime.rs`: tool execution wrapper and tool effect extraction.
- `src/runtime/exec_session.rs`: persistent subprocess sessions.
- `src/runtime/policy.rs`: command policy and pending approval registry.
- `src/runtime/compaction.rs`: conversation summarization and replacement history.

## State

- `src/state/session.rs`: `SessionSnapshot` and session-adjacent state types.
- `src/state/events.rs`: `RuntimeEvent` and event variants.
- `src/state/rollout.rs`: rollout item format, read/write, snapshot/event reconstruction.
- `src/state/transcript.rs`: JSON helpers, session id generation, legacy path helpers.

## Tools

- `src/tools/mod.rs`: `Tool` trait.
- `src/tools/registry.rs`: tool registry and `ToolContext`.
- `src/tools/read_file.rs`: workspace-bounded file reads.
- `src/tools/write_file.rs`: workspace-bounded file writes.
- `src/tools/run_command.rs`: shell command execution, persistent commands, approval decisions.

## Model

- `src/model/types.rs`: IDs, messages, tool calls, completions, token usage.
- `src/model/llm.rs`: `LlmClient`, mock LLM, OpenAI-compatible adapter.
