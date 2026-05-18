# ExAgent Session-Centered Agent Runtime Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move live turn execution to a session-centered architecture where `ThreadSession` is the only runtime history/snapshot/event writer, while model calls and tool execution become execution components.

**Architecture:** Keep the existing `ThreadManager -> ThreadRuntime -> ThreadRuntimeLoop -> ThreadSession` actor shape. Move turn-loop state mutation out of `Agent`, introduce a Codex-like `ToolCallRuntime`, and make every sampling request derive its prompt from committed `ThreadSession` history.

**Tech Stack:** Rust, Tokio, serde JSON snapshots/events, existing `LlmClient`, `ToolRegistry`, `ExecSessionManager`, `PolicyManager`, `ThreadEventRecorder`, app-server runtime tests.

**Implementation Status:** Implemented in the current working tree. The remaining future cleanup is to decide whether to rename the reduced `Agent` type to `ModelClient`/`ModelTurnRunner` once the public API impact is clear.

## Reference Design

Read this first:

- `docs/architecture/2026-05-18-exagent-session-centered-agent-architecture.md`
- `src/runtime/agent.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/thread_session/events.rs`
- `src/tools/registry.rs`
- `external-references/Codex/codex-rs/core/src/session/turn.rs`
- `external-references/Codex/codex-rs/core/src/tools/parallel.rs`
- `external-references/Codex/codex-rs/core/src/stream_events_utils.rs`

The desired final invariant:

```text
If live turn state changes, ThreadSession did it.
If the model receives context, it came from ThreadSession history.
If a tool result affects snapshot/history/events, ThreadSession applied it.
```

## Current High-Risk Code

The first refactor target is `src/runtime/agent.rs`.

Current risks:

- `legacy Agent live-turn runner` clones `snapshot.conversation` into local `messages`.
- `legacy Agent live-turn runner` appends to both `messages` and `snapshot.conversation`.
- `Agent` mutates `snapshot.open_exec_sessions`.
- `Agent` mutates `snapshot.pending_approvals`.
- `Agent` depends on `runtime::thread_session::LiveEventSink`.
- Approval and exec session state are parsed from `ToolResult.meta` inside `Agent`.

Do not begin with a broad rename. Preserve working behavior first, then narrow ownership.

## Implementation Milestones

This plan is intentionally staged so each milestone can pass tests before the next one starts.

```text
P0: Characterize current behavior with tests.
P1: Add typed turn-step/effect model.
P2: Move snapshot/history/event writes from Agent to ThreadSession.
P3: Remove persistent local working conversation from Agent.
P4: Introduce ToolCallRuntime.
P5: Re-home the turn loop under ThreadSession.
P6: Clean up legacy boundaries and architecture guards.
```

## Task 1: Add Failing Characterization Test For Prompt Source

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`

**Step 1: Add a test-only LLM that records prompt lengths**

Inside `#[cfg(test)] mod tests` in `src/runtime/thread_session/turn.rs`, add a local `RecordingLlm`:

```rust
struct RecordingLlm {
    turns: tokio::sync::Mutex<Vec<AssistantTurn>>,
    prompt_lens: Arc<std::sync::Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for RecordingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<AssistantTurn> {
        self.prompt_lens.lock().unwrap().push(messages.len());
        let mut turns = self.turns.lock().await;
        if turns.is_empty() {
            anyhow::bail!("RecordingLlm is out of turns");
        }
        Ok(turns.remove(0))
    }
}
```

**Step 2: Add a test for multi-sampling prompt growth**

Add a test that creates:

- first model output with one tool call
- second model output as final assistant message
- a registry with one simple echo tool

Expected prompt lengths:

```text
first sampling sees user message only
second sampling sees user + assistant tool call + tool result
```

The exact test name:

```rust
#[tokio::test]
async fn thread_session_next_sampling_uses_committed_history() {
    // arrange session, RecordingLlm, one echo tool
    // act: handle_user_input(...)
    // assert: prompt_lens == vec![1, 3]
}
```

**Step 3: Run the focused test**

Run:

```bash
cargo test runtime::thread_session::turn::tests::thread_session_next_sampling_uses_committed_history -- --nocapture
```

Expected before later refactor:

- This may pass with current implementation because `Agent` manually mirrors `messages`.
- Keep it anyway. It locks the external behavior before changing ownership.

**Step 4: Commit**

