# ExAgent Thread Runtime Actor V2 Implementation Plan

**Goal:** Replace request-scoped turn execution with a long-lived per-thread runtime actor model inspired by Codex.

**Architecture:** `ThreadManager` becomes a registry/factory for loaded `ThreadRuntime` handles. Each `ThreadRuntime` owns a mailbox sender and live event broadcaster, `ThreadRuntimeLoop` serializes mailbox submissions, and `ThreadSession` owns per-thread execution state, calls `Agent`, writes `events.jsonl`, and broadcasts events. Public `turn/start` becomes asynchronous: it returns an in-progress turn, and final output arrives through events. Protocol DTOs are normalized at the `ThreadManager` boundary before entering `ThreadRuntime`.

**Tech Stack:** Rust, Tokio `mpsc`/`broadcast`/`watch`/`oneshot`, Axum, serde, existing ExAgent `Agent`, transcript persistence, and app-server protocol DTOs.

**Status:** Implemented in the current working tree.

Implemented decisions:

- `ThreadRuntime` owns the live actor mailbox, event broadcaster, status watch channel, and active-turn interrupt handle.
- `ThreadSession` owns the per-thread live state, including the in-memory snapshot, live Agent, event cursor, and handlers for turn lifecycle and event emission.
- `ThreadManager` owns the process-level runtime registry and no longer calls `Agent::resume*` for public `turn/start`.
- `turn/start` returns `TurnStatus::InProgress`; final output is read from live/replayed events.
- `initialize.supported_ops` maps to serializable `BoundaryOp` variants; streaming surfaces are advertised separately through `supported_streams`.
- HTTP live events use `POST /events/subscribe` and return an SSE stream of serialized `RuntimeEvent` values.
- `events/subscribe(after_event_id)` replays persisted events after the cursor before switching to the live broadcast channel.
- Cursor recovery is positional: `after_event_id` is found in `events.jsonl`, and string ordering is not used.
- Events produced during one `ThreadOp::UserInput` are normalized to the outer runtime turn id.
- Concurrent turns are rejected in V2 rather than queued.
- Child session lineage and multi-runtime child orchestration stay outside V2.

## Context

Current V1 state:

- `src/app_server/thread_manager.rs` directly calls `Agent::resume_with_turn_cwd` in `turn_start_direct`.
- `turn/start` returns final output.
- `events/replay` exists, but there is no live event subscription.
- CLI uses `thread_start -> turn_start` and prints the final output from the response.

Target V2 state:

- `ThreadManager` owns `loaded_threads: HashMap<SessionId, Arc<ThreadRuntime>>`.
- `ThreadRuntime` is a live thread handle with guarded user-input submission, `subscribe_events`, `live_view`, `status`, and `shutdown`.
- `ThreadRuntimeLoop` serializes `ThreadSubmission` values and dispatches `ThreadOp`.
- `ThreadSession` owns per-thread live execution state, event id assignment, event append/broadcast, and snapshot checkpointing. It handles `ThreadOp::UserInput` without reconstructing Agent state from disk per turn.
- `turn/start` submits `ThreadOp::UserInput` and returns `TurnStatus::InProgress`.
- CLI waits on live events until turn completion, then prints final assistant text.

Reference design:

- `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`
- `external-references/Codex/codex-rs/core/src/thread_manager.rs`
- `external-references/Codex/codex-rs/core/src/codex_thread.rs`
- `external-references/Codex/codex-rs/core/src/session/mod.rs`
- `external-references/Codex/codex-rs/core/src/session/handlers.rs`

## Task 1: Add Thread Runtime Types

**Files:**
- Create: `src/runtime/thread_runtime.rs`
- Modify: `src/app_server/mod.rs`
- Test: `tests/thread_runtime.rs`

**Step 1: Write the failing test**

Create `tests/thread_runtime.rs` with a construction and submit smoke test:

