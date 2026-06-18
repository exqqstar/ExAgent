# Thread Runtime Folder Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `src/runtime/thread_runtime.rs` into a focused `thread_runtime/` module tree without changing runtime behavior, then make turn reservation own its full busy/interruption invariant.

**Architecture:** Follow the Codex-inspired boundary where external control requests become internal runtime operations consumed by a single actor loop. Keep actor scheduling logic with the actor because it uses the loop's private state. Keep the existing behavior tests together in `tests.rs` because they exercise the full `ThreadRuntime` through public crate APIs rather than module-private units.

**Tech Stack:** Rust, Tokio, existing ExAgent runtime actor/session/tool architecture, Cargo tests.

---

## File Structure

- Create: `src/runtime/thread_runtime/mod.rs`
  - Module declarations and public re-exports for the existing external API.
- Create: `src/runtime/thread_runtime/op.rs`
  - Runtime command/result/status/error types: `ThreadOp`, `ThreadOpResult`, `ThreadRuntimeStatus`, `ThreadRuntimeError`, `ThreadTurnContext`.
- Create: `src/runtime/thread_runtime/reservation.rs`
  - Step A: moved reservation state/functions.
  - Step B: `TurnReservations` newtype that owns allocation, active-turn lookup, interrupt signaling, and guard release.
- Create: `src/runtime/thread_runtime/actor.rs`
  - Runtime loop actor, submission envelope, post-op scheduling methods, loop spawning helper, completion helpers.
- Create: `src/runtime/thread_runtime/facade.rs`
  - Public `ThreadRuntime` facade, options, factory aliases, workspace operation gate, external control methods.
- Create: `src/runtime/thread_runtime/tests.rs`
  - Existing `thread_runtime.rs` behavior tests kept as one cohesive integration-style suite.
- Delete: `src/runtime/thread_runtime.rs`
  - Replaced by the module directory.

Do not stage or commit unrelated local-only files or the existing `src/runtime/thread_session/*` working changes from the prior context-refresh refactor.

---

### Task 1: Pure Module Move

**Files:**
- Create: `src/runtime/thread_runtime/mod.rs`
- Create: `src/runtime/thread_runtime/op.rs`
- Create: `src/runtime/thread_runtime/reservation.rs`
- Create: `src/runtime/thread_runtime/actor.rs`
- Create: `src/runtime/thread_runtime/facade.rs`
- Create: `src/runtime/thread_runtime/tests.rs`
- Delete: `src/runtime/thread_runtime.rs`

- [ ] **Step 1: Inspect current public consumers**

Run:

```bash
rg "crate::runtime::thread_runtime::|runtime::thread_runtime::" src tests -n
```

Expected: consumers reference these public symbols and must remain source-compatible:

```text
ThreadRuntime
ThreadRuntimeStatus
ThreadRuntimeError
ThreadOpResult
ThreadTurnContext
AgentFactory
WorkspaceRuntimeOpGate
ThreadRuntimeOptions
```

- [ ] **Step 2: Create module files by moving the existing code**

Move code from `src/runtime/thread_runtime.rs` into these files:

```text
op.rs
  ThreadRuntimeError
  ThreadRuntimeStatus
  ThreadTurnContext
  ThreadOp
  ThreadOpResult

reservation.rs
  reserve_next_turn_from_state
  reserve_turn_record_from_state
  TurnReservationState
  ActiveRuntimeTurnRecord
  ActiveRuntimeTurnGuard
  Drop for ActiveRuntimeTurnGuard

actor.rs
  PENDING_MAIL_TURN_PROMPT
  ThreadSubmission
  spawn_runtime_loop
  ThreadRuntimeLoop
  impl ThreadRuntimeLoop
  complete
  terminal_notification_from_user_input_result

facade.rs
  AgentFactory
  WorkspaceRuntimeOpPermit
  WorkspaceRuntimeOpGate
  ThreadRuntimeOptions
  ThreadRuntime
  impl ThreadRuntime

tests.rs
  the entire existing #[cfg(test)] mod tests body, converted to a top-level module file
```

Create `src/runtime/thread_runtime/mod.rs` with:

```rust
mod actor;
mod facade;
mod op;
mod reservation;

#[cfg(test)]
mod tests;

pub use facade::{AgentFactory, ThreadRuntime, ThreadRuntimeOptions, WorkspaceRuntimeOpGate};
pub use op::{ThreadOpResult, ThreadRuntimeError, ThreadRuntimeStatus, ThreadTurnContext};
```

- [ ] **Step 3: Apply the minimum cross-module visibility**

