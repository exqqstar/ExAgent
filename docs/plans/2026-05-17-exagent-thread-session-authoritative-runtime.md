# ExAgent Thread Session Authoritative Runtime Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make a loaded ExAgent thread behave like a live runtime actor: `ThreadSession` is the authoritative state and event owner while loaded, disk is replay/recovery storage, and `Agent` is reduced to turn execution.

**Architecture:** Keep the existing `ThreadManager -> ThreadRuntime -> ThreadRuntimeLoop -> ThreadSession -> Agent` shape, but move persistence, event assignment, broadcast, and live read ownership into `ThreadSession`. `ThreadRuntime` remains the handle/mailbox and `ThreadRuntimeLoop` remains the serializer; neither should own conversation state.

**Tech Stack:** Rust, Tokio `mpsc`/`broadcast`/`oneshot`/`watch`, serde JSON snapshots, JSONL event logs, existing app-server boundary DTOs.

**Implementation Status:** P0-P2 are implemented in the current working tree. `ThreadRuntime::live_view()` now serves loaded-thread reads, live user-input submission goes through guarded methods, and the live turn path returns event deltas for `ThreadSession` to persist and broadcast.

## Why This Plan Exists

The current runtime split is moving in the right direction, but the live-session contract is not fully true yet.

The intended contract is:

```text
loaded thread
  -> live ThreadSession is authoritative
  -> thread_read sees live state
  -> live event stream and persisted replay share one event source
  -> disk is recovery/replay/checkpoint storage
```

The current code only partially implements that:

```text
ThreadRuntime::spawn
  -> creates ThreadSession once
  -> ThreadRuntimeLoop owns it
  -> turn_start submits ThreadOp::UserInput
  -> ThreadSession calls legacy Agent live-turn runner(&mut snapshot, ...)
```

That gives us a long-lived session object, but the responsibilities are still split incorrectly. `ThreadSession` owns `SessionSnapshot`, yet `Agent` still writes the snapshot and appends assistant/tool events. `ThreadSession` writes lifecycle events and broadcasts. `thread_read` still reads snapshot/events from disk, even when a runtime is loaded.

This creates three problems:

1. Live state is not actually the reader source of truth.
2. Event persistence and broadcast have multiple writers.
3. `Agent` is too heavy and owns session behavior that should belong to `ThreadSession`.

## P0-P2 Scope

This plan treats P0-P2 as an implementation sequence, not separate product phases.

P0 is correctness. A loaded runtime must be the source of truth for thread reads, and raw user-input submission must not bypass active-turn reservation. Without this, GUI/app-server clients can observe stale state, interrupts can miss a running turn, and concurrent turn rejection can become inconsistent.

P1 is ownership cleanup. `ThreadSession` must own live event persistence, event ID assignment, broadcasting, and snapshot checkpoints. `Agent` should execute a turn and return deltas; it should not be the hidden writer for live session state. Without this, success and error paths use different event sources and future refactors can silently break replay or streaming.

P2 is architecture alignment. The docs and code should describe the same boundary: `ThreadManager` manages loaded runtimes, `ThreadRuntime` is the handle/mailbox, `ThreadRuntimeLoop` serializes ops, `ThreadSession` owns live state, and `Agent`/`AgentRunner` owns execution mechanics. This mirrors the Codex design closely enough that future work such as richer turn context, approval ops, event replay, and GUI streams can build on the same boundary.

## Current Implementation

Current ExAgent shape:

```text
AppServerService / AppServerBoundary
  -> ThreadManager
  -> loaded_threads: HashMap<SessionId, Arc<ThreadRuntime>>
  -> ThreadRuntime::spawn(...)
  -> ThreadSession::new(...)
  -> ThreadRuntimeLoop owns ThreadSession
  -> ThreadSession::handle_user_input(...)
  -> legacy Agent live-turn runner(&mut SessionSnapshot, ...)
```

Important files:

- `src/app_server/thread_manager.rs`
- `src/runtime/thread_runtime.rs`
- `src/runtime/thread_session/mod.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/thread_session/events.rs`
- `src/agent.rs`