```rust
use exagent::runtime::thread_runtime::{
    ThreadOp, ThreadRuntime, ThreadRuntimeOptions, ThreadRuntimeStatus,
};
use exagent::config::AgentConfig;
use exagent::types::{SessionId, TurnId};

#[tokio::test]
async fn thread_runtime_starts_idle_and_accepts_shutdown_op() {
    let thread_id = SessionId::new("session_runtime_test");
    let config = AgentConfig::default();
    let runtime = ThreadRuntime::spawn(ThreadRuntimeOptions::new(thread_id.clone(), config))
        .await
        .expect("spawn runtime");

    assert_eq!(runtime.thread_id(), &thread_id);
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Idle);

    runtime
        .submit(ThreadOp::Shutdown)
        .await
        .expect("submit shutdown");
    runtime.wait_until_terminated().await;
    assert_eq!(runtime.status(), ThreadRuntimeStatus::Stopped);
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test thread_runtime thread_runtime_starts_idle_and_accepts_shutdown_op -- --nocapture
```

Expected: compile failure because `thread_runtime` module and types do not exist.

**Step 3: Implement minimal runtime skeleton**

Create `src/runtime/thread_runtime.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, mpsc, watch};

use crate::config::AgentConfig;
use crate::events::RuntimeEvent;
use crate::types::{SessionId, TurnId};

const THREAD_OP_CHANNEL_CAPACITY: usize = 64;
const THREAD_EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRuntimeStatus {
    Idle,
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
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

#[derive(Debug)]
pub struct ThreadSubmission {
    pub op: ThreadOp,
}

pub struct ThreadRuntimeOptions {
    pub thread_id: SessionId,
    pub config: AgentConfig,
}

impl ThreadRuntimeOptions {
    pub fn new(thread_id: SessionId, config: AgentConfig) -> Self {
        Self { thread_id, config }
    }
}

pub struct ThreadRuntime {
    thread_id: SessionId,
    op_tx: mpsc::Sender<ThreadSubmission>,
    event_tx: broadcast::Sender<RuntimeEvent>,
    status_rx: watch::Receiver<ThreadRuntimeStatus>,
    terminated: Arc<tokio::sync::Notify>,
}

impl ThreadRuntime {
    pub async fn spawn(options: ThreadRuntimeOptions) -> Result<Arc<Self>> {
        let (op_tx, op_rx) = mpsc::channel(THREAD_OP_CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(THREAD_EVENT_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = watch::channel(ThreadRuntimeStatus::Idle);
        let terminated = Arc::new(tokio::sync::Notify::new());

        let runtime = Arc::new(Self {
            thread_id: options.thread_id.clone(),
            op_tx,
            event_tx: event_tx.clone(),
            status_rx,
            terminated: terminated.clone(),
        });

        tokio::spawn(async move {
            ThreadRuntimeLoop {
                thread_id: options.thread_id,
                _config: options.config,
                op_rx,
                _event_tx: event_tx,
                status_tx,
                terminated,
            }
            .run()
            .await;
        });

        Ok(runtime)
    }

    pub fn thread_id(&self) -> &SessionId {
        &self.thread_id
    }

    pub fn status(&self) -> ThreadRuntimeStatus {
        *self.status_rx.borrow()
    }

    pub async fn submit(&self, op: ThreadOp) -> Result<()> {
        self.op_tx
            .send(ThreadSubmission { op })
            .await
            .map_err(|_| anyhow!("thread runtime is stopped"))
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.event_tx.subscribe()
    }

    pub async fn wait_until_terminated(&self) {
        self.terminated.notified().await;
    }
}

struct ThreadRuntimeLoop {
    thread_id: SessionId,
    _config: AgentConfig,
    op_rx: mpsc::Receiver<ThreadSubmission>,
    _event_tx: broadcast::Sender<RuntimeEvent>,
    status_tx: watch::Sender<ThreadRuntimeStatus>,
    terminated: Arc<tokio::sync::Notify>,
}

impl ThreadRuntimeLoop {
    async fn run(mut self) {
        while let Some(submission) = self.op_rx.recv().await {
            match submission.op {
                ThreadOp::Shutdown => break,
                ThreadOp::UserInput { .. } | ThreadOp::Interrupt { .. } => {
                    let _ = self.status_tx.send(ThreadRuntimeStatus::Running);
                    let _ = self.status_tx.send(ThreadRuntimeStatus::Idle);
                }
            }
        }
        let _ = self.status_tx.send(ThreadRuntimeStatus::Stopped);
        self.terminated.notify_waiters();
    }
}
```

