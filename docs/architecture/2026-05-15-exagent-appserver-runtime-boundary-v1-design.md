# ExAgent AppServer Runtime Boundary V1 Design

**Date:** 2026-05-15
**Status:** Architecture design
**Audience:** ExAgent runtime design, future CLI/API/GUI adapters, interview explanation
**Related:** `codex-app-server-reference-pack/`, `docs/plans/2026-05-15-exagent-appserver-runtime-boundary-v1.md`

## Purpose

This document defines the target architecture for ExAgent's first app-server runtime boundary.

The goal is not to make the CLI prettier. The goal is to stop every entrypoint from assembling the runtime in its own way. CLI, HTTP API, GUI, and test harnesses should all speak to one boundary that understands thread lifecycle, turn lifecycle, override policy, durable events, and replay.

The core design lesson from Codex is that the durable boundary is not "there is a server." The durable boundary is:

```text
Thread = state container
Turn   = one unit of progress inside a thread
Op     = typed command submitted to a thread
Event  = replayable runtime fact emitted by execution
```

The stable API should be drawn around `Op + Event`, because that pair does not depend on whether the caller is a CLI, HTTP client, GUI, IDE, or test harness.

## Design Position

The old mental model is:

```text
CLI/API request
  -> build Agent directly
  -> run model/tool loop
  -> return final answer
```

That model works for a demo, but it makes long-term growth expensive. Runtime construction spreads across entrypoints. Configuration rules drift. A future GUI or replay tool has to rediscover internal details.

The target model is:

```text
External adapter
  -> protocol request
  -> AppServerService
  -> ThreadManager
  -> Thread / Turn / Op
  -> Agent core
  -> RuntimeEvent log
  -> adapter-specific presentation
```

The CLI is one adapter. The HTTP API is one adapter. A future desktop app is one adapter. None of them should own runtime assembly.

## Core Objects

### Thread

A thread is the durable state container. In the current ExAgent codebase, it maps most closely to `SessionSnapshot`, but the app-server boundary should use the thread name deliberately.

A thread owns:

- `thread_id`
- `workspace_root`
- `cwd`
- conversation state
- open exec session references
- pending approvals
- lineage metadata
- event log path
- snapshot path

Thread identity is stable across turns. A thread can be resumed, read, interrupted, and replayed.

### Turn

A turn is one unit of progress inside a thread. It starts from user input plus explicit turn context overrides, then advances the model/tool loop until the runtime reaches a terminal condition for that turn.

The important distinction is:

```text
thread/start creates a state container
turn/start pushes work into that container
```

This keeps context and execution separate. A GUI can create a thread first, show its configuration, then start a turn later. A CLI can hide that split and perform both operations as one command.

### Op

An Op is the typed internal command submitted to a live thread.

V1 should distinguish boundary operations from queued thread operations:

```text
Boundary operation:
  initialize
  thread/start
  thread/resume
  thread/read
  turn/start
  turn/interrupt
  events/replay

Queued thread op:
  user_input
  user_input_with_turn_context
  interrupt
  shutdown
```

Not every boundary operation becomes a queued thread op. `events/replay` is a read-side operation and should go directly to the store/event log. It must not be blocked by a live turn queue, and it must not mutate thread state.

### Event

Events are replayable runtime facts. They are not just logs.

V1 events include:

- `turn_started`
- `turn_completed`
- `turn_interrupted`
- `assistant_turn`
- `tool_result`
- `exec_output`
- `approval_requested`
- `approval_decision`
- `compaction_written`
- `runtime_error`

The event log is the observability and replay plane. The snapshot is the fast-resume plane. Both are needed.

## Lifecycle State Model

The boundary should expose lifecycle state explicitly. Callers should not infer state from missing files, open subprocess handles, or ad hoc text output.

V1 thread states:

```text
idle
  Thread exists and has no active turn.

running
  A turn is actively advancing the model/tool loop.

waiting_approval
  A turn is blocked on a pending approval.

failed
  The latest active turn ended with a runtime error.

archived
  Thread is retained for replay/read access but no longer accepts new turns.
```

V1 turn states:

```text
queued
  Accepted by the boundary but not yet active.

running
  The runtime is processing model/tool work.

waiting_approval
  The turn cannot continue until approval is resolved.

completed
  The turn reached a terminal assistant response.

failed
  The turn ended with an error.

interrupted
  The caller interrupted the active turn.
```

V1 can start without a sophisticated scheduler, but the status vocabulary should exist early. It gives GUI, HTTP clients, CLI JSON output, and replay tooling one shared language.

## Concurrency Rules

V1 should keep concurrency conservative.

The default rule is:

```text
one active turn per thread
```

If `turn/start` is called while a thread already has an active turn, V1 should either reject the request with a typed error or enqueue it explicitly. The choice must be visible in the protocol. Silent parallel turns in the same thread are not allowed.

Recommended V1 behavior:

```text
turn/start while idle
  -> accepted

turn/start while running
  -> rejected with thread_busy

turn/interrupt while running
  -> submits interrupt to active turn

turn/interrupt while waiting_approval
  -> clears pending approvals and records turn_interrupted

events/replay while running
  -> allowed, read-only
```