```bash
git add src/runtime/thread_session/turn.rs
git commit -m "test: characterize committed history between tool samplings"
```

## Task 2: Add Architecture Guard For Agent Snapshot Mutation

**Files:**

- Create: `tests/architecture_guards.rs`

**Step 1: Write a guard test that currently fails**

Create:

```rust
#[test]
fn agent_does_not_mutate_session_snapshot_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in [
        "snapshot.conversation.push",
        "snapshot.open_exec_sessions",
        "snapshot.pending_approvals",
        "LiveEventSink",
    ] {
        assert!(
            !agent.contains(forbidden),
            "src/runtime/agent.rs should not contain {forbidden}"
        );
    }
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test architecture_guards agent_does_not_mutate_session_snapshot_directly -- --nocapture
```

Expected:

```text
FAIL because src/runtime/agent.rs still mutates snapshot and imports LiveEventSink.
```

**Step 3: Commit failing test**

Commit the failing architecture guard only if your workflow allows red commits. If not, keep it uncommitted until Task 5 makes it pass.

```bash
git add tests/architecture_guards.rs
git commit -m "test: guard agent against direct session mutation"
```

## Task 3: Introduce Turn Effects Types

**Files:**

- Modify: `src/runtime/agent.rs`

**Step 1: Add effect/result types near `Agent`**

Add:

```rust
pub(crate) enum AgentTurnStep {
    AssistantTurn(AssistantTurn),
    ToolResult(ToolExecutionOutcome),
}

pub(crate) struct ToolExecutionOutcome {
    pub result: crate::types::ToolResult,
    pub effects: Vec<ToolEffect>,
}

pub(crate) enum ToolEffect {
    ExecSessionUpdate(ExecSessionUpdate),
    ApprovalUpdate(ApprovalUpdate),
}

pub(crate) enum ExecSessionUpdate {
    Running {
        exec_session_id: ExecSessionId,
        command: String,
        cwd: PathBuf,
    },
    NotRunning {
        exec_session_id: ExecSessionId,
    },
}

pub(crate) enum ApprovalUpdate {
    Requested {
        approval_id: ApprovalId,
        tool_name: String,
        reason: String,
    },
    Approved {
        approval_id: ApprovalId,
    },
    Denied {
        approval_id: ApprovalId,
    },
}
```

These can start in `agent.rs` to keep the change small. Later tasks may move them to `runtime/turn_effects.rs` or `runtime/tool_call_runtime.rs`.

**Step 2: Add conversion helpers from current `ToolResult.meta`**

Temporarily keep parsing `ToolResult.meta`, but parse it into typed effects:

```rust
fn tool_effects_from_result(result: &crate::types::ToolResult, fallback_cwd: &Path) -> Vec<ToolEffect>
```

This preserves behavior while moving mutation out of `Agent`.

**Step 3: Run compile**

Run:

```bash
cargo check
```

Expected:

```text
PASS
```

**Step 4: Commit**

```bash
git add src/runtime/agent.rs
git commit -m "refactor: introduce typed turn effects"
```

## Task 4: Change Agent To Return Steps Instead Of Recording Events

**Files:**

- Modify: `src/runtime/agent.rs`
- Modify: `src/runtime/thread_session/turn.rs`

**Step 1: Add new Agent method**

Add a new method beside `legacy live-turn runner`:

```rust
pub(crate) async fn run_turn_steps(
    &self,
    history: &[ConversationMessage],
    runtime_turn_id: TurnId,
    session_id: SessionId,
    workspace_root: PathBuf,
    cwd: PathBuf,
) -> Result<Vec<AgentTurnStep>>
```

Initial implementation may still maintain a local `messages` vector internally, but it must return steps instead of touching snapshot or sink.

Inside the loop:

```rust
let turn = self.llm.complete(&messages, &self.registry.schemas()).await?;
steps.push(AgentTurnStep::AssistantTurn(turn.clone()));
messages.push(ConversationMessage::assistant(turn.text.clone(), turn.tool_calls.clone()));

for call in turn.tool_calls.clone() {
    let result = self.registry.execute(call, Some(&ctx)).await;
    let effects = tool_effects_from_result(&result, &cwd);
    let outcome = ToolExecutionOutcome { result: result.clone(), effects };
    messages.push(ConversationMessage::tool(result.tool_call_id.clone(), serde_json::to_string(&result)?));
    steps.push(AgentTurnStep::ToolResult(outcome));
}
```

