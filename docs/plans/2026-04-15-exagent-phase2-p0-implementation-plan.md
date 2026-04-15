# ExAgent Phase 2 P0 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn the current Phase 1 single-run agent loop into a durable coding-agent runtime that can resume interrupted work, keep shell state across turns, enforce approval on risky actions, compact long sessions, and replay runs for debugging.

**Architecture:** Keep the existing `Agent + ToolRegistry + LlmClient` shape, but wrap it in a session-oriented runtime. Persist typed session state and runtime events under `.exagent/sessions/<session_id>/`, treat one-shot command execution and persistent exec sessions as separate runtime paths, and rebuild model context from persisted state plus compacted artifacts instead of only rebuilding from raw messages.

**Tech Stack:** Rust, Tokio, Serde, Serde JSON, Anyhow, Thiserror, Tracing, Axum

**Relevant Skills During Execution:** `@superpowers:test-driven-development`, `@superpowers:verification-before-completion`, `@superpowers:systematic-debugging`

**Current Baseline**

- `src/agent.rs` runs an in-memory message loop and writes append-only JSONL transcripts.
- `src/types.rs` models chat messages and tool calls/results, but not sessions, events, approvals, or resumable runtime state.
- `src/tools/run_command.rs` only supports one-shot commands with timeout and truncated output.
- `src/registry.rs` exposes tool name/description/schema only; it has no risk metadata or policy hook.
- `src/transcript.rs` creates per-run JSONL files, but not structured replayable session artifacts.

**Immediate Scope**

This plan covers the `P0: Durable Runtime` milestone from `docs/plans/2026-04-13-exagent-phase2-runtime-design.md`. Do not start the leverage-layer work until these primitives are stable:

1. session and event model
2. persistence and resume
3. persistent exec session
4. verification artifact feedback path
5. policy and approval hook
6. basic compaction
7. replay and scenario eval harness

### Task 1: Introduce typed session and runtime event primitives

**Files:**
- Create: `src/session.rs`
- Create: `src/events.rs`
- Modify: `src/types.rs`
- Modify: `src/lib.rs`
- Test: `tests/resume.rs`

**Step 1: Write the failing tests**

Add tests that round-trip a `SessionSnapshot` and `RuntimeEvent` through JSON and assert stable identifiers for `session_id`, `turn_id`, and `event_id`.

**Step 2: Run tests to verify they fail**

Run: `cargo test --test resume`
Expected: FAIL because the session/event types do not exist yet.

**Step 3: Write the minimal implementation**

Add:
- `SessionSnapshot` with `session_id`, `workspace_root`, `cwd`, `conversation`, `open_exec_sessions`, `latest_compaction`, and `pending_approvals`
- `RuntimeEvent` enum for assistant turns, tool results, exec output, approval requests, approval decisions, compaction artifacts, and runtime errors
- typed IDs as string wrappers or validated aliases instead of anonymous strings wherever practical

**Step 4: Run tests to verify they pass**

Run: `cargo test --test resume session_snapshot_round_trips_to_json -- --exact`
Expected: PASS

**Step 5: Commit**

```bash
git add src/session.rs src/events.rs src/types.rs src/lib.rs tests/resume.rs
git commit -m "feat: add phase2 session and event primitives"
```

### Task 2: Persist sessions and support resume/replay entrypoints

**Files:**
- Modify: `src/agent.rs`
- Modify: `src/transcript.rs`
- Modify: `src/main.rs`
- Modify: `src/api.rs`
- Modify: `src/config.rs`
- Test: `tests/resume.rs`
- Test: `tests/api_server.rs`

**Step 1: Write the failing tests**

Add tests that:
- start a run, persist a session snapshot, and resume it in a second call
- reconstruct a run timeline from persisted events without rerunning tools
- verify API and CLI can target an existing `session_id`

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test resume
cargo test --test api_server
```

Expected: FAIL because the runtime only supports fire-once runs with transcript files.

**Step 3: Write the minimal implementation**

Persist runtime state under:

```text
.exagent/sessions/<session_id>/
  snapshot.json
  events.jsonl
  compaction/
```

Add:
- session creation and snapshot loading in `Agent`
- `resume` path that rebuilds active context from `snapshot.json` plus recent events
- replay helper that reads persisted events and returns an inspectable timeline
- API support for `session_id` on `/run`, and CLI support for `resume <session_id>`

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test resume
cargo test --test api_server
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/agent.rs src/transcript.rs src/main.rs src/api.rs src/config.rs tests/resume.rs tests/api_server.rs
git commit -m "feat: persist sessions and add resume flow"
```

### Task 3: Add a persistent exec-session runtime

**Files:**
- Create: `src/exec_session.rs`
- Modify: `src/tools/run_command.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/registry.rs`
- Modify: `src/agent.rs`
- Test: `tests/exec_session.rs`

**Step 1: Write the failing tests**

Add tests that prove:
- a command can stay alive across turns
- stdin can be written into a live session
- stdout/stderr are appended as runtime events
- cancellation closes the tracked session cleanly

**Step 2: Run tests to verify they fail**

Run: `cargo test --test exec_session`
Expected: FAIL because all command execution is currently one-shot.

**Step 3: Write the minimal implementation**

Add an `ExecSessionManager` keyed by `session_id`, and separate two execution modes:
- existing `run_command` for short one-shot commands
- new persistent exec flow that returns `exec_session_id`, current buffered output, and lifecycle state