This keeps the first boundary deterministic. Later versions can add a real queue, but the V1 contract should not imply hidden scheduling.

## Protocol Surface

V1 should expose a compact protocol surface:

```text
initialize
thread/start
thread/resume
thread/read
turn/start
turn/interrupt
events/replay
```

`events/subscribe` is a natural future addition, but V1 may remain non-streaming if implementation risk needs to stay low. The design should still treat event streaming as the intended output plane, not as incidental tracing.

### CLI Mapping

CLI commands become protocol clients:

```text
exagent "prompt"
  -> thread/start
  -> turn/start
  -> print final assistant text

exagent resume <thread_id> "prompt"
  -> thread/resume
  -> turn/start
```

The CLI may combine multiple protocol calls for convenience, but it should not build `Agent` directly.

The earlier `fork`, `inspect`, and `collect` commands were removed from the
current boundary. Child orchestration will be reintroduced only after it can be
modeled as runtime-native thread operations instead of a separate legacy
execution path.

### HTTP/API Mapping

HTTP should be a transport adapter. The current V1 app-server boundary route names mirror the protocol names:

```text
POST /initialize
POST /thread/start
POST /thread/read
POST /thread/resume
POST /turn/start
POST /turn/interrupt
POST /thread/op
POST /events/replay
```

`POST /initialize` is the dedicated HTTP adapter for `BoundaryOp::Initialize`.
`POST /thread/op` remains the generic protocol dispatch route for clients that
want to submit tagged `BoundaryOp` payloads directly.

The older runtime-control routes `POST /threads` and
`POST /threads/{session_id}/turns` are not part of the public AppServer
Runtime Boundary V1 surface. The legacy runtime-control prototype has been
removed so thread and turn lifecycle cannot enter through a competing runtime
path.

The legacy `POST /fork`, `POST /inspect`, `POST /collect`,
`POST /thread/spawn_child`, `POST /thread_spawn_child`, and
`POST /events_replay` routes are not part of the boundary surface.

The route shape can evolve, but the rule is stable: HTTP handlers translate JSON into protocol requests and delegate to the app-server boundary.

## Override Policy

Override handling is a first-class design concern. It should not be hidden inside route handlers or message processor code.

Codex's useful lesson is that override rules are positional. There is no single global merge rule that works for every lifecycle operation.

ExAgent V1 should model override policy by entrypoint:

### `thread/start`

`thread/start` creates new durable state, so request overrides may participate in initial config resolution.

Inputs:

```text
default config
environment config
request workspace_root / cwd / model / policy fields
```

Output:

```text
resolved thread config
initial snapshot
thread started event or response
```

### `turn/start`

`turn/start` may carry explicit turn context overrides. These overrides must be validated for that turn, then applied to the model/tool loop for that turn only.

This avoids a race where context changes are applied separately from user input. V1 turn context is ephemeral: `turn_context.cwd` must not rewrite the durable thread snapshot.

Target internal shape:

```text
Op::UserInput
Op::UserInputWithTurnContext
```

If no overrides are provided, the turn uses the thread's current context.

### `thread/resume`

Resume should respect persisted thread metadata. Request overrides may be allowed, but they must be explicit and narrow.

Default rule:

```text
persisted thread context wins
unsupported request overrides are reported as ignored
```

In V1, `cwd` on `thread/resume` is ignored and reported as `IgnoredOverrideField::Cwd`. This keeps old sessions stable while still leaving room for deliberate model or policy changes when the protocol supports them.

### Running Thread Resume

If a thread is already running, resume should not silently mutate its active config.

The boundary should report ignored overrides or mismatch warnings rather than pretending the request was applied.

Target behavior:

```text
resume running thread
  -> attach/read current state
  -> report ignored override mismatches
  -> do not mutate active thread config
```

### Config Authority Matrix

The source of truth depends on the lifecycle operation:

```text
Operation              Authority
---------------------  --------------------------------------------------
thread/start           defaults + environment + explicit request overrides
turn/start             current thread context + explicit per-turn overrides; no snapshot mutation
thread/resume          persisted thread context; unsupported overrides reported as ignored
running thread/resume  active thread context; request overrides ignored with warnings
events/replay          persisted event log only; no config mutation
```

This matrix is the contract `OverridePolicy` should implement and tests should pin. It is intentionally entrypoint-specific.

## Child Thread Status

The earlier child-session surface was deliberately removed from the current
boundary. It created a second execution path around the live thread runtime and
depended on old `Agent::fork_session`, `inspect_children`, and `collect_session`
helpers.

The next child-thread design should be explicit about whether it is:

```text
snapshot fork       copy durable state at a point in time
prefix fork         copy transcript prefix then inject new input
lineage child       create a related thread without transcript inheritance
```

Until that design exists, the protocol surface intentionally contains no
`thread/spawn_child` operation.

Transcript-copying forks can be added later as separate operations:

```text
thread/fork_from_history
thread/fork_from_snapshot
```