**Step 2: Make `legacy live-turn runner` delegate temporarily**

Keep `legacy live-turn runner` for compatibility during this task. It can call `run_turn_steps` and then apply the old mutation logic locally. This keeps behavior stable while `ThreadSession` is changed in the next task.

**Step 3: Run tests**

Run:

```bash
cargo test runtime::thread_session::turn::tests -- --nocapture
cargo test --test thread_runtime -- --nocapture
```

Expected:

```text
PASS
```

**Step 4: Commit**

```bash
git add src/runtime/agent.rs src/runtime/thread_session/turn.rs
git commit -m "refactor: make agent produce turn steps"
```

## Task 5: Move Step Application Into ThreadSession

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/thread_session/events.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/agent.rs`

**Step 1: Add ThreadSession methods for step application**

In `src/runtime/thread_session/turn.rs` or a new `src/runtime/thread_session/apply.rs`, add:

```rust
impl ThreadSession {
    pub(crate) fn history_for_prompt_from_snapshot(
        snapshot: &SessionSnapshot,
    ) -> Vec<ConversationMessage> {
        snapshot.conversation.clone()
    }

    fn apply_agent_step(
        &mut self,
        snapshot: &mut SessionSnapshot,
        turn_id: &TurnId,
        step: AgentTurnStep,
    ) -> Result<Option<AssistantTurn>> {
        match step {
            AgentTurnStep::AssistantTurn(turn) => {
                if turn.text.is_some() || !turn.tool_calls.is_empty() {
                    snapshot
                        .conversation
                        .push(ConversationMessage::assistant(turn.text.clone(), turn.tool_calls.clone()));
                }
                self.append_and_broadcast_snapshot(
                    snapshot,
                    Some(turn_id),
                    RuntimeEventKind::AssistantTurn { turn: turn.clone() },
                )?;
                Ok(if turn.tool_calls.is_empty() { Some(turn) } else { None })
            }
            AgentTurnStep::ToolResult(outcome) => {
                self.apply_tool_effects(snapshot, turn_id, &outcome)?;
                snapshot.conversation.push(ConversationMessage::tool(
                    outcome.result.tool_call_id.clone(),
                    serde_json::to_string(&outcome.result)?,
                ));
                self.append_and_broadcast_snapshot(
                    snapshot,
                    Some(turn_id),
                    RuntimeEventKind::ToolResult {
                        result: outcome.result,
                    },
                )?;
                Ok(None)
            }
        }
    }
}
```

**Step 2: Add `apply_tool_effects`**

Move old logic from `agent.rs` into `ThreadSession`:

- old `apply_exec_session_update`
- old `record_deferred_policy_event`
- old `apply_pending_approval_update`

The new method should consume typed `ToolEffect` values, not parse JSON meta directly.

Approval requested needs a reserved event id:

```text
reserve event id
insert PendingApproval with requested_event_id
record_reserved ApprovalRequested
```

Approval approved/denied records `ApprovalDecision`.

**Step 3: Change `handle_user_input_inner`**

Replace:

```rust
agent.run_legacy_live_turn(&mut snapshot, turn_id.clone(), turn_cwd, recorder)
```

with:

```rust
let cwd = turn_cwd.unwrap_or_else(|| snapshot.cwd.clone());
let steps = self.agent.run_turn_steps(
    &snapshot.conversation,
    turn_id.clone(),
    snapshot.session_id.clone(),
    snapshot.workspace_root.clone(),
    cwd,
).await?;