Modify `src/app_server/mod.rs`:

```rust
pub mod thread_runtime;
```

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test thread_runtime thread_runtime_starts_idle_and_accepts_shutdown_op -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/thread_runtime.rs src/app_server/mod.rs tests/thread_runtime.rs
git commit -m "feat: add thread runtime actor skeleton"
```

## Task 2: Make ThreadManager Own Loaded ThreadRuntime Handles

**Files:**
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write the failing test**

Add a test to `tests/app_server_boundary.rs`:

```rust
#[tokio::test]
async fn thread_start_registers_loaded_runtime_and_thread_resume_reuses_it() {
    let (_dir, service) = service_with_workspace();
    let started = service
        .thread_start(ThreadStartParams {
            workspace_root: Some(_dir.path().display().to_string()),
            cwd: None,
        })
        .expect("thread start");

    assert!(service.thread_manager_for_test().is_thread_loaded(&started.thread_id));

    let resumed = service
        .thread_resume(ThreadResumeParams {
            thread_id: started.thread_id.clone(),
            workspace_root: Some(_dir.path().display().to_string()),
            cwd: None,
        })
        .expect("thread resume");

    assert_eq!(resumed.thread.thread_id, started.thread_id);
    assert!(service.thread_manager_for_test().is_thread_loaded(&started.thread_id));
}
```

If `thread_manager_for_test` is too invasive, expose a `loaded_thread_count_for_test`
method behind `#[cfg(test)]` or assert behavior through `turn/start` in Task 3.

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test app_server_boundary thread_start_registers_loaded_runtime_and_thread_resume_reuses_it -- --nocapture
```

Expected: compile failure because `ThreadManager` has no loaded runtime registry.

**Step 3: Implement registry/factory**

Modify `ThreadManager` to include:

```rust
loaded_threads: Arc<Mutex<HashMap<String, Arc<ThreadRuntime>>>>,
```

Add:

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

Add methods:

```rust
async fn start_thread_with_options(&self, options: StartThreadOptions) -> Result<NewThread>;
async fn ensure_runtime_loaded(&self, thread_id: &SessionId, config: AgentConfig) -> Result<Arc<ThreadRuntime>>;
fn runtime_for(&self, thread_id: &SessionId) -> Option<Arc<ThreadRuntime>>;
```

Update `thread_start`:

```text
merge thread_start overrides
create snapshot
spawn ThreadRuntime
insert into loaded_threads
return ThreadStartResponse
```

Update `thread_resume`:

```text
read snapshot
ensure runtime loaded
return ThreadResumeResponse
```

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test app_server_boundary thread_start_registers_loaded_runtime_and_thread_resume_reuses_it -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/app_server/thread_manager.rs src/runtime/thread_runtime.rs tests/app_server_boundary.rs
git commit -m "refactor: register loaded thread runtimes"
```

## Task 3: Move Turn Execution Into ThreadRuntimeLoop

**Files:**
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/app_server/thread_manager.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write the failing test**

Add:

```rust
#[tokio::test]
async fn turn_start_submits_op_and_returns_in_progress_before_completion() {
    let (_dir, service) = service_with_workspace_and_slow_llm();
    let started = service
        .thread_start(ThreadStartParams {
            workspace_root: Some(_dir.path().display().to_string()),
            cwd: None,
        })
        .expect("thread start");

    let response = service
        .turn_start(TurnStartParams {
            thread_id: started.thread_id.clone(),
            prompt: "do slow work".into(),
            workspace_root: Some(_dir.path().display().to_string()),
            turn_context: None,
        })
        .await
        .expect("turn start");

    assert_eq!(response.thread_id, started.thread_id);
    assert_eq!(response.turn.status, TurnStatus::InProgress);
    assert!(response.output.text.is_none());
}
```

Adjust the expected field names after protocol changes in Task 4 if needed.

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test app_server_boundary turn_start_submits_op_and_returns_in_progress_before_completion -- --nocapture
```

Expected: FAIL because current `turn_start` waits for final output.

**Step 3: Implement runtime loop execution**

Move this logic out of `ThreadManager::turn_start_direct` and into
`ThreadRuntimeLoop`:

```text
append TurnStarted
call Agent::resume_with_turn_cwd
append AssistantTurn / ToolResult events already produced by Agent
append TurnCompleted
append RuntimeError on error
handle Interrupt
update status
notify completion_tx
```

`ThreadManager::turn_start` becomes:

```text
merge turn_start override
normalize protocol turn_context into ThreadTurnContext
ensure runtime loaded
generate turn_id
runtime.submit(ThreadOp::UserInput { turn_id, prompt, turn_context })
return in-progress turn response
```

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test app_server_boundary turn_start_submits_op_and_returns_in_progress_before_completion -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/thread_runtime.rs src/app_server/thread_manager.rs tests/app_server_boundary.rs
git commit -m "refactor: run turns through thread runtime mailbox"
```

## Task 4: Change Protocol DTOs To Thread/Turn Views

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/api.rs`
- Modify: `tests/api_server.rs`
- Modify: `tests/app_server_boundary.rs`

**Step 1: Write failing protocol tests**

Update or add tests asserting:

```rust
assert_eq!(turn_response.turn.status, TurnStatus::InProgress);
assert!(turn_response.output.is_none());
```

Add DTOs:

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
    ApprovalRequested,
    ApprovalDecision,
    RuntimeError { message: String },
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test app_server_boundary turn_start_submits_op_and_returns_in_progress_before_completion -- --nocapture
cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt -- --nocapture
```

Expected: FAIL due old response shape.

**Step 3: Implement DTO changes**

Change:

```rust
pub struct TurnStartResponse {
    pub thread_id: SessionId,
    pub turn: TurnView,
}
```

Change `ThreadReadResponse` to include `ThreadView` or replace it with:

```rust
pub struct ThreadReadResponse {
    pub thread: ThreadView,
}
```

Update route tests to expect `turn.status == "in_progress"` and no final output in
the response.

**Step 4: Run tests**

Run:

```bash
cargo test --test app_server_boundary -- --nocapture
cargo test --test api_server -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/app_server/protocol.rs src/app_server/thread_manager.rs src/api.rs tests/api_server.rs tests/app_server_boundary.rs
git commit -m "feat: return thread and turn views from boundary"
```