Minimum behaviors:
- spawn process with piped stdio
- append output as `RuntimeEvent::ExecOutput`
- allow `stdin` writes to a live session
- support explicit `terminate`

Keep the agent-facing contract narrow; avoid introducing many overlapping command tools.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test exec_session`
Expected: PASS

**Step 5: Commit**

```bash
git add src/exec_session.rs src/tools/run_command.rs src/tools/mod.rs src/registry.rs src/agent.rs tests/exec_session.rs
git commit -m "feat: add persistent exec sessions"
```

### Task 4: Add policy classification and approval hooks

**Files:**
- Create: `src/policy.rs`
- Modify: `src/config.rs`
- Modify: `src/agent.rs`
- Modify: `src/tools/run_command.rs`
- Modify: `src/exec_session.rs`
- Test: `tests/policy.rs`

**Step 1: Write the failing tests**

Add tests that cover:
- safe commands executing immediately
- risky commands returning `review_required`
- explicit approve and deny flows
- approval decisions being written into the session event log

**Step 2: Run tests to verify they fail**

Run: `cargo test --test policy`
Expected: FAIL because command execution has no policy boundary today.

**Step 3: Write the minimal implementation**

Add:
- `PolicyDecision::{Allow, Deny, ReviewRequired}`
- command classifier that inspects command, cwd, and tool risk metadata
- `PendingApproval` records persisted in session state
- config-driven policy mode such as `off`, `advisory`, `enforced`

The first implementation can focus on `run_command` and persistent exec spawn requests. Do not block Phase 2 on full OS sandboxing.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test policy`
Expected: PASS

**Step 5: Commit**

```bash
git add src/policy.rs src/config.rs src/agent.rs src/tools/run_command.rs src/exec_session.rs tests/policy.rs
git commit -m "feat: add policy and approval hooks"
```

### Task 5: Add verification artifacts and context compaction

**Files:**
- Create: `src/context.rs`
- Modify: `src/agent.rs`
- Modify: `src/session.rs`
- Modify: `src/events.rs`
- Modify: `src/tools/run_command.rs`
- Test: `tests/compaction.rs`

**Step 1: Write the failing tests**

Add tests that verify:
- command results tagged as build/test/lint create a structured verification artifact
- context budget overflow triggers compaction
- compacted sessions rehydrate required facts, approvals, and recent verification outcomes

**Step 2: Run tests to verify they fail**

Run: `cargo test --test compaction`
Expected: FAIL because there is no budget tracker, compaction artifact, or verification model.

**Step 3: Write the minimal implementation**

Add:
- lightweight `VerificationArtifact` with `command_kind`, `exit_code`, `cwd`, timestamps, and output references
- byte-based or token-estimate budget tracker
- compaction trigger policy
- deterministic compacted artifact containing:
  - current task summary
  - pinned workspace facts
  - latest approval decisions
  - latest verification outcomes
  - active exec-session references

Rebuild model context from the compacted artifact plus a small recent tail, not the full raw transcript.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test compaction`
Expected: PASS

**Step 5: Commit**

```bash
git add src/context.rs src/agent.rs src/session.rs src/events.rs src/tools/run_command.rs tests/compaction.rs
git commit -m "feat: add compaction and verification artifacts"
```

### Task 6: Add replay tooling and a scenario-based eval harness

**Files:**
- Modify: `src/main.rs`
- Modify: `src/api.rs`
- Modify: `src/transcript.rs`
- Create: `tests/fixtures/`
- Test: `tests/resume.rs`
- Test: `tests/exec_session.rs`
- Test: `tests/policy.rs`
- Test: `tests/compaction.rs`

**Step 1: Write the failing tests**

Add scenario tests for:
- create/edit/test/fix loop in a temporary workspace
- interrupted run resumed mid-task
- denied risky command handled cleanly
- long session completing through compaction

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test resume
cargo test --test exec_session
cargo test --test policy
cargo test --test compaction
```

Expected: at least one scenario FAILS until replay and eval support exist.

**Step 3: Write the minimal implementation**

Add:
- replay command or API route that emits a structured timeline from persisted events
- fixture-driven scenario helpers for temporary workspaces
- timing and status assertions to catch runtime regressions

Use the replay artifact as the primary debugging surface for Phase 2 regressions.

**Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/main.rs src/api.rs src/transcript.rs tests/fixtures tests/resume.rs tests/exec_session.rs tests/policy.rs tests/compaction.rs
git commit -m "test: add phase2 replay and scenario evals"
```

## Phase 2 Scope Guardrails

Do not expand into these areas before the plan above is green:

- multi-agent orchestration
- connector or MCP ecosystems
- long-term semantic memory
- dynamic tool discovery
- broad product UI work
- full sandbox backends

## Recommended Sequencing

1. Finish Tasks 1 and 2 before changing tool behavior.
2. Finish Task 3 before attempting approval on long-running commands.
3. Land Task 4 before claiming ExAgent is safe for autonomous command execution.
4. Land Task 5 before advertising long-session durability.
5. Land Task 6 before starting any leverage-layer experiments.

## Nice-To-Have After P0

These are worthwhile, but they should not block the durable runtime milestone:

- `list_files`
- `search_text`
- `apply_patch`
- richer tool metadata for diagnostics
- trace diff tooling

Plan complete and saved to `docs/plans/2026-04-15-exagent-phase2-p0-implementation-plan.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

**Which approach?**
