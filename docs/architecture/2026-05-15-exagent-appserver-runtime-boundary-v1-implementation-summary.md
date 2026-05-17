# ExAgent AppServer Runtime Boundary V1 Implementation Summary

**Date:** 2026-05-15
**Status:** Implemented and verified
**Update 2026-05-17:** The legacy child orchestration surface
`fork`/`inspect`/`collect`/`thread_spawn_child` has been removed. Child threads
will be reintroduced as runtime-native operations in a later design.
**Related:**
- `docs/architecture/2026-05-15-exagent-appserver-runtime-boundary-v1-design.md`
- `docs/plans/2026-05-15-exagent-appserver-runtime-boundary-v1.md`
- `codex-app-server-reference-pack/`

## What Changed

ExAgent now has a shared app-server runtime boundary instead of letting each
entrypoint assemble runtime state on its own.

The new shape is:

```text
CLI / HTTP / future GUI / tests
  -> AppServerBoundary
  -> AppServerService
  -> ThreadManager
  -> Agent core
  -> snapshot + events.jsonl
```

The important design move is not the HTTP server itself. The important move is
that thread lifecycle, turn lifecycle, config override policy, replay, and
interrupt behavior now live behind one protocol-shaped boundary.

## Main Protocol Surface

The primary protocol is `BoundaryOp` in `src/app_server/protocol.rs`.

V1 operations:

- `initialize`
- `thread_start`
- `thread_read`
- `thread_resume`
- `turn_start`
- `turn_interrupt`
- `events_replay`

`BoundaryCapability` is kept aligned with `BoundaryOp`. The test
`boundary_capabilities_match_boundary_op_type_names` guards that invariant.

## Thread And Turn Model

`thread_start` creates durable state. It writes the initial `SessionSnapshot`
and returns paths for the snapshot and event log.

`turn_start` advances one unit of work inside an existing thread. It emits
`TurnStarted`, calls the `Agent` runtime, and then emits either `TurnCompleted`,
`TurnInterrupted`, or `RuntimeError`.

Only one active turn is allowed per thread in one app-server process. A second
`turn_start` while a turn is active returns `ThreadBusy`.

V1 active-turn coordination is in-process only. Two separate app-server
processes pointing at the same workspace do not share the active-turn registry.

## Override Policy

Override rules are entrypoint-specific and live in
`src/app_server/override_policy.rs`.

The implemented rules are:

- `thread_start`: workspace and cwd request overrides can create durable thread
  context.
- `turn_start`: `turn_context.cwd` is validated and applied only to that turn.
  It does not rewrite the thread snapshot.
- `thread_resume`: persisted thread context wins. Unsupported request
  overrides are reported with `IgnoredOverrideField`.
- `events_replay`: read-only; no config mutation.

The key concurrency fix is that `turn_context` is validated after active-turn
reservation and is passed to `Agent::resume_with_turn_cwd` as per-turn runtime
context. Rejected concurrent turns cannot mutate the snapshot.

## Interrupt Semantics

`turn_interrupt` is part of `AppServerBoundary`.

For a running turn:

```text
turn_interrupt
  -> sends interrupt to the registered active turn
  -> waits until TurnInterrupted is written
  -> returns TurnInterruptResponse
```

For a waiting-approval thread:

```text
turn_interrupt
  -> clears pending approvals from the snapshot
  -> cancels pending policy approvals
  -> writes TurnInterrupted
  -> returns TurnInterruptResponse
```

This means clients can trust that a returned interrupt response has already
been reflected in replayable events.

## Replay Surface

`events_replay` is a first-class boundary operation. It is not a queued thread
operation and does not mutate state.

Supported request fields:

- `after_event_id`
- `limit`
- `include_snapshot`
- `event_kinds`

When `include_snapshot` is true, the response returns `ReplaySnapshotView`, not
the internal `SessionSnapshot`. The view exposes stable protocol fields:

- `thread_id`
- `cwd`
- `latest_compaction`
- `open_exec_session_count`
- `conversation_message_count`
- `pending_approval_count`

## Adapter Mapping

CLI code uses `src/cli_adapter.rs`. It maps user commands into boundary calls:

```text
run     -> thread_start -> turn_start
resume  -> thread_resume -> turn_start
```

HTTP code in `src/api.rs` is a transport adapter. The current app-server boundary V1 routes are:

- `POST /initialize`
- `POST /thread/start`
- `POST /thread/read`
- `POST /thread/resume`
- `POST /turn/start`
- `POST /turn/interrupt`
- `POST /thread/op`
- `POST /events/replay`

`POST /initialize` maps to `BoundaryOp::Initialize` and returns the tagged
boundary response shape. `POST /thread/op` is still available as the generic
protocol dispatch route for clients that submit tagged `BoundaryOp` payloads.

The older runtime-control routes `POST /threads` and
`POST /threads/{session_id}/turns` are intentionally not exposed as HTTP
routes. The legacy runtime-control prototype has been removed, so thread and
turn lifecycle enters through the app-server boundary.

The earlier orchestration routes `POST /fork`, `POST /inspect`,
`POST /collect`, and `POST /thread/spawn_child` have been removed. The earlier
underscore aliases `POST /thread_spawn_child` and `POST /events_replay` have
also been removed.

## Code Map

Core files:

- `src/app_server/protocol.rs`: protocol DTOs, ops, responses, status enums.
- `src/app_server/override_policy.rs`: named config merge rules.
- `src/app_server/thread_manager.rs`: thread lifecycle, turn lifecycle,
  active-turn registry, replay, interrupt handling.
- `src/app_server/service.rs`: public boundary facade and trait.
- `src/app_server/error.rs`: typed app-server errors.
- `src/cli_adapter.rs`: CLI-to-boundary adapter.
- `src/api.rs`: HTTP-to-boundary adapter.
- `src/agent.rs`: core model/tool loop with per-turn cwd support.
- `src/policy.rs`: pending approval management, including cancellation by
  session.

Important tests:

- `tests/app_server_boundary.rs`: protocol lifecycle, concurrency, replay,
  interrupt, override policy integration.
- `tests/api_server.rs`: HTTP route mapping and JSON shapes.
- `tests/cli_adapter.rs`: CLI uses boundary calls instead of legacy direct
  runtime assembly.
- `tests/override_policy.rs`: focused override merge rules.

## Review Items Closed

The implementation closes the review findings in these ways:

- `ThreadStart` is a dispatchable `BoundaryOp`.
- `turn_interrupt` is in the boundary trait and HTTP adapter.
- `turn_context` no longer writes snapshot state before active-turn
  reservation.
- active-turn interrupt registration has no post-spawn handle window.
- `submit_boundary_op` is the canonical protocol dispatch path.
- slash HTTP routes exist for child spawn and replay.
- legacy DTOs are marked as compatibility surface.
- `ignored_overrides` uses `IgnoredOverrideField`.
- replay exposes `ReplaySnapshotView`.
- interrupt response waits until the interrupt event is persisted.
- waiting approval state is readable and interruptible.
- interrupted event logs remain valid JSONL.

## Verification

Fresh verification used:

```bash
cargo fmt -- --check
git diff --check
cargo test
```

## Remaining V2 Work

Useful follow-ups that are intentionally outside V1:

- client capability negotiation in `initialize`, if GUI or streaming clients
  need to advertise supported features.
- richer tracing spans for `thread_start`, `turn_start`, `turn_interrupt`, and
  replay/subscription paths.
- explicit SDK/client documentation for the `events_subscribe` SSE envelope.
- cross-process active-turn ownership.
- runtime-native child thread/fork modes.
- replacing the in-process active-turn `Mutex<HashMap<...>>` if concurrency
  pressure requires it.