let mut final_turn = None;
for step in steps {
    if let Some(turn) = self.apply_agent_step(&mut snapshot, &turn_id, step)? {
        final_turn = Some(turn);
    }
}
let final_turn = final_turn.ok_or_else(|| anyhow::anyhow!("turn completed without final assistant turn"))?;
```

**Step 4: Remove direct use of `recorder` from `handle_user_input_inner`**

After this task, `Agent` should not need `LiveEventSink` for the live path.

**Step 5: Run focused tests**

Run:

```bash
cargo test runtime::thread_session::turn::tests -- --nocapture
cargo test --test thread_runtime -- --nocapture
cargo test --test app_server_boundary events_subscribe_receives_live_approval_requested_events -- --nocapture
cargo test --test app_server_boundary turn_interrupt_clears_waiting_approval_and_records_interrupted_event -- --nocapture
```

Expected:

```text
PASS
```

**Step 6: Run architecture guard**

Run:

```bash
cargo test --test architecture_guards agent_does_not_mutate_session_snapshot_directly -- --nocapture
```

Expected:

```text
PASS after direct snapshot mutation is removed from src/runtime/agent.rs.
```

**Step 7: Commit**

```bash
git add src/runtime/agent.rs src/runtime/thread_session src/runtime/thread_session/turn.rs tests/architecture_guards.rs
git commit -m "refactor: make thread session apply live turn steps"
```

## Task 6: Remove Agent's Local Persistent Working Conversation

**Files:**

- Modify: `src/runtime/agent.rs`
- Modify: `src/runtime/thread_session/turn.rs`

**Step 1: Add a sampling callback API**

Change `Agent` so it performs one model/tool cycle at a time:

```rust
pub(crate) async fn sample_next(
    &self,
    prompt: &[ConversationMessage],
    runtime_turn_id: TurnId,
    session_id: SessionId,
    workspace_root: PathBuf,
    cwd: PathBuf,
) -> Result<AgentTurnStepBatch>
```

Where:

```rust
pub(crate) struct AgentTurnStepBatch {
    pub assistant_turn: AssistantTurn,
    pub tool_results: Vec<ToolExecutionOutcome>,
}
```

This method must not keep `let mut messages = ...` across multiple model calls.

**Step 2: Move loop to ThreadSession**

In `handle_user_input_inner`, replace one call to `run_turn_steps` with:

```rust
for _ in 0..self.agent.max_turns() {
    let prompt = snapshot.conversation.clone();
    let batch = self.agent.sample_next(...prompt...).await?;

    let assistant = batch.assistant_turn.clone();
    self.apply_agent_step(&mut snapshot, &turn_id, AgentTurnStep::AssistantTurn(assistant.clone()))?;

    if assistant.tool_calls.is_empty() {
        final_turn = Some(assistant);
        break;
    }

    for outcome in batch.tool_results {
        self.apply_agent_step(&mut snapshot, &turn_id, AgentTurnStep::ToolResult(outcome))?;
    }
}
```

This makes each sampling request derive from the current committed snapshot.

**Step 3: Delete `run_turn_steps` if no longer needed**

Keep compatibility only if tests still rely on it. Prefer deleting it once `ThreadSession` owns the loop.

**Step 4: Add architecture guard for local working conversation**

In `tests/architecture_guards.rs`, add:

```rust
#[test]
fn agent_does_not_keep_mutable_working_conversation() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains("let mut messages ="),
        "Agent should not keep a mutable working conversation"
    );
}
```

**Step 5: Run tests**

Run:

```bash
cargo test runtime::thread_session::turn::tests::thread_session_next_sampling_uses_committed_history -- --nocapture
cargo test --test architecture_guards -- --nocapture
cargo test --test thread_runtime -- --nocapture
```

Expected:

```text
PASS
```

**Step 6: Commit**

```bash
git add src/runtime/agent.rs src/runtime/thread_session/turn.rs tests/architecture_guards.rs
git commit -m "refactor: derive each sampling prompt from thread session history"
```

## Task 7: Introduce ToolCallRuntime Module

**Files:**

- Create: `src/runtime/tool_call_runtime.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/agent.rs`
- Modify: `src/runtime/thread_session/turn.rs`

**Step 1: Create module**

Add to `src/runtime/mod.rs`:

```rust
pub(crate) mod tool_call_runtime;
```

Create `src/runtime/tool_call_runtime.rs` with:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::config::AgentConfig;
use crate::exec_session::ExecSessionManager;
use crate::policy::PolicyManager;
use crate::registry::{ToolContext, ToolRegistry};
use crate::session::{ApprovalId, ExecSessionId};
use crate::types::{ToolCall, ToolResult, TurnId, SessionId};

pub(crate) struct ToolCallRuntime {
    config: AgentConfig,
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    session_id: SessionId,
    turn_id: TurnId,
    cwd: PathBuf,
}
```

Move `ToolExecutionOutcome`, `ToolEffect`, `ExecSessionUpdate`, and `ApprovalUpdate` into this module.

**Step 2: Add constructor and execute**