Use `pub(super)` only where a sibling module needs access during the pure move:

```text
actor.rs:
  pub(super) struct ThreadSubmission
  pub(super) fields on ThreadSubmission
  pub(super) struct ThreadRuntimeLoop
  pub(super) fn spawn_runtime_loop

reservation.rs:
  pub(super) fn reserve_next_turn_from_state
  pub(super) fn reserve_turn_record_from_state
  pub(super) struct TurnReservationState
  pub(super) struct ActiveRuntimeTurnRecord
  pub(super) fields on ActiveRuntimeTurnRecord for Step A only
  pub(super) struct ActiveRuntimeTurnGuard

facade.rs:
  pub(crate) type WorkspaceRuntimeOpPermit
```

Do not use `pub(crate)` unless an existing external consumer already needs it.

- [ ] **Step 4: Compile the moved module**

Run:

```bash
cargo test --test thread_runtime
```

Expected: PASS. If imports fail, fix only module paths and visibility needed for the pure move.

- [ ] **Step 5: Run architecture/tool regression checks**

Run:

```bash
cargo test --test architecture_guards
cargo test --test tool_runtime_architecture
```

Expected: PASS.

- [ ] **Step 6: Commit the pure move**

Stage only this refactor's files:

```bash
git add src/runtime/thread_runtime.rs src/runtime/thread_runtime docs/superpowers/plans/2026-06-18-thread-runtime-folder-refactor.md
git commit -m "refactor: split thread runtime module"
```

Expected: commit contains the plan and pure module move. It must not include `AGENTS.md`, `ExAgent-notes/`, or unrelated `src/runtime/thread_session/*` changes.

---

### Task 2: Turn Reservation Owner API

**Files:**
- Modify: `src/runtime/thread_runtime/reservation.rs`
- Modify: `src/runtime/thread_runtime/facade.rs`
- Modify: `src/runtime/thread_runtime/actor.rs`
- Test: `src/runtime/thread_runtime/tests.rs`

- [ ] **Step 1: Introduce `TurnReservations` newtype**

In `reservation.rs`, wrap the shared state:

```rust
#[derive(Clone)]
pub(super) struct TurnReservations {
    state: Arc<Mutex<TurnReservationState>>,
}

impl TurnReservations {
    pub(super) fn new(next_turn_index: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(TurnReservationState {
                next_turn_index,
                active_turn: None,
            })),
        }
    }
}
```

- [ ] **Step 2: Move reserve functions onto `TurnReservations`**

Replace the free functions with methods:

```rust
impl TurnReservations {
    pub(super) fn reserve_next(
        &self,
        thread_id: &ThreadId,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<(TurnId, ActiveRuntimeTurnGuard)> {
        let mut state = self.state.lock().expect("turn reservation mutex poisoned");
        if state.active_turn.is_some() {
            return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
        }
        let turn_id = TurnId::new(format!("turn_{}", state.next_turn_index));
        state.next_turn_index = state.next_turn_index.saturating_add(1);
        state.active_turn = Some(ActiveRuntimeTurnRecord {
            public_turn_id: Some(turn_id.clone()),
            interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
            interrupted,
        });

        Ok((
            turn_id,
            ActiveRuntimeTurnGuard {
                reservations: self.clone(),
            },
        ))
    }

    pub(super) fn reserve_record(
        &self,
        thread_id: &ThreadId,
        public_turn_id: Option<TurnId>,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<ActiveRuntimeTurnGuard> {
        let mut state = self.state.lock().expect("turn reservation mutex poisoned");
        if state.active_turn.is_some() {
            return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
        }
        state.active_turn = Some(ActiveRuntimeTurnRecord {
            public_turn_id,
            interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
            interrupted,
        });

        Ok(ActiveRuntimeTurnGuard {
            reservations: self.clone(),
        })
    }
}
```

- [ ] **Step 3: Move active lookup and path-A interrupt signaling into `reservation.rs`**

Add:

```rust
impl TurnReservations {
    pub(super) fn active_turn_id(&self) -> Option<TurnId> {
        self.state.lock().ok().and_then(|state| {
            state
                .active_turn
                .as_ref()
                .and_then(|record| record.public_turn_id.clone())
        })
    }

    pub(super) async fn signal_interrupt(
        &self,
        thread_id: &ThreadId,
        requested_turn_id: Option<&TurnId>,
    ) -> Result<TurnId> {
        let record = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.active_turn.clone())
            .ok_or_else(|| anyhow!("thread has no active turn"))?;
        let public_turn_id =
            record
                .public_turn_id
                .clone()
                .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                    thread_id: thread_id.clone(),
                    reason: "active operation is not interruptible".to_string(),
                })?;
        if let Some(requested_turn_id) = requested_turn_id {
            if requested_turn_id != &public_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: thread_id.clone(),
                    reason: format!("active turn is {}", public_turn_id.as_str()),
                }
                .into());
            }
        }

        let did_send_interrupt = record
            .interrupt_tx
            .lock()
            .expect("active turn interrupt mutex poisoned")
            .take()
            .map(|interrupt_tx| interrupt_tx.send(()).is_ok())
            .unwrap_or(false);
        if !did_send_interrupt {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: thread_id.clone(),
                reason: "active turn is already interrupting or completed".to_string(),
            }
            .into());
        }
        record.interrupted.notified().await;
        Ok(public_turn_id)
    }
}
```

This method must clone the active record before awaiting. Do not hold the state mutex across `.await`.

- [ ] **Step 4: Update the guard to release through the newtype**

Change the guard to:

```rust
pub(super) struct ActiveRuntimeTurnGuard {
    reservations: TurnReservations,
}

impl Drop for ActiveRuntimeTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.reservations.state.lock() {
            state.active_turn = None;
        }
    }
}
```

After this, `TurnReservationState`, `ActiveRuntimeTurnRecord`, and their fields should be private to `reservation.rs`.

- [ ] **Step 5: Replace raw `Arc<Mutex<TurnReservationState>>` call sites**

In `facade.rs`:

```rust
turn_reservation: TurnReservations,
```

Initialize with:

```rust
turn_reservation: TurnReservations::new(next_turn_index),
```

Replace facade methods:

```rust
pub(crate) fn active_turn_id(&self) -> Option<TurnId> {
    self.turn_reservation.active_turn_id()
}

pub(crate) async fn interrupt_active_turn(
    &self,
    requested_turn_id: Option<&TurnId>,
) -> Result<TurnId> {
    self.turn_reservation
        .signal_interrupt(&self.thread_id, requested_turn_id)
        .await
}
```

Replace reserve helpers:

```rust
self.turn_reservation.reserve_next(&self.thread_id, interrupt_tx, interrupted)
```

and:

```rust
self.turn_reservation.reserve_record(
    &self.thread_id,
    None,
    interrupt_tx,
    Arc::new(Notify::new()),
)
```

In `actor.rs`, update `ThreadRuntimeLoop` to hold `TurnReservations` and call:

```rust
self.turn_reservation.active_turn_id()
self.turn_reservation.reserve_next(&self.thread_id, interrupt_tx, interrupted.clone())?
```

- [ ] **Step 6: Keep path-B interrupt in the facade**

Do not move:

```rust
pub(crate) async fn interrupt_waiting_approval_turn(...)
```

It remains a facade control operation that submits `ThreadOp::Interrupt` to the actor queue.

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test runtime::thread_runtime::tests::rejected_busy_submit_does_not_consume_turn_id
cargo test runtime::thread_runtime::tests::concurrent_submits_allocate_and_reserve_atomically
cargo test runtime::thread_runtime::tests::manual_compaction_reservation_rejects_concurrent_submit
cargo test runtime::thread_runtime::tests::interrupt_during_manual_compaction_is_rejected_without_sentinel
cargo test runtime::thread_runtime::tests::compact_now_rejects_while_user_turn_running
```

Expected: each command passes.

- [ ] **Step 8: Run full runtime architecture tests**

Run:

```bash
cargo test --test thread_runtime
cargo test --test architecture_guards
cargo test --test tool_runtime_architecture
```

Expected: PASS.

- [ ] **Step 9: Commit reservation owner API**

Stage only thread runtime files:

```bash
git add src/runtime/thread_runtime
git commit -m "refactor: encapsulate turn reservations"
```

Expected: second commit only changes `src/runtime/thread_runtime/*`.

---

## Verification Checklist

- [ ] `cargo test --test thread_runtime`
- [ ] `cargo test --test architecture_guards`
- [ ] `cargo test --test tool_runtime_architecture`
- [ ] `git status -sb` shows no unintended staged or committed local-only files.

## Plan Self-Review

- Spec coverage: covers pure folder split, actor/scheduler co-location, reservation newtype, path-A/path-B interrupt split, and tests kept in one file.
- Placeholder scan: no TBD/TODO placeholders.
- Type consistency: `TurnReservations`, `ActiveRuntimeTurnGuard`, `ThreadRuntimeLoop`, and public re-exports are named consistently across tasks.
