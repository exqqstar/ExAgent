# ExAgent Thread Runtime Follow-Up Plan: Live State Completion And Approval Cancel Fix

**Date:** 2026-05-17
**Status:** Proposed
**Related:**
- `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`
- `docs/plans/2026-05-16-exagent-thread-runtime-actor-v2.md`
- `docs/plans/2026-05-17-exagent-thread-session-authoritative-runtime.md`
- `external-references/Codex/codex-rs/core/src/session/session.rs`

## Why This Plan Exists

The authoritative-runtime plan (2026-05-17) landed P0-P2 successfully:

- `thread_read` now prefers `runtime.live_view()` when a runtime is loaded.
- `ThreadSession` is the only writer for live runtime events.
- `legacy Agent live-turn runner` returns `event_kinds` deltas instead of writing events to disk.
- `broadcast_events_since` and `persisted_event_count` are gone.
- `checkpoint_snapshot()` is an explicit, single-callsite snapshot persistence path.
- Raw `submit(ThreadOp::UserInput)` is no longer reachable from public API.

A focused review of the resulting tree found one correctness bug and three
design gaps that prevent the live runtime model from being fully realized. They
are not regressions; they are remaining work that the authoritative-runtime
plan deferred or did not fully cover.

This plan captures them in one place so the next iteration is unambiguous.

## Summary Of Findings

| ID    | Severity | Area                            | One-line summary |
|-------|----------|---------------------------------|------------------|
| F1    | P0       | Interrupt correctness           | `ThreadSession::handle_interrupt` does not call `policy.cancel_pending_for_session`, so pending approval futures leak. |
| F2    | P1       | Live state completeness         | The live snapshot is stale during turn execution; readers see no progress until the turn completes. |
| F3    | P1       | State ownership shape           | `live_state` is a shadow copy maintained by manual republish; any future write path that forgets to publish will silently break reader views. |
| F4    | P1       | Agent / Session contract clarity| Agent claims to return deltas but still mutates `&mut SessionSnapshot` in place for conversation, exec sessions, and pending approvals. The boundary is half-delta and undocumented. |
| F5a   | P2       | Turn id ownership               | `next_turn_id` reads disk in `ThreadManager`; it should live in `ThreadSession`. |
| F5b   | P2       | Memory bound                    | `ThreadSession::events` grows without bound. |
| F5c   | P2       | API ergonomics                  | `live_view()` returns `Result` only for mutex poisoning; the shape misleads callers. |
| F5d   | P2       | Lifecycle safety                | `mark_stopped` is called after the loop's normal exit but not on panic; subscribers can hang on `wait_until_terminated`. |
| F5e   | P2       | API tidiness                    | `AgentLiveTurnOutput` exposes `session_id`, `snapshot_path`, `events_path` that `ThreadSession` already owns. |

## Problem 1 (F1, P0): Interrupt Leaks Pending Policy Approvals

### Symptom

When a runtime is loaded and a turn is waiting on an approval, calling
`turn/interrupt` routes into `ThreadSession::handle_interrupt`. That handler
clears `snapshot.pending_approvals`, persists the snapshot, and records
`TurnInterrupted`. It does not, however, tell `PolicyManager` that the
approvals are cancelled.

The disk-fallback path used when no runtime is loaded does the cancellation:

```rust
// src/app_server/thread_manager.rs (disk fallback)
self.policy
    .cancel_pending_for_session(&params.thread_id)
    .await;
crate::transcript::append_runtime_event(
    workspace_root,
    &params.thread_id,
    Some(&turn_id),
    RuntimeEventKind::TurnInterrupted,
)?;
```

A grep confirms only one caller:

```text
src/policy.rs:123                            (definition)
src/app_server/thread_manager.rs:460         (only caller)
```

The runtime path skips this entirely.

### Root Cause

`ThreadSession` was extracted from `ThreadManager` without a reference to
`PolicyManager`. `ThreadSession::new` takes `AgentConfig` and an
`AgentFactory`, but the agent factory does not expose the policy manager to
its caller after construction.

### Consequence

