# ExAgent Thread Runtime Actor V2 Design

**Date:** 2026-05-16
**Status:** Implemented
**Related:**
- `docs/architecture/2026-05-15-exagent-appserver-runtime-boundary-v1-design.md`
- `docs/architecture/2026-05-15-exagent-appserver-runtime-boundary-v1-implementation-summary.md`
- `docs/plans/2026-05-16-exagent-thread-runtime-actor-v2.md`
- `external-references/Codex/codex-rs/core/src/thread_manager.rs`
- `external-references/Codex/codex-rs/core/src/codex_thread.rs`
- `external-references/Codex/codex-rs/core/src/session/mod.rs`
- `external-references/Codex/codex-rs/core/src/session/handlers.rs`

## Goal

ExAgent moved from request-scoped agent execution to a thread-scoped runtime
actor model.

The V1 boundary correctly separated thread lifecycle from turn lifecycle, but
`ThreadManager` still directly ran the agent for a turn. V2 makes
`ThreadManager` a registry/factory and moves turn execution into a long-lived
per-thread runtime loop.

Target shape:

```text
CLI / HTTP / future GUI
  -> typed app-server protocol
  -> AppServerService
  -> ThreadManager
       loaded_threads: HashMap<SessionId, Arc<ThreadRuntime>>
  -> ThreadRuntime
       submit_user_input(...)
       subscribe_events()
       live_view()
       status()
       shutdown()
  -> ThreadRuntimeLoop
       receives ThreadSubmission
       dispatches mailbox ops
  -> ThreadSession
       owns per-thread execution state
       holds live Agent + snapshot
       emits RuntimeEvent
       persists snapshot + events.jsonl
       exposes authoritative live view while loaded
```

## Codex Reference Model

Codex has a useful separation:

```text
app-server
  -> codex_core::ThreadManager
  -> CodexThread
  -> Codex
  -> Session
  -> submission_loop
  -> run_turn
```

The important mapping is not the exact type names. It is the responsibility
split:

```text
ThreadManager
  Process-level registry and factory for live threads.

CodexThread
  Live thread handle. It can submit ops, expose events/status, and shut down.

Codex
  Queue pair. It owns the submission sender and event receiver.

Session
  Per-thread runtime state.

submission_loop
  Mailbox loop. It receives Op values and serializes runtime mutations.
```

ExAgent V2 should copy this shape in smaller form:

```text
ThreadManager        ~= Codex core ThreadManager
ThreadRuntime        ~= CodexThread + Codex handle
ThreadRuntimeLoop    ~= submission_loop
ThreadSession        ~= Session
ThreadOp             ~= Codex Op
RuntimeEvent         ~= Codex EventMsg-derived output plane
```

## Current V1 Problem

Today, `src/app_server/thread_manager.rs` does too many jobs:

```text
thread registry-ish behavior
override policy application
snapshot read/write
event append
active turn reservation
agent construction
agent execution
interrupt handling
child spawn
replay
```

The highest-risk part is `turn_start_direct`:

```text
turn_start_direct
  -> reserve_active_turn
  -> append TurnStarted
  -> Agent::resume_with_turn_cwd(...)
  -> append TurnCompleted / RuntimeError / TurnInterrupted
  -> return final output
```

That means `turn/start` is still a request-scoped execution call. It is not a
submission into a long-lived thread runtime. This makes future GUI behavior
awkward because the final answer is coupled to the request response instead of
the event stream.

## V2 Responsibility Split

### `ThreadManager`

`ThreadManager` becomes the process-level registry and factory.

Responsibilities:

- Hold shared runtime dependencies:
  - base `AgentConfig`
  - LLM factory
  - tool registry factory
  - `ExecSessionManager`
  - `PolicyManager`
- Hold loaded thread handles:

```rust
loaded_threads: Arc<Mutex<HashMap<String, Arc<ThreadRuntime>>>>
```

- Create new runtimes through `start_thread_with_options`.
- Resume existing runtimes from snapshot/event storage.
- Look up loaded runtimes by thread id.
- Submit ops to a loaded runtime.
- Read stored thread state for `thread/read`.
- Replay persisted events for `events/replay`.
- Shut down or unload runtimes.

