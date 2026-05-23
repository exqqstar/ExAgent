# ExAgent Runtime Overlay Separation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate durable session history from live-only runtime state so cold rollout replay never recreates fake approvals, running exec sessions, or active turns.

**Architecture:** Keep `rollout.jsonl` as the only durable source of truth. `ThreadSession` owns a stateful `ContextManager` for prompt-visible history and a `RuntimeOverlay` for live-only state. `ThreadView` is a projection of durable state plus the current runtime overlay; cold storage reads use an empty overlay.

**Tech Stack:** Rust, Tokio, serde JSONL, existing `ThreadSession`, `ThreadRuntime`, `ContextManager`, `RolloutStore`, `SessionSnapshot`, `RuntimeEvent`, `ThreadManager`, app-server boundary tests.

## Implementation Status

This plan has been implemented in the current working tree.

The important outcome is:

```text
rollout.jsonl remains the only durable source of truth
ContextManager remains the prompt-visible history owner
RuntimeOverlay owns live-only approvals and persistent exec refs
cold reads reconstruct durable history with RuntimeOverlay::default()
loaded reads project durable state plus the live RuntimeOverlay
```

## Current Problem

The rollout migration already made the important behavioral choice:

```text
rollout cold replay restores conversation/context/compaction
rollout cold replay does not restore pending_approvals/open_exec_sessions
```

That behavior is correct. The remaining problem is that `SessionSnapshot` still contains both durable fields and live-only fields:

```text
Durable-ish:
  session_id
  workspace_root
  cwd
  reference_turn_context
  conversation
  latest_compaction

Live-only:
  open_exec_sessions
  pending_approvals
```

This makes the type look like complete persisted session state, even though `open_exec_sessions` and `pending_approvals` are only meaningful while a `ThreadSession` process is alive.

## Target Invariant

Do not introduce another storage file. This is an in-memory responsibility split, not a persistence split.

```text
Disk:
  .exagent/threads/<thread_id>/rollout.jsonl

ThreadSession memory:
  ContextManager
    - prompt-visible history
    - reference_turn_context

  Durable materialized view
    - SessionSnapshot-compatible history metadata

  RuntimeOverlay
    - active/live approvals
    - active/live exec sessions
    - future live-only control state

App-server projection:
  ThreadView = durable materialized view + RuntimeOverlay + live events
```

Live runs do not reread files:

```text
ThreadSession starts:
  read rollout.jsonl once
  hydrate ContextManager
  hydrate durable materialized view
  RuntimeOverlay::default()

Turn runs:
  update ContextManager
  update RuntimeOverlay
  append durable items to rollout.jsonl

Loaded thread_read:
  read ThreadSession memory only

Cold thread_read:
  read rollout.jsonl
  use RuntimeOverlay::default()
```

## Codex Alignment

Codex keeps pending approvals and similar active callbacks inside turn-scoped runtime state, not rollout replay state. The durable rollout rebuilds history, `TurnContextItem`, compaction checkpoints, and selected historical events. It does not reconstruct `oneshot::Sender`, child process handles, stdin handles, cancellation tokens, or live subscribers.

ExAgent should follow the same rule:

```text
rollout tells us what happened
RuntimeOverlay tells us what is alive now
ContextManager tells the model what to see next
```

## Non-Goals

Do not do these in this implementation:

```text
Do not add a second runtime state file.
Do not persist child process handles or approval waiters.
Do not make cold replay produce pending approvals.
Do not make cold replay produce running exec sessions.
Do not implement compaction in this change.
Do not remove legacy transcript migration in this change.
```

## Task 1: Add Cold Replay Invariant Tests

**Files:**

- Modify: `src/state/rollout.rs`
- Modify: `tests/app_server_boundary.rs`
- Modify: `tests/thread_runtime.rs`

**Step 1: Write a failing rollout reconstruction test**

Add a test proving `snapshot_from_rollout_items` ignores live-only event history:

```rust
#[test]
fn rollout_snapshot_does_not_restore_live_only_runtime_state() {
    let thread_id = SessionId::new("session_overlay_cold");
    let workspace_root = PathBuf::from("/tmp/exagent-overlay");
    let snapshot = SessionSnapshot::new_thread(
        thread_id.clone(),
        workspace_root.clone(),
        workspace_root.clone(),
    );
    let approval_id = ApprovalId::new("approval_1");

    let items = vec![
        RolloutItem::SessionMeta(session_meta_from_snapshot(&snapshot)),
        RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_1"),
            session_id: thread_id.clone(),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::ApprovalRequested {
                approval_id,
                tool_name: "run_command".to_string(),
                reason: "approval required".to_string(),
            },
        }),
    ];

    let rebuilt = snapshot_from_rollout_items(&thread_id, &items).expect("rebuild snapshot");
    assert!(rebuilt.pending_approvals.is_empty());
    assert!(rebuilt.open_exec_sessions.is_empty());
}
```