Tools that registered a pending approval through `PolicyManager` continue to
hold a `tokio::sync::Notify` waiter even after the interrupt clears the
snapshot side. The next `Notify::notified().await` inside the tool body
remains pending until the process exits. Symptoms a user would see:

- Interrupt returns success.
- The thread becomes `Idle` from the runtime's point of view.
- A subsequent `turn/start` succeeds.
- The previous tool execution sits in a leaked future, holding any borrowed
  state, until process restart.

This is a real correctness bug, not a cosmetic gap. It silently degrades the
runtime across long-lived sessions.

### Fix

1. Add a `policy: Arc<PolicyManager>` field to `ThreadSession`.
2. Extend `ThreadSessionOptions` with a builder method `with_policy(...)` so
   `ThreadManager::ensure_runtime_loaded` can inject the same
   `Arc<PolicyManager>` it already passes into `Agent`.
3. In `ThreadSession::handle_interrupt`, after persisting the snapshot and
   before appending `TurnInterrupted`, call:

```rust
self.policy
    .cancel_pending_for_session(&self.thread_id)
    .await;
```

4. Add a regression test in `tests/thread_runtime.rs` that:
   - registers a pending approval through a fake tool;
   - submits an interrupt op to the runtime;
   - asserts `policy.pending_count(&thread_id)` is zero after the interrupt
     completes (add a small `#[cfg(test)]` accessor on `PolicyManager` if
     none exists).

### After

The runtime path and the disk-fallback path become behaviourally identical:
both clear snapshot-side approvals, cancel policy-side waiters, and record
`TurnInterrupted`. The disk-fallback path can eventually be deleted once
runtimes are always loaded on interrupt, but that is a separate cleanup.

## Problem 2 (F2, P1): Live Snapshot Goes Stale Mid-Turn

### Symptom

During `ThreadSession::handle_user_input`, the visible sequence is:

```text
1. snapshot.conversation.push(user_message)
2. checkpoint_snapshot()              -- live_state.snapshot now includes user msg
3. agent.legacy live-turn runner(&mut snapshot, ...)
     - LLM call (seconds)
     - tool call (seconds)
     - LLM call again
     - ...
   self.snapshot accumulates assistant/tool messages during this time,
   but live_state.snapshot is NOT republished.
   event_kinds are accumulated inside Agent and not yet recorded.
4. for kind in output.event_kinds { append_and_broadcast(...) }
5. checkpoint_snapshot()              -- live_state finally catches up
6. append_and_broadcast(TurnCompleted)
```

A `thread_read` issued between steps 3 and 4 returns:

- `snapshot.conversation`: only the user message
- `events`: `[TurnStarted]`
- `status`: `Running`

After several seconds, the reader suddenly sees a burst of
`AssistantTurn`, `ToolResult`, more `AssistantTurn`, then `TurnCompleted`.

### Root Cause

`legacy Agent live-turn runner` accumulates `event_kinds: Vec<RuntimeEventKind>`
locally and returns the whole vector at the end of the turn. The
ThreadSession-as-event-owner refactor moved persistence into `ThreadSession`
but kept Agent's batched return shape, so events are only recorded after the
turn finishes.

### Consequence

The actor model's central value -- a live event stream that mirrors agent
progress in real time -- is not realized for the most interesting part of a
turn (the LLM and tool execution). GUI clients cannot show streaming
progress; `tail -f`-style debugging on the event log is bursty; the live
snapshot is misleading.

This does not violate the durable contract -- on-disk state still ends up
correct -- but it breaks the spirit of the "ThreadSession is authoritative"
invariant for any reader observing a turn in flight.

### Fix

Two equally reasonable approaches; pick one and document the choice.

**Option A: EventSink callback (smaller change)**

Replace the `Vec<RuntimeEventKind>` return value with a sink the agent
calls for each event:

```rust
#[async_trait]
pub trait LiveEventSink: Send {
    async fn record(&mut self, kind: RuntimeEventKind) -> Result<RuntimeEvent>;
}

impl Agent {
    pub(crate) async fn legacy live-turn runner<S: LiveEventSink>(
        &self,
        snapshot: &mut SessionSnapshot,
        turn_id: TurnId,
        turn_cwd: Option<PathBuf>,
        sink: &mut S,
    ) -> Result<AgentLiveTurnOutput>;
}
```