Non-responsibilities:

- It should not call `Agent::resume*` directly.
- It should not own per-turn cancellation mechanics directly.
- It should not stream individual runtime events itself.

Initial shape:

```rust
pub struct StartThreadOptions {
    pub config: AgentConfig,
    pub initial_history: InitialHistory,
}

pub enum InitialHistory {
    New,
    Resume { thread_id: SessionId },
}

pub struct NewThread {
    pub thread_id: SessionId,
    pub runtime: Arc<ThreadRuntime>,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}
```

The first version stays smaller than Codex. Shared managers such as
`AuthManager`, `ModelsManager`, `SkillsManager`, `PluginsManager`,
`McpManager`, and `EnvironmentManager` should not be added until ExAgent has
real features that need them.

Fork/spawn remains on the compatibility path for now. It still records lineage
events, but it does not create child runtimes in V2.

### `ThreadRuntime`

`ThreadRuntime` is the live thread handle. It is the thing stored in
`ThreadManager.loaded_threads`.

Responsibilities:

- Submit user-input turns through guarded methods.
- Expose live event subscription.
- Expose current status.
- Own the active-turn interrupt handle.
- Expose an authoritative live view for `thread/read` while loaded.
- Provide thread metadata such as snapshot/event paths.
- Shut down the runtime loop.

Non-responsibilities:

- It should not expose raw `ThreadOp::UserInput` submission. User input must go
  through active-turn reservation so concurrent turn rejection and interrupt
  state stay coherent.

Target shape:

```rust
pub struct ThreadRuntime {
    thread_id: SessionId,
    op_tx: mpsc::Sender<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status: watch::Receiver<ThreadRuntimeStatus>,
    active_turn: Arc<Mutex<Option<ActiveRuntimeTurnRecord>>>,
    snapshot_path: PathBuf,
    events_path: PathBuf,
}

pub struct ThreadSubmission {
    pub submission_id: String,
    pub op: ThreadOp,
    pub completion_tx: Option<oneshot::Sender<ThreadOpResult>>,
}
```

`ThreadRuntime` should not run the agent itself. It is a handle to a background
loop, like Codex's `CodexThread`.

### `ThreadRuntimeLoop`

`ThreadRuntimeLoop` serializes mailbox submissions for one thread. It should
stay thin: receive a `ThreadSubmission`, dispatch the internal `ThreadOp`, and
complete the optional response channel.

Responsibilities:

- Receive `ThreadSubmission` values from `op_rx`.
- Dispatch `ThreadOp` to `ThreadSession`.
- Preserve submission completion semantics.
- Stop the loop on shutdown.

Non-responsibilities:

- It should not call `Agent` directly.
- It should not append or replay runtime events directly.
- It should not own long-lived session state beyond the mailbox receiver.

Target op shape:

```rust
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
}

pub enum ThreadOp {
    UserInput {
        turn_id: TurnId,
        prompt: String,
        turn_context: Option<ThreadTurnContext>,
    },
    Interrupt {
        turn_id: Option<TurnId>,
    },
    Shutdown,
}
```

The loop makes the agent input clean:

```rust
pub struct AgentTurnInput {
    pub thread_id: SessionId,
    pub turn_id: TurnId,
    pub prompt: String,
    pub turn_cwd: Option<PathBuf>,
}
```

No CLI command, HTTP request, or `BoundaryOp` should reach `Agent`.

### `ThreadSession`

`ThreadSession` is the per-thread state and handler object held by
`ThreadRuntimeLoop`.

Responsibilities:

- Hold runtime execution inputs:
  - thread id
  - `AgentConfig`
  - live `Agent`
  - in-memory `SessionSnapshot`
  - transcript/event paths
  - next event cursor
  - live event broadcaster
  - status watch sender