**Step 2: Run the focused test**

Run:

```bash
cargo test rollout_snapshot_does_not_restore_live_only_runtime_state --lib
```

Expected: PASS today if current behavior remains correct.

**Step 3: Add an app-server cold-read test**

Add a boundary test that writes a rollout containing an `ApprovalRequested` event, then calls `thread_read` without loading a runtime. Assert the returned thread is not `WaitingApproval`.

Expected behavior:

```text
history can show the approval request
thread status must not be WaitingApproval
no current actionable approval is exposed
```

**Step 4: Run focused app-server tests**

Run:

```bash
cargo test --test app_server_boundary cold_thread_read_does_not_restore_historical_approval_as_waiting
```

Expected: initially FAIL if `thread_read` derives `WaitingApproval` from cold reconstructed snapshot incorrectly; PASS after projection is fixed.

## Task 2: Introduce RuntimeOverlay

**Files:**

- Create: `src/runtime/thread_session/overlay.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_session/events.rs`

**Step 1: Add the overlay type**

Create `src/runtime/thread_session/overlay.rs`:

```rust
use crate::session::{ExecSessionRef, PendingApproval};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RuntimeOverlay {
    pub(crate) open_exec_sessions: Vec<ExecSessionRef>,
    pub(crate) pending_approvals: Vec<PendingApproval>,
}
```

**Step 2: Wire it into the module**

In `src/runtime/thread_session/mod.rs`:

```rust
pub mod events;
pub(crate) mod overlay;
pub mod turn;

pub(crate) use overlay::RuntimeOverlay;
```

**Step 3: Extend live state and live view**

Change `ThreadSessionLiveState` and `ThreadSessionLiveView`:

```rust
pub struct ThreadSessionLiveView {
    pub thread_id: SessionId,
    pub snapshot: SessionSnapshot,
    pub overlay: RuntimeOverlay,
    pub events: Vec<RuntimeEvent>,
    pub status: ThreadRuntimeStatus,
}

pub(crate) struct ThreadSessionLiveState {
    pub(crate) snapshot: SessionSnapshot,
    pub(crate) overlay: RuntimeOverlay,
    pub(crate) events: Vec<RuntimeEvent>,
    pub(crate) status: ThreadRuntimeStatus,
}
```

Initialize with `RuntimeOverlay::default()` when constructing `ThreadSession`.

**Step 4: Run compile check**

Run:

```bash
cargo test --test thread_runtime
```

Expected: may fail until all live state initializers include `overlay`.

## Task 3: Move Live-Only Mutations Out Of SessionSnapshot

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/thread_session/overlay.rs`
- Test: `tests/thread_runtime.rs`

**Step 1: Add overlay mutation helpers**

In `overlay.rs`, add methods:

```rust
use crate::runtime::tool_call_runtime::{
    ApprovalUpdate, ExecSessionUpdate,
};
use crate::session::{
    ApprovalStatus, ExecSessionRef, ExecSessionStatus, PendingApproval,
};
use crate::types::EventId;

impl RuntimeOverlay {
    pub(crate) fn apply_exec_session_update(&mut self, update: ExecSessionUpdate) {
        let exec_session_id = match &update {
            ExecSessionUpdate::Running { exec_session_id, .. }
            | ExecSessionUpdate::NotRunning { exec_session_id } => exec_session_id.clone(),
        };
        self.open_exec_sessions
            .retain(|entry| entry.exec_session_id != exec_session_id);

        if let ExecSessionUpdate::Running {
            exec_session_id,
            command,
            cwd,
        } = update
        {
            self.open_exec_sessions.push(ExecSessionRef {
                exec_session_id,
                command,
                cwd,
                status: ExecSessionStatus::Running,
            });
        }
    }

    pub(crate) fn apply_approval_requested(
        &mut self,
        approval_id: crate::session::ApprovalId,
        requested_event_id: EventId,
        tool_name: String,
        reason: String,
    ) {
        self.pending_approvals
            .retain(|entry| entry.approval_id != approval_id);
        self.pending_approvals.push(PendingApproval {
            approval_id,
            requested_event_id,
            tool_name,
            reason,
            status: ApprovalStatus::Pending,
        });
    }