`ThreadSession` implements `LiveEventSink` by calling its own
`append_and_broadcast`. Each `AssistantTurn` and `ToolResult` is recorded
the moment Agent produces it.

This is the closer mapping to Codex's `Session::send_event` /
`Session::send_event_raw` pattern.

**Option B: mpsc channel (looser coupling)**

Agent owns a `tokio::sync::mpsc::Sender<RuntimeEventKind>`; ThreadSession
drains it concurrently with the turn future:

```rust
let (event_tx, mut event_rx) = mpsc::channel(64);
let turn_future = self.agent.legacy live-turn runner(&mut self.snapshot, ..., event_tx);
tokio::select! {
    result = turn_future => { ... }
    Some(kind) = event_rx.recv() => self.append_and_broadcast(...)
}
```

This avoids a trait but introduces ordering subtleties (the drain must keep
up with the producer and must continue after the turn future returns).

**Recommendation:** Option A. It is local, async-friendly, mirrors Codex
exactly, and does not introduce a new background drain.

In addition, call `checkpoint_snapshot()` after each assistant message and
each tool result is appended to `snapshot.conversation`. Today the snapshot
is only checkpointed once at the end of the turn. The cost is N small JSON
writes per turn instead of one, which is acceptable for typical turn sizes
and matches Codex's per-event rollout persistence.

### After

A reader polling `thread_read` mid-turn sees:

```text
events: [TurnStarted, AssistantTurn, ToolResult]
snapshot.conversation: [user, assistant, tool]
status: Running
```

increasing event-by-event, exactly mirroring what subscribers on the
broadcast channel see.

## Problem 3 (F3, P1): live_state Is A Shadow Copy

### Symptom

`ThreadSession` carries two parallel representations of the same data:

```rust
pub struct ThreadSession {
    ...
    snapshot: SessionSnapshot,
    events: Vec<RuntimeEvent>,
    live_state: Arc<Mutex<ThreadSessionLiveState>>, // mirrors snapshot + events + status
    ...
}
```

The invariant "`live_state.snapshot == self.snapshot` and
`live_state.events == self.events`" is upheld by convention. Every mutating
path must remember to call `publish_live_snapshot()` or assign
`live_state.events = self.events.clone()`.

In addition, every event push clones the entire events vector into the
shared mirror:

```rust
// src/runtime/thread_session/events.rs
self.events.push(event.clone());
if let Ok(mut live_state) = self.live_state.lock() {
    live_state.events = self.events.clone();   // O(N) per event
}
```

### Consequence

Two distinct problems:

1. **Silent invariant violation.** If a future change adds a new method that
   mutates `self.snapshot` without going through `checkpoint_snapshot`, or
   pushes to `self.events` without the manual republish, live readers will
   see stale state. The compiler cannot catch this; tests must be written
   for each new write path.

2. **Memory churn.** For a thread with N events, each new event triggers an
   O(N) clone of the entire event vec. Snapshot republish similarly clones
   the entire `SessionSnapshot` (which includes the whole conversation
   history) on every checkpoint. Long-running threads pay this on every
   turn.

### Root Cause

When `live_view()` was added, it needed to return data to external callers
without blocking the actor mailbox. A separate mirror behind a sync `Mutex`
was the smallest change that delivered that. The trade-off is now visible.

### Fix

Replace the dual representation with a single source of truth behind
`Arc<RwLock<...>>`:

```rust
pub struct ThreadSessionState {
    pub snapshot: SessionSnapshot,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}

pub struct ThreadSession {
    ...
    state: Arc<RwLock<ThreadSessionState>>,
    ...
}
```

- All actor writes take `state.write()` briefly to push an event, append a
  message, or update status. No second copy exists.
- `ThreadRuntime::live_view()` takes `state.read()`, clones what the caller
  needs (snapshot + events), and drops the lock.