What is good:

- `ThreadRuntime` is a handle/mailbox.
- `ThreadRuntimeLoop` serializes ops.
- `ThreadSession` is now long-lived and reused across turns.
- `submit_user_input_and_wait` supports CLI-style synchronous execution.
- `subscribe_events` supports GUI/app-server live consumers.
- `reserve_active_turn` rejects concurrent turns before actor state is touched.

Pre-P0 gaps this plan fixes:

- `thread_read` still reads disk directly in `thread_read_resolved`.
- `ThreadSession` does not expose a live read view.
- `Agent::run_legacy_session_snapshot` writes snapshot and assistant/tool events.
- `ThreadSession::append_and_broadcast` writes lifecycle events.
- Error handling calls `broadcast_events_since`, which rereads disk to find events the `Agent` wrote.
- `ThreadRuntime::submit(ThreadOp)` lets callers submit `UserInput` without reserving `active_turn`.

## Codex Reference Design

Codex uses a similar but more complete architecture:

```text
app-server
  -> codex_core::ThreadManager
  -> HashMap<ThreadId, Arc<CodexThread>>
  -> CodexThread
  -> Codex
  -> submission_loop
  -> Arc<Session>
  -> run_turn
```

Relevant reference files:

- `external-references/Codex/codex-rs/core/src/thread_manager.rs`
- `external-references/Codex/codex-rs/core/src/codex_thread.rs`
- `external-references/Codex/codex-rs/core/src/session/mod.rs`
- `external-references/Codex/codex-rs/core/src/session/session.rs`
- `external-references/Codex/codex-rs/core/src/session/handlers.rs`
- `external-references/Codex/codex-rs/core/src/session/turn.rs`

Codex responsibilities:

```text
ThreadManager
  - creates/resumes/forks threads
  - owns registry of loaded Arc<CodexThread>
  - shares process-level managers

CodexThread
  - lightweight handle around Codex
  - exposes submit(Op)
  - does not own session state

Codex
  - owns submission channel and event receiver
  - owns Arc<Session>
  - starts submission_loop

submission_loop
  - serializes Op handling
  - dispatches UserTurn, Interrupt, approvals, shutdown, etc.

Session
  - owns live session state
  - owns event sender
  - owns active turn state
  - owns mailbox and pending input
  - owns SessionServices
  - persists rollout items
  - sends events through send_event / send_event_raw

run_turn
  - executes a turn using Arc<Session> and TurnContext
  - builds tool runtime per turn
  - records history through Session
  - emits events through Session
```

The important Codex pattern is not simply "there is an app-server." The important pattern is:

```text
Session is the single runtime state and event owner.
Execution functions use Session APIs; they do not own persistence or client delivery.
```

Codex `Session::send_event` and `Session::send_event_raw` are the model to copy conceptually:

```text
send_event(...)
  -> build Event
  -> record trace/rollout metadata
  -> send_event_raw(...)

send_event_raw(...)
  -> persist event to rollout
  -> update agent status if relevant
  -> tx_event.send(event)
```

That is the shape ExAgent should move toward with `ThreadSession::record_event`.

## Architecture Comparison

Current ExAgent:

```text
ThreadManager
  -> ThreadRuntime
  -> ThreadRuntimeLoop
  -> ThreadSession
       owns Agent
       owns SessionSnapshot
       owns event_tx/status_tx
       writes lifecycle events
  -> Agent
       owns LLM
       owns ToolRegistry
       owns ExecSessionManager
       owns PolicyManager
       mutates snapshot
       writes snapshot
       writes assistant/tool events
```

Target ExAgent:

```text
ThreadManager
  -> owns loaded ThreadRuntime registry
  -> starts/resumes/forks threads
  -> asks runtime for live view when loaded

ThreadRuntime
  -> public handle/mailbox
  -> safe methods only: start_turn, interrupt, shutdown, subscribe, live_view
  -> does not expose raw UserInput submit

ThreadRuntimeLoop
  -> owns ThreadSession
  -> serializes ThreadOp
  -> no business logic beyond dispatch

ThreadSession
  -> owns SessionSnapshot
  -> owns event cursor
  -> owns event persistence and broadcast
  -> owns status and active turn view
  -> builds live ThreadRead view
  -> calls AgentRunner for execution

AgentRunner / Agent
  -> owns LLM call mechanics
  -> owns tool execution mechanics for now
  -> returns turn deltas
  -> does not write snapshot/events directly
```

Comparison to Codex:

```text
Codex ThreadManager       ~= ExAgent ThreadManager
Codex CodexThread         ~= ExAgent ThreadRuntime handle
Codex Codex               ~= ExAgent ThreadRuntime channel wrapper
Codex submission_loop     ~= ExAgent ThreadRuntimeLoop
Codex Session             ~= ExAgent ThreadSession
Codex run_turn            ~= ExAgent AgentRunner / Agent turn execution
Codex SessionServices     ~= future ExAgent session services/tool runtime facade
```

The current gap is that ExAgent's `Agent` still owns too much of what Codex puts behind `Session`.

## Desired Invariants

1. If a thread is loaded, reads go through the loaded runtime.
2. If no runtime is loaded, reads may fall back to disk.
3. `ThreadSession` is the only code path that assigns runtime event IDs.
4. `ThreadSession` is the only code path that appends live runtime events.
5. `ThreadSession` is the only code path that broadcasts live runtime events.
6. In the live runtime path, `Agent` may mutate only
   `snapshot.conversation`, `snapshot.open_exec_sessions`, and
   `snapshot.pending_approvals` in place.
7. `Agent` must not call `append_json_line` for runtime events.
8. `Agent` must not call `write_json` for session snapshots in the live runtime path.
9. Raw `ThreadOp::UserInput` cannot be submitted without reserving `active_turn`.
10. Interrupt uses the same active-turn state that `turn_start` reserves.
11. `ThreadSession` owns event id assignment, event persistence, event
    broadcast, snapshot checkpointing, live read publication, and status
    updates.

## Proposed Runtime Data Flow

Target turn flow:

```text
turn_start request
  -> ThreadManager::ensure_runtime_loaded(thread_id)
  -> ThreadRuntime::start_turn(...)
  -> reserve_active_turn(...)
  -> ThreadRuntimeLoop receives ThreadOp::UserInput
  -> ThreadSession::handle_user_input(...)
  -> ThreadSession::record_event(TurnStarted)
  -> AgentRunner::run_turn(...)
  -> returns TurnExecutionDelta
  -> ThreadSession applies delta to live SessionSnapshot
  -> ThreadSession records assistant/tool events
  -> ThreadSession writes snapshot checkpoint
  -> ThreadSession records TurnCompleted / RuntimeError / TurnInterrupted
```

Target read flow:

```text
thread_read request
  -> ThreadManager::runtime_for(thread_id)
      Some(runtime) -> runtime.live_view()
      None -> read snapshot/events from disk
```

Target event flow:

```text
ThreadSession::record_event(kind)
  -> assign next EventId
  -> append events.jsonl
  -> push to in-memory event buffer
  -> broadcast
  -> return RuntimeEvent
```

## Task 1: Lock the Expected Broken Behavior With Tests

**Files:**

- Modify: `tests/thread_runtime.rs`
- Modify: `tests/app_server_boundary.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_runtime.rs`

**Step 1: Add a test showing loaded runtime can expose live state without rereading disk**

Add a focused test that constructs a thread, loads runtime, runs a turn, and reads through a runtime live-view API.

Expected new API shape:

```rust
let view = runtime.live_view();
assert_eq!(view.thread_id, thread_id);
assert!(view.events.iter().any(|event| matches!(
    event.kind,
    RuntimeEventKind::TurnStarted
)));
```

This test can start as compile-failing because `live_view` does not exist.

**Step 2: Add an app-server test for `thread_read` using loaded runtime**

Add a test that proves `ThreadManager::thread_read` prefers runtime view when loaded.