    pub(crate) fn clear_approval(&mut self, approval_id: &crate::session::ApprovalId) {
        self.pending_approvals
            .retain(|entry| &entry.approval_id != approval_id);
    }
}
```

Keep exact imports adjusted to the final code shape.

**Step 2: Update turn effect application**

Change `apply_exec_session_update` and `apply_approval_update` so they mutate `RuntimeOverlay`, not `SessionSnapshot`.

Desired behavior:

```text
ToolEffect::ExecSessionUpdate
  -> RuntimeOverlay.open_exec_sessions

ToolEffect::ApprovalUpdate::Requested
  -> reserve event id
  -> RuntimeOverlay.pending_approvals
  -> record ApprovalRequested event

ToolEffect::ApprovalUpdate::{Approved,Denied}
  -> RuntimeOverlay.pending_approvals remove
  -> record ApprovalDecision event
```

**Step 3: Keep durable snapshot clean**

After this task, `SessionSnapshot.pending_approvals` and `SessionSnapshot.open_exec_sessions` should not be mutated in the live turn path.

Use this guard:

```bash
rg -n "snapshot\\.pending_approvals|snapshot\\.open_exec_sessions" src/runtime
```

Expected: no live turn mutations remain. Legacy tests or state definitions may still contain the field names.

**Step 4: Run focused tests**

Run:

```bash
cargo test --test thread_runtime
cargo test --test app_server_boundary
```

Expected: FAIL until projection reads overlay. PASS after Task 4.

## Task 4: Project ThreadView From Durable State Plus Overlay

**Files:**

- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Change live view consumption**

When `ThreadManager` reads a loaded runtime, use:

```text
live_view.snapshot
live_view.overlay
live_view.events
live_view.status
```

When `ThreadManager` reads cold storage, use:

```text
stored.snapshot
RuntimeOverlay::default()
stored.events
```

**Step 2: Update status derivation**

Thread status should use overlay for live-only state:

```text
WaitingApproval:
  only when RuntimeOverlay.pending_approvals has pending entries

Idle/Failed/Running:
  from active_turn, latest turn events, and runtime status
```

Do not derive `WaitingApproval` from cold rollout events alone.

**Step 3: Add tests for loaded runtime behavior**

Add an app-server test that starts a turn with a tool requiring approval and confirms:

```text
loaded thread_read returns WaitingApproval
pending approval is visible only while runtime is loaded
```

**Step 4: Add tests for cold storage behavior**

Add a test that creates a rollout with historical `ApprovalRequested`, does not load runtime, and confirms:

```text
thread_read returns Idle or Failed based on turn lifecycle
thread_read does not return WaitingApproval
```

Also add a test for persistent exec sessions:

```text
loaded events/replay include_snapshot reports open_exec_session_count from RuntimeOverlay
cold events/replay include_snapshot for the same rollout reports open_exec_session_count = 0
```

**Step 5: Run focused tests**

Run:

```bash
cargo test --test app_server_boundary approval
cargo test --test app_server_boundary cold_thread_read_does_not_restore_historical_approval_as_waiting
cargo test --test app_server_boundary events_replay_snapshot_counts_live_open_exec_sessions_from_overlay_only
```

Expected: PASS.

## Task 5: Add Architecture Guards

**Files:**

- Modify: `tests/architecture_guards.rs`

**Step 1: Add guard against rollout restoring live-only state**

Add a text/source guard that protects `snapshot_from_rollout_items` from populating live-only fields:

```rust
#[test]
fn rollout_reconstruction_does_not_restore_runtime_overlay_fields() {
    let source = std::fs::read_to_string("src/state/rollout.rs").unwrap();
    assert!(source.contains("open_exec_sessions: vec![]"));
    assert!(source.contains("pending_approvals: vec![]"));
}
```

**Step 2: Add guard against runtime prompt using overlay**

Add a source guard proving `ContextManager::for_prompt()` remains the prompt source and does not include overlay fields.

Suggested check:

```rust
#[test]
fn context_manager_remains_prompt_history_owner() {
    let source = std::fs::read_to_string("src/runtime/thread_session/turn.rs").unwrap();
    assert!(source.contains("context_manager.for_prompt()"));
    assert!(!source.contains("pending_approvals"));
    assert!(!source.contains("open_exec_sessions"));
}
```

If the final implementation keeps overlay update helpers in `turn.rs`, narrow this guard to the sampling block instead of the whole file.

**Step 3: Run architecture tests**

Run:

```bash
cargo test --test architecture_guards
```

Expected: PASS.

## Task 6: Clean Protocol Compatibility Documentation

**Files:**

- Modify: `docs/protocol/app-server-boundary-v2.md`
- Modify: `docs/architecture/2026-05-20-exagent-rollout-persistence-architecture.md`

**Step 1: Document ThreadView projection**

Add this invariant to protocol docs:

```text
ThreadView is a projection. It is not the source of truth.