- The duplicated `snapshot: SessionSnapshot`, `events: Vec<RuntimeEvent>`
  fields on `ThreadSession` are deleted; all reads go through the lock.

To avoid the O(N) clone on every event push:

- Keep `events: Vec<RuntimeEvent>` inside the lock; do not republish.
- For `live_view()`, clone the vec only when a reader actually asks.
- For broadcasters, continue to send each `RuntimeEvent` (a single event
  clone) through the existing `broadcast::Sender`.

`tokio::sync::RwLock` is the right choice here, not `std::sync::RwLock`,
because `handle_interrupt` will become `async` once it calls
`policy.cancel_pending_for_session().await` (see F1). If async holding is
undesirable, fall back to `std::sync::RwLock` and ensure no `.await` happens
while a lock is held.

### After

```rust
// Single mutating path
{
    let mut state = self.state.write().await;
    state.events.push(event.clone());
    state.snapshot.conversation.push(message);
}

// Single reading path
let view = {
    let state = self.state.read().await;
    ThreadSessionLiveView {
        thread_id: self.thread_id.clone(),
        snapshot: state.snapshot.clone(),
        events: state.events.clone(),
        status: state.status,
    }
};
```

The invariant is now structural -- there is no second copy that can fall
out of sync. Future writers physically cannot bypass the publish step
because there is no publish step.

## Problem 4 (F4, P1): Agent / Session Contract Is Half-Delta

### Symptom

The authoritative-runtime plan describes Agent as returning "deltas". The
code now returns `event_kinds: Vec<RuntimeEventKind>` as a delta, but Agent
still mutates the snapshot directly:

```rust
// src/agent.rs, inside run_live_session_snapshot
snapshot.conversation.push(assistant_message);          // in-place
apply_exec_session_update(snapshot, &result);           // in-place
apply_pending_approval_update(snapshot, &result);       // in-place
snapshot.conversation.push(tool_message);               // in-place
```

So the actual contract is:

- Events: delta, returned to ThreadSession to persist + broadcast.
- Conversation, exec sessions, pending approvals: in-place mutation of the
  borrowed `&mut SessionSnapshot`.

### Consequence

Two related risks:

1. **Documentation drift.** A future reader of the design doc will expect
   Agent to be side-effect-free over snapshot. They will refactor
   `&mut SessionSnapshot` to `&SessionSnapshot` "to match the contract",
   and silently lose all conversation history for the live path.

2. **No single integration point for snapshot mutations.** When we later
   want to checkpoint after each conversation message (see F2), we cannot
   intercept those pushes because Agent owns them. If snapshot mutations
   were also deltas, ThreadSession could checkpoint between each one.

### Root Cause

The refactor extracted event persistence into ThreadSession but did not
extract the rest. The result is a partial migration that is invisible from
the design docs.

### Fix

Two options, pick one and write it down.

**Option A: Document the half-delta as intentional (smaller change).**

Add an explicit "Agent ownership matrix" to the V2 design doc and to the
authoritative-runtime invariants:

```text
Agent owns (in-place mutates) during a live turn:
  - snapshot.conversation
  - snapshot.open_exec_sessions
  - snapshot.pending_approvals

ThreadSession owns:
  - event id assignment
  - events.jsonl append
  - event broadcast
  - snapshot.json persistence
  - thread status updates
```

This is the lower-risk path. It makes the existing code correct by
specification.

**Option B: Full delta (larger change).**

Define a richer return type:

```rust
pub struct AgentTurnDelta {
    pub final_turn: AssistantTurn,
    pub conversation_appends: Vec<ConversationMessage>,
    pub exec_session_updates: Vec<ExecSessionUpdate>,
    pub pending_approval_updates: Vec<PendingApprovalUpdate>,
    pub events: Vec<RuntimeEventKind>,
}
```

Agent takes `&SessionSnapshot` (read-only) and returns the full delta.
ThreadSession applies it, with the option to checkpoint between deltas and
broadcast each event before applying the next.

This makes Agent purely functional in the live path and gives ThreadSession
maximum control. It is also a much bigger diff.