## Task 5: Add Live Events Subscribe API

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/service.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/api.rs`
- Test: `tests/app_server_boundary.rs`
- Test: `tests/api_server.rs`

**Step 1: Write failing service test**

Add:

```rust
#[tokio::test]
async fn events_subscribe_receives_turn_lifecycle_events() {
    let (_dir, service) = service_with_workspace();
    let started = service.thread_start(...).expect("thread start");
    let mut events = service
        .events_subscribe(EventsSubscribeParams {
            thread_id: started.thread_id.clone(),
            workspace_root: Some(_dir.path().display().to_string()),
        })
        .expect("subscribe");

    service.turn_start(...).await.expect("turn start");

    let event = events.recv().await.expect("first event");
    assert_eq!(event.kind, RuntimeEventKind::TurnStarted);
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test app_server_boundary events_subscribe_receives_turn_lifecycle_events -- --nocapture
```

Expected: compile failure because `events_subscribe` does not exist.

**Step 3: Implement service-level subscribe**

Add protocol params:

```rust
pub struct EventsSubscribeParams {
    pub thread_id: SessionId,
    pub workspace_root: Option<String>,
    pub after_event_id: Option<String>,
}
```

Add to `AppServerBoundary`:

```rust
fn events_subscribe(&self, params: EventsSubscribeParams) -> Result<broadcast::Receiver<RuntimeEvent>>;
```

ThreadManager implementation:

```text
ensure runtime loaded
return runtime.subscribe_events()
```

**Step 4: Add HTTP streaming route**

Add route:

```text
POST /events/subscribe
```

Use axum SSE. Each SSE data frame contains one serialized `RuntimeEvent`.

Whichever transport is chosen, each event must carry a stable event id/cursor.
If `after_event_id` is provided, the route should replay persisted events after
that cursor before switching to live broadcast. This closes the replay-then-
subscribe race for reconnecting clients.

**Step 5: Run tests**

Run:

```bash
cargo test --test app_server_boundary events_subscribe_receives_turn_lifecycle_events -- --nocapture
cargo test --test api_server events_subscribe_route_streams_runtime_events -- --nocapture
```

Expected: PASS.

**Step 6: Commit**

```bash
git add src/app_server/protocol.rs src/app_server/service.rs src/app_server/thread_manager.rs src/api.rs tests/app_server_boundary.rs tests/api_server.rs
git commit -m "feat: add live runtime event subscription"
```

## Task 6: Update CLI To Wait On Events

**Files:**
- Modify: `src/cli_adapter.rs`
- Test: `tests/cli_adapter.rs`

**Step 1: Write failing CLI adapter test**

Add:

```rust
#[tokio::test]
async fn cli_run_waits_for_turn_completed_event_and_prints_final_text() {
    let boundary = FakeStreamingBoundary::new(vec![
        RuntimeEventKind::TurnStarted,
        RuntimeEventKind::AssistantTurn { turn: assistant_text("done") },
        RuntimeEventKind::TurnCompleted,
    ]);

    let output = execute_cli_command(
        &boundary,
        CliCommand::Run { prompt: "work".into() },
    )
    .await
    .expect("cli command");

    assert_eq!(output.stdout, "done\n");
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test cli_adapter cli_run_waits_for_turn_completed_event_and_prints_final_text -- --nocapture
```

Expected: FAIL because CLI still reads final output from `TurnStartResponse`.

**Step 3: Implement event wait**

Update CLI run/resume:

```text
thread_start/thread_resume
events_subscribe
turn_start
wait until matching turn_completed / turn_interrupted / runtime_error
print latest AssistantTurn text
```

**Step 4: Run CLI tests**

Run:

```bash
cargo test --test cli_adapter -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/cli_adapter.rs tests/cli_adapter.rs
git commit -m "refactor: make cli consume turn events"
```

## Task 7: Update Replay And Thread Read To Build Thread/Turn/Item Views

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/thread_manager.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write failing test**

Add:

```rust
#[tokio::test]
async fn thread_read_reconstructs_turns_and_items_from_events() {
    let (_dir, service) = service_with_workspace();
    let started = service.thread_start(...).expect("thread start");
    service.turn_start(...).await.expect("turn start");
    wait_for_completed_event(&service, &started.thread_id).await;

    let response = service
        .thread_read(ThreadReadParams {
            thread_id: started.thread_id.clone(),
            workspace_root: Some(_dir.path().display().to_string()),
        })
        .expect("thread read");

    assert_eq!(response.thread.turns.len(), 1);
    assert_eq!(response.thread.turns[0].status, TurnStatus::Completed);
    assert!(response.thread.turns[0]
        .items
        .iter()
        .any(|item| matches!(item, ThreadItem::AssistantMessage { .. })));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test app_server_boundary thread_read_reconstructs_turns_and_items_from_events -- --nocapture
```

Expected: FAIL because thread/read does not build turns/items.

**Step 3: Implement event-derived view builder**

Add helper:

```rust
fn build_thread_view_from_events(
    snapshot: SessionSnapshot,
    events: Vec<RuntimeEvent>,
    active_turn: Option<TurnState>,
    paths: SessionPaths,
) -> ThreadView
```

Keep first version lossy and simple:

```text
TurnStarted creates TurnView
AssistantTurn appends AssistantMessage
ToolResult appends ToolResult
ExecOutput appends ExecOutput
RuntimeError appends RuntimeError and marks failed
TurnCompleted marks completed
TurnInterrupted marks interrupted
```

**Step 4: Run tests**

Run:

```bash
cargo test --test app_server_boundary thread_read_reconstructs_turns_and_items_from_events -- --nocapture
cargo test --test app_server_boundary -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/app_server/protocol.rs src/app_server/thread_manager.rs tests/app_server_boundary.rs
git commit -m "feat: build thread turn item views"
```

## Task 8: Remove V1 Compatibility Assumptions And Update Docs

**Files:**
- Modify: `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`
- Modify: `docs/plans/2026-05-16-exagent-thread-runtime-actor-v2.md`
- Test: none

**Step 1: Update docs**

Document:

```text
turn/start returns in_progress
final output arrives via events
ThreadManager is registry/factory
ThreadRuntime is actor handle
ThreadRuntimeLoop owns Agent execution
CLI waits on event stream
```

**Step 2: Run doc whitespace check**

Run:

```bash
git diff --check
```

Expected: no output and exit 0.

**Step 3: Run full verification**

Run:

```bash
cargo fmt -- --check
cargo test
```

Expected: all tests pass.

**Step 4: Commit**

```bash
git add docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md docs/plans/2026-05-16-exagent-thread-runtime-actor-v2.md
git commit -m "docs: document thread runtime actor v2"
```

## Verification Before Merge

Run:

```bash
cargo fmt -- --check
git diff --check
cargo test
```

Expected:

```text
all tests pass
no formatting changes required
no whitespace errors
```

## Commit Strategy

Use small commits:

```text
feat: add thread runtime actor skeleton
refactor: register loaded thread runtimes
refactor: run turns through thread runtime mailbox
feat: return thread and turn views from boundary
feat: add live runtime event subscription
refactor: make cli consume turn events
feat: build thread turn item views
docs: document thread runtime actor v2
```

## Landed Defaults And Remaining Follow-Ups

Landed defaults:

- Use SSE for HTTP live events.
- Return `InProgress` once a turn is accepted by the runtime.
- Reject concurrent turns in V2 unless an explicit queue is added later.
- Keep replay and subscribe separate, but let `events/subscribe(after_event_id)` replay the gap before switching to live events.
- Treat the live `ThreadSession` state as authoritative while a runtime is loaded. Persisted `snapshot.json` and `events.jsonl` are the recovery and replay surface; live `broadcast` is best-effort and clients reconnect by replaying from the last seen event id before subscribing with that cursor.
- Normalize `TurnContextOverrides` into `ThreadTurnContext` at `ThreadManager`. `ThreadOp::UserInput` holds the internal type so `thread_runtime.rs` does not depend on protocol DTOs.
- Keep active-turn interrupt state inside `ThreadRuntime`, not `ThreadManager`.
- Keep turn lifecycle and event append/broadcast logic inside `ThreadSession`, not `ThreadRuntimeLoop`.
- Leave child session lineage and multi-runtime orchestration outside V2. Existing fork/spawn behavior stays on the compatibility path until a later design decides how parent and child timelines route through live runtimes.

Remaining follow-ups:

- Add explicit SDK/client documentation for the SSE event envelope.
- Add runtime unload/shutdown APIs when thread list/archive management lands.
- Add token-level streaming events only after the LLM client supports streaming deltas.