Do not overload V1 child spawn with hidden transcript-copy behavior.

## Events And Replay

ExAgent's project identity depends on replayable event logs. Therefore replay must be visible at the boundary.

`events/replay` is a read-side boundary operation:

```text
events/replay(thread_id)
  -> read events.jsonl
  -> return ordered RuntimeEvent list
```

It is not a queued thread op. It does not start a turn. It does not require the thread to be loaded. It should work for completed, failed, or inactive threads as long as the persisted event log exists.

This makes future GUI replay panels, deterministic test harnesses, audit tooling, and debugging workflows possible without reaching into internal file paths.

### Replay Cursor

V1 can return the full ordered event list, but the protocol should reserve cursor semantics now.

Minimal V1 request:

```text
events/replay(thread_id)
```

Reserved future fields:

```text
after_event_id
limit
include_snapshot
event_kinds
```

Cursor semantics:

```text
without cursor
  -> return all events in persisted order

after_event_id
  -> return events strictly after that event

limit
  -> cap the number of returned events without changing order

include_snapshot
  -> include a stable ReplaySnapshotView next to events for fast UI reconstruction
```

V1 implements `after_event_id`, `limit`, `event_kinds`, and `include_snapshot`. `include_snapshot` must not expose the internal `SessionSnapshot` shape directly; it returns a protocol view with `thread_id`, `cwd`, `latest_compaction`, and count fields for conversation messages, open exec sessions, and pending approvals.

## Store Boundary

The store is the persistence boundary behind the app-server service.

It owns:

- snapshot reads and writes
- event append
- event replay
- session path resolution
- lineage event reads

The important rule:

```text
CLI/API/GUI adapters do not read session files directly.
```

Adapters ask the app-server boundary for thread state, collected child output, or replayed events. The boundary delegates to store helpers such as `transcript.rs`.

This preserves one persistence contract. It also prevents future clients from depending on incidental file layout.

## Component Boundaries

Target module responsibilities:

```text
src/app_server/protocol.rs
  Public DTOs and protocol request/response types.

src/app_server/override_policy.rs
  Named override merge rules by lifecycle entrypoint.

src/app_server/thread_manager.rs
  Owns thread lifecycle, runtime construction, shared exec/policy managers,
  and conversion from protocol operations to Agent/core calls.

src/app_server/service.rs
  Public service facade used by CLI/API/GUI/test harness adapters.

src/api.rs
  HTTP adapter only.

src/cli.rs
  CLI parser and command DTOs only.

src/main.rs
  Process startup and top-level dispatch only.

src/agent.rs
  Core model/tool loop. It should not know CLI, HTTP, or GUI details.

src/transcript.rs
  Snapshot/event persistence and replay helpers.
```

The design pressure should always push caller-specific logic outward and runtime state transitions inward.

## Error Model

The boundary should return typed runtime errors that adapters can present differently.

Examples:

- invalid request
- thread not found
- turn rejected
- cwd outside workspace
- override ignored
- config resolution failed
- runtime failed

HTTP can map these to status codes. CLI can render concise terminal messages. Tests can assert variants directly.

## Non-Goals For V1

V1 should not attempt to implement the full Codex app-server feature set.

Non-goals:

- multiple transports
- remote auth
- plugin marketplace
- full event subscription stream, unless needed immediately
- transcript-copy fork modes
- planner or task graph scheduler
- mailbox-based multi-agent coordination
- production sandbox isolation
- cross-process active-turn coordination

The goal is the smallest boundary that makes runtime construction centralized and protocol-shaped.

## V1 Runtime Scope

V1 active-turn coordination is in-process only. The implementation uses a single-process active-turn registry to enforce `thread_busy`; two app-server processes pointing at the same workspace do not share that registry. The durable snapshot and event log remain shared persistence, but active turn ownership is not distributed in V1.

## Design Invariants

These invariants should stay true after the refactor:

1. CLI and API do not construct `Agent` directly.
2. New thread creation and turn execution are separate concepts.
3. Turn context overrides are explicit and validated before accepting input.
4. Child spawn semantics are lineage-based in V1.
5. Replay is a first-class boundary operation.
6. Snapshot and event log remain separate persistence planes.
7. Core runtime does not know whether the caller was CLI, HTTP, GUI, or test harness.
8. `BoundaryCapability` and `BoundaryOp` stay aligned; every advertised capability has a dispatchable op.
9. `events/replay` returns protocol DTOs, not internal snapshot structs.
10. Active-turn exclusivity is guaranteed only inside one app-server process in V1.

## Summary

The architectural move is from entrypoint-owned runtime assembly to a shared app-server runtime boundary.

The boundary should make these ideas explicit:

```text
Thread lifecycle
Turn lifecycle
Override policy by entrypoint
Queued thread ops
Replayable runtime events
Adapter-specific presentation
```

Once this boundary exists, CLI v2 becomes a small client of the runtime instead of the runtime's owner. HTTP and GUI clients can reuse the same protocol. Tests can exercise the same surface users exercise. That is the long-term value.