```rust
impl ToolCallRuntime {
    pub(crate) fn new(...) -> Self { ... }

    pub(crate) fn schemas(&self) -> Vec<serde_json::Value> {
        self.registry.schemas()
    }

    pub(crate) async fn execute(&self, call: ToolCall) -> ToolExecutionOutcome {
        let ctx = ToolContext {
            config: self.config.clone(),
            session_id: Some(self.session_id.clone()),
            turn_id: Some(self.turn_id.clone()),
            exec_sessions: self.exec_sessions.clone(),
            policy: self.policy.clone(),
            defer_policy_events: true,
        };
        let result = self.registry.execute(call, Some(&ctx)).await;
        let effects = tool_effects_from_result(&result, &self.cwd);
        ToolExecutionOutcome { result, effects }
    }
}
```

**Step 3: Change Agent to use ToolCallRuntime**

`Agent` should no longer build `ToolContext` directly. It should receive or create `ToolCallRuntime` and call `execute`.

Temporary acceptable shape:

```rust
let tool_runtime = self.tool_runtime(...);
let turn = self.llm.complete(prompt, &tool_runtime.schemas()).await?;
let outcome = tool_runtime.execute(call).await;
```

**Step 4: Run tests**

Run:

```bash
cargo check
cargo test --test run_command -- --nocapture
cargo test --test exec_session -- --nocapture
cargo test --test policy -- --nocapture
cargo test --test thread_runtime -- --nocapture
```

Expected:

```text
PASS
```

**Step 5: Commit**

```bash
git add src/runtime/mod.rs src/runtime/tool_call_runtime.rs src/runtime/agent.rs src/runtime/thread_session/turn.rs
git commit -m "refactor: introduce tool call runtime"
```

## Task 8: Reduce Agent To Model Sampling

**Files:**

- Modify: `src/runtime/agent.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/tool_call_runtime.rs`

**Step 1: Rename behavior mentally before renaming type**

Do not rename `Agent` yet. First reduce behavior.

`Agent` should expose:

```rust
pub(crate) async fn sample_assistant_turn(
    &self,
    prompt: &[ConversationMessage],
    tool_schemas: &[serde_json::Value],
) -> Result<AssistantTurn>
```

This method should only call:

```rust
self.llm.complete(prompt, tool_schemas).await
```

**Step 2: Move tool execution loop fully to ThreadSession**

`ThreadSession` should:

- create `ToolCallRuntime`
- ask `Agent` for assistant turn
- record assistant turn
- execute tool calls through `ToolCallRuntime`
- record tool results
- repeat

**Step 3: Update architecture guard**

In `tests/architecture_guards.rs`, strengthen:

```rust
#[test]
fn agent_does_not_execute_tools_directly() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    assert!(
        !agent.contains(".execute(call"),
        "Agent should not execute tools directly"
    );
    assert!(
        !agent.contains("ToolContext"),
        "Agent should not build ToolContext directly"
    );
}
```

**Step 4: Run tests**

Run:

```bash
cargo test --test architecture_guards -- --nocapture
cargo test runtime::thread_session::turn::tests -- --nocapture
cargo test --test thread_runtime -- --nocapture
cargo test --test app_server_boundary -- --nocapture
```

Expected:

```text
PASS
```

**Step 5: Commit**

```bash
git add src/runtime/agent.rs src/runtime/thread_session/turn.rs src/runtime/tool_call_runtime.rs tests/architecture_guards.rs
git commit -m "refactor: reduce agent to model sampling"
```

## Task 9: Replace JSON Meta Parsing With Typed Tool Effects Boundary

**Files:**

- Modify: `src/tools/run_command.rs`
- Modify: `src/runtime/policy.rs`
- Modify: `src/runtime/tool_call_runtime.rs`
- Modify: `tests/policy.rs`
- Modify: `tests/exec_session.rs`

**Step 1: Keep external ToolResult meta compatibility**

Do not remove `ToolResult.meta` yet. Existing tests and app-server surfaces may still expect it.

**Step 2: Add optional typed effect extraction close to tool execution**

Keep parsing inside `ToolCallRuntime`, not `Agent`.

The guard for this phase is:

```text
JSON meta parsing may exist in ToolCallRuntime.
JSON meta parsing must not exist in Agent.
```

**Step 3: Add architecture guard**

```rust
#[test]
fn agent_does_not_parse_tool_meta() {
    let agent = std::fs::read_to_string("src/runtime/agent.rs").expect("read src/runtime/agent.rs");
    for forbidden in ["approval_status", "approval_id", "exec_session_id", "lifecycle"] {
        assert!(
            !agent.contains(forbidden),
            "Agent should not parse tool meta key {forbidden}"
        );
    }
}
```