**Recommendation:** Option A in this iteration. Combine it with the F2 fix
(EventSink) so that the visible behaviour is correct even though the
conversation update path remains in-place. Revisit Option B only if a real
need appears (e.g., write-ahead delta logs, transactional turn rollback).

### After

- The design doc explicitly states what Agent is allowed to mutate.
- The authoritative-runtime invariants gain a tenth entry:

```text
10. Agent may mutate snapshot.conversation, snapshot.open_exec_sessions,
    and snapshot.pending_approvals in place during legacy live-turn runner.
    No other in-place mutation of snapshot is allowed from Agent.
```

- A grep guard is added to the verification section of this plan:

```bash
rg "snapshot\." src/agent.rs | rg -v \
    "conversation|open_exec_sessions|pending_approvals|workspace_root|cwd|session_id|normalize_lineage"
```

## Problem 5 (F5, P2): Small Cleanups

### F5a: Move turn id minting into ThreadSession

`next_turn_id` reads `snapshot.json` from disk inside `ThreadManager`. The
loaded runtime already owns the latest conversation. Move the helper:

```rust
impl ThreadSession {
    pub(crate) fn next_turn_id(&self) -> TurnId {
        let assistant_count = self
            .snapshot
            .conversation
            .iter()
            .filter(|m| matches!(m.role, MessageRole::Assistant))
            .count();
        TurnId::new(format!("turn_{}", assistant_count + 1))
    }
}
```

Expose it through `ThreadRuntime::next_turn_id()`. `ThreadManager` calls
that and removes the disk read.

### F5b: Bound the in-memory event buffer

`ThreadSession::events` grows without bound. For long-running threads this
becomes a memory issue and slows down `live_view()`'s clone.

Add a configurable cap (default `2048`):

```rust
const DEFAULT_LIVE_EVENT_BUFFER_CAP: usize = 2048;
```

When the buffer exceeds the cap, drop the oldest events from the in-memory
copy. Persisted events on disk are not affected; `events/replay` always
reads from disk. Live subscribers continue to receive new events; only the
`live_view().events` window is bounded.

### F5c: Right-size `live_view()`'s return type

`live_view()` currently returns `Result<ThreadSessionLiveView>` whose only
error is mutex poisoning. Mutex poisoning means the actor has panicked, in
which case the runtime is unusable and a panic is acceptable. Change to:

```rust
pub fn live_view(&self) -> ThreadSessionLiveView
```

Panicking on poison is correct here; callers cannot meaningfully recover.

After the F3 fix this becomes:

```rust
pub async fn live_view(&self) -> ThreadSessionLiveView
```

(async because `RwLock::read` is async).

### F5d: Make `mark_stopped` panic-safe

`ThreadRuntimeLoop::run` calls `self.session.mark_stopped()` after the loop
exits normally. If a handler panics, the panic propagates and
`mark_stopped` never runs. Subscribers blocked on
`wait_until_terminated()` hang forever.

Wrap the loop in an `AbortGuard` that fires `mark_stopped()` from `Drop`:

```rust
struct StoppedGuard<'a> {
    session: &'a ThreadSession,
}

impl Drop for StoppedGuard<'_> {
    fn drop(&mut self) {
        self.session.mark_stopped();
    }
}

async fn run(mut self) {
    let _stopped = StoppedGuard { session: &self.session };
    while let Some(submission) = self.op_rx.recv().await { ... }
}
```

### F5e: Trim `AgentLiveTurnOutput`

`AgentLiveTurnOutput.session_id`, `snapshot_path`, and `events_path` are
all derivable by ThreadSession from its own fields. Remove them; keep
`final_turn` and (until F2 lands) `event_kinds`. This avoids implying that
the Agent layer has any authority over those identifiers.

## Architecture Decisions

This plan implies four explicit architecture choices. They are recorded
here so the next person changing the runtime does not undo them.

### Decision 1: ThreadSession is the EventSink For Live Turns (F2)

Agent does not own event ids, persistence, or broadcast. It produces
typed event values and hands them to ThreadSession, one at a time, during
turn execution. Codex's `Session::send_event` is the reference.

