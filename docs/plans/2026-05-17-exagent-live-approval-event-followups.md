# ExAgent Live Approval Event Follow-Ups

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the remaining live runtime ownership gaps found after the
F3-F5 thread-session refactor.

**Architecture:** `ThreadSession` must be the only live runtime event owner.
Tools may request policy approvals and return metadata, but the live Agent path
must hand approval request/decision events back to `ThreadSession` for event id
assignment, persistence, live broadcast, and `thread_read` publication. Disk
remains full replay storage; the loaded runtime's live event buffer is a bounded
recent window.

**Tech Stack:** Rust, Tokio, app-server boundary protocol, ExAgent
`ThreadRuntime` / `ThreadSession` / `Agent` live path.

## Review Findings

### P1: Approval Events Bypass ThreadSession

Before this follow-up, `src/tools/run_command.rs` wrote
`ApprovalRequested` and `ApprovalDecision` directly to `events.jsonl`. In the
live runtime path this bypassed `ThreadEventRecorder`, so the events were not
broadcast through `events/subscribe` and did not enter the loaded
`ThreadSession` live event buffer.

This violates the ownership matrix:

```text
ThreadSession owns event id assignment, event persistence, event broadcast,
and live read publication.
```

Fix: live `ToolContext` should defer approval event recording. `run_command`
returns policy metadata only; `legacy Agent live-turn runner` records approval events
through the live `LiveEventSink`.

### P2: Bounded Live Event Buffer Changes thread_read Semantics

The loaded runtime now exposes only a bounded recent event window through
`live_view().events`. That is acceptable for V1, but it must be documented:
`thread_read` on a loaded runtime is a live/recent view, while
`events/replay` remains the complete persisted event log.

### P2: Architecture Guard Misses Tools

The current grep guard checks `src/agent.rs src/runtime`, but approval events
were written from `src/tools/run_command.rs`. The guard must include
`src/tools` or it will miss the exact ownership violation this review found.

### P3: API Drift in Plans

The authoritative runtime plan still shows `runtime.live_view().await?`.
The implemented API is synchronous and returns `ThreadSessionLiveView`
directly.

## Implementation Tasks

### Task 1: Capture Live Approval Broadcast Failure

**Files:**

- Modify: `tests/app_server_boundary.rs`

Add a test that subscribes before `turn_start`, triggers an enforced-policy
`run_command`, and asserts the live broadcast receiver gets
`RuntimeEventKind::ApprovalRequested`.

Expected before fix: the test times out because `run_command` writes directly
to disk instead of broadcasting through `ThreadSession`.

### Task 2: Defer Tool Approval Events in Live Context

**Files:**

- Modify: `src/registry.rs`
- Modify: `src/agent.rs`
- Modify: `src/tools/run_command.rs`
- Update tests constructing `ToolContext`

Add `defer_policy_events: bool` to `ToolContext`.

- Non-live `run_legacy_session_snapshot` uses `false` to preserve existing legacy
  behavior.
- Live `run_live_session_snapshot` uses `true`.
- `run_command` writes approval events only when `defer_policy_events == false`.

### Task 3: Record Deferred Approval Events Through ThreadSession

**Files:**

- Modify: `src/agent.rs`

In the live path, after tool execution:

1. Inspect tool metadata for `approval_id` and `approval_status`.
2. If status is `pending`, reserve an event id from `LiveEventSink`.
3. Mutate `PendingApproval.requested_event_id` with that reserved id before
   recording the event, so the paired live snapshot is already
   `WaitingApproval`.
4. Record `ApprovalRequested` through `LiveEventSink` with the reserved id.
5. If status is `approved` or `denied`, record `ApprovalDecision` through
   `LiveEventSink`.
6. Continue recording the `ToolResult` event through the same sink.

### Task 4: Update Docs and Guards

**Files:**

- Modify: `docs/plans/2026-05-17-exagent-thread-session-authoritative-runtime.md`
- Modify: `docs/plans/2026-05-17-exagent-thread-runtime-followup-fixes.md`
- Modify: `docs/architecture/2026-05-16-exagent-thread-runtime-actor-v2-design.md`

Update grep guards to include `src/tools`. Fix stale `live_view().await?`
examples. Document that loaded `thread_read` uses a bounded live event window
and `events/replay` is the full timeline source.

## Verification

Run:

```bash
cargo fmt -- --check
git diff --check
cargo test app_server_boundary
cargo test policy
cargo test thread_runtime
cargo test
```

Run the guard:

```bash
rg "append_json_line|append_runtime_event|write_json" src/agent.rs src/runtime src/tools
```

Intentional matches must be legacy non-live `Agent::run_legacy_session_snapshot`,
`ThreadSession` event recording, tests, or non-runtime approval decision
compatibility. Live approval events must no longer be directly written from
`run_command`.