**Step 4: Run tests**

Run:

```bash
cargo test --test architecture_guards -- --nocapture
cargo test --test policy -- --nocapture
cargo test --test exec_session -- --nocapture
```

Expected:

```text
PASS
```

**Step 5: Commit**

```bash
git add src/runtime/tool_call_runtime.rs src/runtime/agent.rs tests/architecture_guards.rs tests/policy.rs tests/exec_session.rs
git commit -m "refactor: keep tool meta parsing inside tool runtime"
```

## Task 10: Clean Up LiveEventSink Exposure

**Files:**

- Modify: `src/runtime/thread_session/events.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/runtime/agent.rs`
- Modify: `tests/architecture_guards.rs`

**Step 1: Make `LiveEventSink` private if still needed**

After `Agent` stops using it, `LiveEventSink` may only be used internally by tests or recorder. Prefer removing the trait if `ThreadEventRecorder` can be called directly.

Possible simplification:

```rust
impl ThreadEventRecorder {
    pub(crate) fn reserve_event_id(&mut self) -> EventId { ... }
    pub(crate) fn record_reserved(...) -> Result<RuntimeEvent> { ... }
    pub(crate) fn record(...) -> Result<RuntimeEvent> { ... }
}
```

Then delete the `LiveEventSink` trait.

**Step 2: Update tests**

Remove `CapturingSink` tests that directly call `legacy Agent live-turn runner`, or rewrite them to test `ThreadSession` behavior.

**Step 3: Run tests**

Run:

```bash
cargo test runtime::thread_session -- --nocapture
cargo test --test architecture_guards -- --nocapture
```

Expected:

```text
PASS
```

**Step 4: Commit**

```bash
git add src/runtime/thread_session src/runtime/agent.rs tests/architecture_guards.rs
git commit -m "refactor: keep live event recording inside thread session"
```

## Task 11: Full Verification

**Files:**

- No code changes unless verification finds issues.

**Step 1: Format check**

Run:

```bash
cargo fmt -- --check
```

Expected:

```text
PASS
```

If it fails, run:

```bash
cargo fmt
```

Then rerun the check.

**Step 2: Diff check**

Run:

```bash
git diff --check
```

Expected:

```text
PASS with no output.
```

**Step 3: Compile**

Run:

```bash
cargo check
```

Expected:

```text
PASS
```

**Step 4: Full test suite**

Run:

```bash
cargo test
```

Expected:

```text
PASS
```

**Step 5: Architecture searches**

Run:

```bash
rg "snapshot\\.conversation\\.push|open_exec_sessions|pending_approvals" src/runtime/agent.rs
rg "LiveEventSink" src/runtime/agent.rs src/tools src/model/llm
rg "approval_status|approval_id|exec_session_id|lifecycle" src/runtime/agent.rs
rg "let mut messages =" src/runtime/agent.rs
rg "ToolContext" src/runtime/agent.rs
```

Expected:

```text
No matches.
```

**Step 6: Commit final cleanup**

```bash
git add .
git commit -m "refactor: complete session-centered live turn ownership"
```

## Rollback Strategy

If a later task becomes too large, stop at the previous passing milestone.

Safe partial landing points:

```text
After Task 3:
  typed effects exist, behavior unchanged.

After Task 5:
  ThreadSession applies steps, Agent still may coordinate execution.

After Task 7:
  ToolCallRuntime exists, Agent still may call it.

After Task 8:
  Agent is reduced to model sampling.
```

Avoid landing a state where:

- both `Agent` and `ThreadSession` write the same snapshot fields
- prompt derivation sometimes comes from local messages and sometimes from session history
- approval requested event is emitted without matching `pending_approvals`
- tool result is recorded without matching conversation message

## Definition Of Done

The feature is complete when:

1. `cargo test` passes.
2. `src/runtime/agent.rs` no longer mutates `SessionSnapshot`.
3. `src/runtime/agent.rs` no longer imports or references `LiveEventSink`.
4. `src/runtime/agent.rs` no longer builds `ToolContext`.
5. `src/runtime/agent.rs` no longer parses tool meta keys.
6. Each model sampling prompt is derived from committed `ThreadSession` history.
7. `ToolCallRuntime` owns tool execution mechanics.
8. `ThreadSession` owns history/snapshot/event mutation.
9. Architecture docs match the implemented shape.