- Handle processed internal ops such as `ThreadOp::UserInput`.
- Update runtime status around a turn.
- Advance turns against the live in-memory snapshot.
- Append lifecycle, assistant, and tool events.
- Assign runtime event ids.
- Broadcast live runtime events.
- Checkpoint `snapshot.json`.
- Keep an in-memory event buffer for loaded-thread reads.
- Report shutdown and interrupt results back through `ThreadOpResult`.

`ThreadSession` loads the durable snapshot when the runtime is spawned and
keeps that snapshot in memory while the thread is live. Disk remains the
recovery and replay surface; normal turn execution should not reconstruct the
agent state from disk for every user input. While a runtime is loaded,
`thread/read` should prefer `ThreadRuntime::live_view()` over direct disk reads.
The live event list exposed by that view is a bounded recent window; clients
that need the complete timeline must use `events/replay`, which reads the full
persisted `events.jsonl`.

### Live Runtime Ownership Matrix

The live turn path intentionally uses a half-delta contract. `Agent` is allowed
to mutate the in-memory `SessionSnapshot` fields that are part of executing a
turn, but `ThreadSession` remains the owner of persistence, event ids,
broadcast, and client-visible state publication.

| Component | Owns | Must Not Own |
| --- | --- | --- |
| `ThreadManager` | Runtime registry, start/resume/factory policy, app-server override resolution. | Agent execution, event streaming, active turn cancellation internals. |
| `ThreadRuntime` | Mailbox handle, guarded user-turn submission, active-turn interrupt handle, live subscriptions. | `SessionSnapshot` mutation, event id assignment, event append. |
| `ThreadRuntimeLoop` | Serial mailbox dispatch from `ThreadOp` to `ThreadSession`. | Agent execution details, durable transcript writes, protocol DTOs. |
| `ThreadSession` | Live snapshot state, event id assignment, `events.jsonl` append, snapshot checkpoint, event broadcast, live read view, status updates. | CLI/HTTP protocol parsing, request-scoped config resolution. |
| `Agent` | Turn execution and in-place mutation of `snapshot.conversation`, `snapshot.open_exec_sessions`, and `snapshot.pending_approvals`. | `RuntimeEvent` ids, event persistence, snapshot file writes, live broadcast, thread status. |

This is the ExAgent equivalent of Codex's `Session::send_event` split:
execution code may produce state changes, but the session object owns recording
and delivery.

The first implementation is intentionally small:

```text
runtime/thread_session/
  mod.rs     # ThreadSession state and op-level helpers
  turn.rs    # user turn lifecycle
  events.rs  # append and broadcast helpers
```

It should not depend on app-server protocol DTOs, `BoundaryOp`, HTTP, or CLI
types.

## Protocol Semantics

Because there are no external users yet, V2 should change the public semantics
now instead of preserving request-scoped execution.

Target behavior:

```text
thread/start
  -> creates or loads ThreadRuntime
  -> returns Thread view with status

turn/start
  -> submits ThreadOp::UserInput
  -> returns Turn view with status: in_progress
  -> final result arrives via events

turn/interrupt
  -> submits ThreadOp::Interrupt
  -> returns accepted/interrupted status

events/subscribe
  -> SSE stream of RuntimeEvent values for a thread
  -> replays after_event_id first when provided, then switches to live broadcast

events/replay
  -> persisted event replay from events.jsonl
```

`initialize` separates request/response operations from streaming surfaces:
`supported_ops` corresponds to `BoundaryOp` variants, while
`supported_streams` advertises stream-only capabilities such as
`events_subscribe`.

`turn/start` should no longer return final assistant output. That output belongs
to the event plane.

The CLI remains synchronous from the user's perspective by acting as a client:

```text
exagent "prompt"
  -> thread/start
  -> events/subscribe
  -> turn/start
  -> wait until turn_completed
  -> print final assistant text
```

## Event Plane

V2 should treat events as the primary output plane.

Every runtime event follows two paths:

```text
ThreadSession emits event
  -> append to events.jsonl
  -> broadcast to live subscribers
```

Tools do not own live runtime event delivery. When a tool needs policy approval,
it returns approval metadata; the live Agent path records
`ApprovalRequested`/`ApprovalDecision` through `ThreadSession` so event ids,
persistence, live broadcast, and `thread_read` publication stay on one path.