Trade-off: Agent's live path takes a generic sink parameter, slightly
complicating the type signature. Benefit: live progress matches reality,
and there is one and only one event-recording code path.

### Decision 2: Single Source Of Truth Via `Arc<RwLock<ThreadSessionState>>` (F3)

The shadow-copy mirror is replaced by one writable state behind an
`RwLock`. Every reader and writer goes through the lock; no manual
publish step exists.

Trade-off: `live_view()` becomes async (or holds a sync lock briefly).
Benefit: it is structurally impossible for a future write path to forget
the publish step, because the publish step is the write step.

### Decision 3: Documented Half-Delta Agent Contract (F4)

Agent is allowed to mutate `conversation`, `open_exec_sessions`, and
`pending_approvals` in place. Agent is not allowed to write events,
write the snapshot file, or assign event ids.

Trade-off: the design is not purely functional. Benefit: avoids a
multi-thousand-line refactor that would not change observable behaviour
once F2 lands.

If a future feature needs transactional rollback or write-ahead delta
logging, revisit and consider full deltas.

### Decision 4: Turn id Lives Inside ThreadSession (F5a)

The loaded runtime owns the latest conversation in memory. Turn id minting
should be local to that state, not pulled from disk on every
`turn_start` call.

Trade-off: ThreadRuntime exposes one more method. Benefit: turn id
generation is consistent with the rest of the live ownership model and
removes an unnecessary disk read from the hot path.

## Implementation Sequence

Each step is a single commit and a single conceptual change. Follow them
in order so each commit is independently testable.

### Step 1: F1 -- approval cancel bug (P0, blocking)

1. Add `policy: Arc<PolicyManager>` to `ThreadSession`.
2. Add `with_policy(...)` to `ThreadSessionOptions`.
3. Wire it through `ThreadManager::runtime_agent_factory`'s neighbouring
   construction site so the same policy instance is shared.
4. Call `self.policy.cancel_pending_for_session(&self.thread_id).await`
   from `handle_interrupt` before appending `TurnInterrupted`.
5. Add a regression test:
   `thread_runtime_interrupt_cancels_pending_policy_approvals`.
6. Commit: `fix: cancel policy approvals on runtime interrupt`.

### Step 2: F5d -- panic-safe stopped guard (cheap and isolated)

1. Add `StoppedGuard` in `thread_runtime.rs`.
2. Remove explicit `self.session.mark_stopped()` at end of `run`.
3. Add a test that drops the runtime mid-poll and asserts
   `wait_until_terminated()` returns.
4. Commit: `fix: notify stopped on runtime loop drop`.

### Step 3: F2 -- streaming events through ThreadSession (P1, biggest)

1. Define `LiveEventSink` trait in `runtime/thread_session/events.rs`.
2. Implement it for `ThreadSession`.
3. Change `legacy Agent live-turn runner` signature to take `&mut impl LiveEventSink`
   and call `sink.record(kind).await` instead of pushing to
   `event_kinds`.
4. Remove the `event_kinds` field from `AgentLiveTurnOutput`.
5. Add `checkpoint_snapshot()` calls after each assistant message and
   each tool result is pushed into `snapshot.conversation`.
6. Update tests in `tests/thread_runtime.rs` and `agent_loop.rs` to
   reflect the per-event recording order.
7. Add a test that polls `live_view()` mid-turn and asserts events
   accumulate progressively.
8. Commit: `refactor: stream live events through thread session`.

### Step 4: F3 -- single source of truth (medium)

1. Define `ThreadSessionState { snapshot, events, status }`.
2. Move state behind `Arc<RwLock<ThreadSessionState>>`.
3. Delete the duplicate `snapshot` and `events` fields on `ThreadSession`.
4. Update `append_and_broadcast` to take a write lock, push, and broadcast.
5. Update `checkpoint_snapshot` to take a read lock for serialization.
6. Update `live_view_from_state` to take a read lock and clone-out.
7. Update tests where direct field access was used.
8. Commit: `refactor: collapse thread session live state into one lock`.