For loaded runtimes:
  ThreadView is derived from ThreadSession durable state plus RuntimeOverlay.

For cold storage reads:
  ThreadView is derived from rollout durable state plus an empty RuntimeOverlay.
```

**Step 2: Mark snapshot/events fields as compatibility-only**

Document:

```text
snapshot_path and events_path are v2 compatibility fields.
New sessions use rollout.jsonl as durable storage.
Future v3 should expose rollout_path or storage metadata instead.
```

**Step 3: Run docs-adjacent checks**

Run:

```bash
rg -n "snapshot_path|events_path|rollout.jsonl|RuntimeOverlay|ThreadView" docs/protocol docs/architecture
```

Expected: docs clearly state the compatibility boundary.

## Task 7: Full Verification

**Files:**

- No source changes unless failures identify a missing update.

**Step 1: Format**

Run:

```bash
cargo fmt -- --check
```

Expected: PASS.

**Step 2: Whitespace check**

Run:

```bash
git diff --check
```

Expected: PASS.

**Step 3: Full test suite**

Run:

```bash
cargo test
```

Expected: PASS.

**Step 4: Source invariant scan**

Run:

```bash
rg -n "snapshot\\.pending_approvals|snapshot\\.open_exec_sessions" src/runtime
rg -n "open_exec_sessions: vec!\\[\\]|pending_approvals: vec!\\[\\]" src/state/rollout.rs
```

Expected:

```text
No live turn mutation of snapshot pending/open exec fields remains.
rollout reconstruction explicitly initializes live-only fields empty while SessionSnapshot still has compatibility fields.
```

## Acceptance Criteria

The implementation is accepted only when all criteria below are true:

```text
Persistence:
  New sessions still write rollout.jsonl as the single durable source of truth.
  No snapshot.json/events.jsonl writes are reintroduced for new sessions.
  No second live-state persistence file is introduced.

Cold replay:
  rollout reconstruction restores conversation, TurnContext, SessionMeta, and compaction data.
  rollout reconstruction never restores pending_approvals.
  rollout reconstruction never restores open_exec_sessions.
  cold thread_read does not report WaitingApproval from historical ApprovalRequested events alone.
  cold thread_read does not report running exec sessions from historical tool results alone.

Live runtime:
  loaded runtime can still report WaitingApproval while a real approval waiter exists.
  loaded runtime can still report open exec sessions while real process handles exist.
  interrupt and approval decision paths still clear live pending approvals.
  exec lifecycle updates still appear in loaded runtime views.

Prompt/history:
  ContextManager remains the only prompt-visible history owner.
  sampling prompt still comes from ContextManager::for_prompt().
  RuntimeOverlay is never included in model prompt history.
  compaction inputs will be able to come from ContextManager only.

Projection:
  ThreadView is documented and implemented as a projection, not a source of truth.
  loaded ThreadView = durable state + RuntimeOverlay + live events.
  cold ThreadView = durable rollout state + empty RuntimeOverlay + persisted events.

Protocol:
  app-server boundary v2 docs mark snapshot_path/events_path as compatibility-only.
  rollout durable storage is documented as the canonical storage path for new sessions.

Verification:
  cargo fmt -- --check passes.
  git diff --check passes.
  cargo test passes.
  architecture guards cover cold replay and prompt ownership invariants.
```

## Recommended Commit Sequence

Use small commits:

```text
test: pin cold replay live-state invariants
refactor: introduce thread runtime overlay
refactor: route live approval and exec state through overlay
fix: project thread views from runtime overlay
test: guard rollout and prompt ownership invariants
docs: document runtime overlay projection model
```

## Implementation Notes

This change is intentionally lower priority than compaction unless UI/runtime behavior starts depending on cold read pending state. Current behavior is mostly correct because rollout replay already initializes live-only fields empty. The value of this work is to make the type boundary honest before compaction, protocol v3, or richer GUI/TUI controls build on top of it.