Each persisted event must carry a stable event id or cursor. Live delivery is
allowed to be best-effort, but replay must be able to recover from a known
cursor so GUI and CLI clients can reconnect without depending on in-memory
broadcast history.

V2 does not compare cursor strings lexicographically. `events/replay` finds the
exact `after_event_id` in `events.jsonl` and returns events after that file
position. `events/subscribe` uses the same replay-first rule before switching to
the live broadcast channel. Future pagination code should keep that positional
semantics or move `EventId` to a typed numeric value.

Initial live event set:

```text
turn_started
assistant_turn
tool_result
exec_output
approval_requested
approval_decision
runtime_error
turn_completed
turn_interrupted
```

Child session lineage events are intentionally not part of the V2 runtime actor
contract. Existing fork/spawn behavior can remain on the compatibility path until
a later orchestration design defines how parent and child timelines should be
routed through live runtimes.

All events produced during one `ThreadOp::UserInput` are normalized to the
outer runtime turn id. This keeps assistant/tool/approval events grouped under
one user turn even when the agent internally performs multiple model/tool
iterations.

The current LLM layer is not streaming token deltas, so V2 should not fake
delta events. Later, when the LLM client supports streaming, add:

```text
assistant_delta
reasoning_delta
tool_started
tool_completed
```

## Thread / Turn / Item View

GUI clients should not reconstruct everything from raw snapshot paths.

V2 should introduce a renderable view:

```rust
pub struct ThreadView {
    pub id: SessionId,
    pub status: ThreadStatus,
    pub active_turn: Option<TurnView>,
    pub turns: Vec<TurnView>,
    pub snapshot_path: PathBuf,
    pub events_path: PathBuf,
}

pub struct TurnView {
    pub id: TurnId,
    pub status: TurnStatus,
    pub items: Vec<ThreadItem>,
}

pub enum ThreadItem {
    UserMessage { text: String },
    AssistantMessage { text: Option<String> },
    ToolResult { name: String },
    ExecOutput { text: String },
    ApprovalRequested { tool_name: String, reason: String },
    ApprovalDecision { status: String, note: Option<String> },
    RuntimeError { message: String },
    StructuredResult { kind: String },
    CompactionWritten,
}
```

The first version is intentionally event-derived and slightly lossy. It builds
turns from `TurnStarted` / `TurnCompleted` / `TurnInterrupted` lifecycle
events, appends assistant/tool/output/approval/error items, and exposes the
active turn from the live `ThreadRuntime` when a turn is still running.

V2 uses `TurnStatus::InProgress` for accepted public turns. `Running` can remain
an internal runtime status, but the app-server protocol aligns with Codex's
client-facing turn vocabulary.

## What Not To Build Yet

Do not copy Codex's full app-server surface in this phase.

Out of scope:

- JSON-RPC connection lifecycle.
- TypeScript/Python SDK generation.
- Plugin marketplace APIs.
- Skills/MCP manager integration.
- Realtime audio.
- Remote control.
- Thread archive/list/search.
- Multi-runtime child orchestration.
- Multi-client approval routing.
- Token delta streaming.

The design should still leave room for those systems by keeping runtime input
as `ThreadOp` and output as `RuntimeEvent`.

## Migration Strategy

This is a deliberate breaking change.

Recommended order:

```text
1. Add ThreadRuntime types and tests.
2. Move turn execution from ThreadManager into ThreadRuntimeLoop.
3. Split per-thread state and handlers into ThreadSession.
4. Change ThreadManager into registry/factory.
5. Change turn/start protocol to return in_progress instead of final output.
6. Add events/subscribe.
7. Change CLI to wait on events.
8. Add Thread/Turn/Item views.
9. Update docs and remove V1 compatibility assumptions.
```

The result should be closer to Codex's stable architecture:

```text
ThreadManager
  -> ThreadRuntime handle
  -> runtime mailbox
  -> ThreadSession
  -> live snapshot + clean Agent turn input
  -> event stream
```