### Step 5: F4 -- document the half-delta contract (text only)

1. Add the ownership matrix section to
   `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`.
2. Add invariant #10 to
   `docs/plans/2026-05-17-exagent-thread-session-authoritative-runtime.md`.
3. Add the grep guard to the Verification section of this plan and the
   authoritative-runtime plan.
4. Commit: `docs: document agent snapshot ownership boundary`.

### Step 6: F5a -- turn id ownership (small)

1. Move `next_turn_id` into `ThreadSession`.
2. Expose `ThreadRuntime::next_turn_id() -> TurnId`.
3. `ThreadManager::run_turn_through_runtime` and
   `start_turn_in_background` call the runtime method instead.
4. Delete the disk-reading `next_turn_id` helper.
5. Commit: `refactor: move turn id minting into thread session`.

### Step 7: F5b -- bounded event buffer (small)

1. Add `DEFAULT_LIVE_EVENT_BUFFER_CAP`.
2. In `append_and_broadcast`, trim from the front when the cap is
   exceeded.
3. Add a test asserting that `events.len()` stays bounded across many
   events but `events/replay` still returns all of them.
4. Commit: `feat: bound live event buffer`.

### Step 8: F5c, F5e -- API tidying (cosmetic)

1. Change `live_view()` to return `ThreadSessionLiveView` directly,
   panicking on poison.
2. Remove `session_id`, `snapshot_path`, `events_path` from
   `AgentLiveTurnOutput`.
3. Update callers.
4. Commit: `refactor: tidy live view and agent turn output shapes`.

## Verification

After each commit, run:

```bash
cargo fmt -- --check
cargo test
git diff --check
```

Additional targeted checks at the end:

```bash
# Live path event/snapshot writes must only originate from thread_session
rg "append_json_line|append_runtime_event|write_json" src/agent.rs src/runtime src/tools

# Agent must not touch fields outside the documented half-delta surface
rg "snapshot\." src/agent.rs | rg -v \
    "conversation|open_exec_sessions|pending_approvals|workspace_root|cwd|session_id|normalize_lineage"
```

The first grep may return intentional matches inside `ThreadSession`, tests,
or legacy non-live `run_legacy_session_snapshot` code. It must not show live policy
events being persisted directly from tools. The second grep should return empty
or only fields explicitly allowed by the half-delta contract.

Manual smoke test:

```bash
cargo run -- run "what time is it"
```

Confirm the CLI prints the assistant text and exits without warnings about
leaked tokio tasks or pending policy waiters.

## Summary

| Finding | Fix Outcome |
|---------|-------------|
| F1 Pending policy approvals leak after runtime interrupt | Runtime and disk interrupt paths become behaviourally identical. No leaked PolicyManager waiters. |
| F2 Live snapshot/events stale during turn execution | Each event is recorded, persisted, broadcast, and visible to `thread_read` the moment Agent produces it. Snapshot is checkpointed after each conversation push. |
| F3 Shadow copy with manual republish | One `Arc<RwLock<ThreadSessionState>>` owns snapshot/events/status. No invariant maintained by convention. No O(N) republish per event. |
| F4 Agent half-delta contract is undocumented | An explicit ownership matrix in the design doc and a grep guard in CI prevent future drift. |
| F5a Turn id read from disk | Turn id is minted from the in-memory snapshot held by ThreadSession. |
| F5b Unbounded event buffer | In-memory buffer is capped; disk replay remains complete. |
| F5c `live_view()` returns Result for poison | Returns value directly; poison is a panic. |
| F5d `mark_stopped` not panic-safe | Drop guard ensures the runtime always reports terminated. |
| F5e `AgentLiveTurnOutput` exposes session-owned fields | Output is trimmed to what only Agent can know. |

After all eight commits, the live runtime invariants from
`2026-05-17-exagent-thread-session-authoritative-runtime.md` become both
true and structurally enforced rather than upheld by convention. The next
phase of work -- richer turn context, streaming token deltas, approval
routing -- can build directly on this surface without reopening the
ownership question.