The test should avoid depending on timing. Use a fake/session hook if necessary, or implement `live_view` first and assert that active turn/live events are visible through `thread_read`.

**Step 3: Run tests and verify failure**

Run:

```bash
cargo test thread_runtime
cargo test app_server_boundary
```

Expected: failure or compile error around missing live view behavior.

## Task 2: Add ThreadSession Live View

**Files:**

- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_session/events.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/app_server/thread_manager.rs`

**Step 1: Add a live view DTO internal to runtime**

Add an internal runtime view type. Keep it separate from app-server protocol DTOs.

```rust
pub(crate) struct ThreadSessionLiveView {
    pub thread_id: SessionId,
    pub snapshot: SessionSnapshot,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}
```

**Step 2: Store live events in ThreadSession**

Add:

```rust
events: Vec<RuntimeEvent>,
```

Initialize it from `events.jsonl` in `ThreadSession::new`.

**Step 3: Update `append_and_broadcast`**

Change it to push each event into `self.events` after append succeeds.

**Step 4: Expose `ThreadSession::live_view`**

Return cloned snapshot/events/status.

**Step 5: Expose `ThreadRuntime::live_view`**

`ThreadRuntime::live_view()` reads a shared live-state snapshot maintained by
`ThreadSession`. It intentionally does not enqueue a read op into
`ThreadRuntimeLoop`, because `thread/read` should not block behind a long
running turn.

**Step 6: Make `thread_read` prefer live runtime**

In `ThreadManager::thread_read_resolved`, first check `runtime_for(&thread_id)`. If present, call `runtime.live_view()` and build `ThreadReadResponse` from that. Fall back to disk only when absent.

## Task 3: Make ThreadSession the Event Owner

**Files:**

- Modify: `src/agent.rs`
- Modify: `src/runtime/thread_session/events.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `tests/agent_loop.rs`
- Modify: `tests/thread_runtime.rs`

**Step 1: Introduce a turn delta return type**

Add a type such as:

```rust
pub struct AgentTurnDelta {
    pub final_turn: AssistantTurn,
    pub messages: Vec<ConversationMessage>,
    pub event_kinds: Vec<RuntimeEventKind>,
}
```

The exact name can change, but the contract must be clear: no assigned event IDs, no writes, no broadcasts.

**Step 2: Split live execution from persistence**

Keep existing non-live `run`, `resume`, and fork APIs working. For the live path, replace `legacy live-turn runner` with a method that accepts mutable snapshot state and returns deltas without calling:

- `crate::transcript::append_json_line`
- `crate::transcript::append_runtime_event`
- `crate::transcript::write_json`

If removing writes from all `Agent` paths is too large, first isolate the live path and leave legacy one-shot paths intact.

**Step 3: Apply deltas in ThreadSession**

`ThreadSession::handle_user_input_inner` should:

1. append user message to live snapshot
2. record `TurnStarted`
3. call agent runner
4. append returned assistant/tool messages to live snapshot
5. record returned assistant/tool event kinds
6. write snapshot checkpoint
7. record `TurnCompleted`

**Step 4: Remove `broadcast_events_since`**

After `Agent` stops writing events directly, error paths should not reread disk. Remove or stop using:

```rust
ThreadSession::broadcast_events_since
ThreadSession::persisted_event_count
```

**Step 5: Verify event order**

Expected event order for a simple turn:

```text
TurnStarted
AssistantTurn
TurnCompleted
```

Expected event order for a tool turn:

```text
TurnStarted
AssistantTurn
ToolResult
AssistantTurn
TurnCompleted
```

Expected error path:

```text
TurnStarted
RuntimeError
```

## Task 4: Make Snapshot Persistence Explicit

**Files:**

- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/thread_session/events.rs`

**Step 1: Add `ThreadSession::checkpoint_snapshot`**

```rust
fn checkpoint_snapshot(&self) -> anyhow::Result<()> {
    crate::transcript::write_json(&self.paths.snapshot_path, &self.snapshot)
}
```

**Step 2: Call it only from ThreadSession**

Call after:

- user message is added
- assistant/tool messages are applied
- pending approval/open exec session changes are applied
- turn terminal state is recorded, if snapshot includes such state later

**Step 3: Add a grep-based guard**

Add a test or CI-friendly assertion that live-path event writes happen only from `thread_session`.

At minimum, manually verify before commit:

```bash
rg "append_json_line|append_runtime_event|write_json" src/agent.rs src/runtime src/tools
```

Expected: `src/agent.rs` should not write live runtime events or snapshots.

## Task 5: Lock Down ThreadRuntime API

**Files:**

- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `tests/thread_runtime.rs`

**Step 1: Make raw submit private or restricted**

Change:

```rust
pub async fn submit(&self, op: ThreadOp) -> Result<()>
```

to either:

```rust
async fn submit_internal(&self, submission: ThreadSubmission) -> Result<()>
```

or:

```rust
pub(crate) async fn submit_control(&self, op: ThreadControlOp) -> Result<()>
```

Do not allow external callers to send `ThreadOp::UserInput` without active-turn reservation.

**Step 2: Keep safe public methods**

Keep or add:

```rust
submit_user_input(...)
submit_user_input_and_wait(...)
interrupt_active_turn(...)
shutdown(...)
subscribe_events(...)
live_view(...)
```

**Step 3: Add a regression test**

Test that a running user input has an active turn and can be interrupted. Also test that concurrent turns are rejected before queueing.

## Task 6: Update Architecture Docs

**Files:**

- Modify: `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`
- Modify: `docs/architecture/2026-05-15-exagent-appserver-runtime-boundary-v1-design.md`
- Modify: `docs/plans/2026-05-16-exagent-thread-runtime-actor-v2.md`
- Modify: this document

**Step 1: Document final ownership**

State clearly:

```text
ThreadSession owns live state, event persistence, event broadcast, and live read view.
AgentRunner owns turn execution only.
ThreadRuntime owns mailbox/handle only.
ThreadRuntimeLoop owns serialization only.
ThreadManager owns registry/factory/resume/start only.
```

**Step 2: Document disk semantics**

State:

```text
When runtime is loaded, disk is not the authoritative read source.
Disk is recovery/replay/checkpoint storage.
When runtime is not loaded, disk is the fallback source.
```

**Step 3: Document Codex comparison**

Keep the comparison table from this plan aligned with the implementation.

## Task 7: Verification

Run:

```bash
cargo fmt -- --check
git diff --check
cargo test
```

Expected:

- formatting passes
- whitespace check passes
- all tests pass

Also run targeted tests while implementing:

```bash
cargo test thread_runtime
cargo test app_server_boundary
cargo test agent_loop
```

Architecture guards:

```bash
rg "append_json_line|append_runtime_event|write_json" src/agent.rs src/runtime src/tools
rg "snapshot\\." src/agent.rs | rg -v \
    "conversation|open_exec_sessions|pending_approvals|workspace_root|cwd|session_id|normalize_lineage"
```

The live runtime path should keep event persistence calls inside
`thread_session`. Matches in `src/agent.rs` are intentional only for the legacy
non-live path. Matches in `src/tools` must not occur for deferred live policy
events.
The second grep should only return fields explicitly allowed by the half-delta
contract, or intentional legacy non-live code.

## Suggested Commit Sequence

Use small commits:

1. `test: capture live thread read expectations`
2. `refactor: add thread session live view`
3. `refactor: move live event ownership into thread session`
4. `refactor: checkpoint live snapshots from thread session`
5. `refactor: restrict raw thread runtime submission`
6. `docs: update thread runtime ownership design`

## Success Criteria

This plan is complete when:

- `thread_read` uses live runtime state when loaded.
- `Agent` no longer writes live runtime events.
- `ThreadSession` is the only live event ID allocator.
- `ThreadSession` is the only live event broadcaster.
- `ThreadSession` checkpoints snapshot explicitly.
- Raw `ThreadOp::UserInput` cannot bypass `reserve_active_turn`.
- Existing CLI/app-server tests still pass.
- Docs explain the Codex comparison and ExAgent ownership model.
