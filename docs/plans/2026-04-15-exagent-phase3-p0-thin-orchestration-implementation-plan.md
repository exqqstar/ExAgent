# ExAgent Phase 3 P0 Thin Orchestration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add the smallest viable multi-agent orchestration layer for ExAgent by introducing parent/child session lineage, agent roles, and thin fork/spawn entrypoints without building a full planner.

**Architecture:** Keep `Agent::run_session(...)` as the single-session execution kernel and layer orchestration above it. Persist parent/child lineage and agent-role metadata in `SessionSnapshot` and `RuntimeEvent`, then expose one thin orchestration surface through CLI/API so a lead session can fork child sessions for `spec`, `test`, `judge`, or `implementation` work. Do not add a workflow engine, mailbox runtime, or complex task graph in this milestone.

**Tech Stack:** Rust, Tokio, Serde, Serde JSON, Anyhow, Axum

**Relevant Skills During Execution:** `@superpowers:test-driven-development`, `@superpowers:verification-before-completion`, `@superpowers:systematic-debugging`

**Current Baseline**

- `src/agent.rs` executes exactly one runtime session at a time and only supports `run_with_meta(...)` and `resume(...)`.
- `src/session.rs` persists `SessionSnapshot`, but it has no parent/child lineage or agent role fields.
- `src/events.rs` persists runtime events, but it has no orchestration lifecycle events such as child-session creation.
- `src/api.rs` and `src/cli.rs` only expose `run` and `resume`.
- `tests/resume.rs`, `tests/exec_session.rs`, and `tests/policy.rs` already cover the durable-runtime substrate and should remain green throughout Phase 3.

**Immediate Scope**

This plan covers `P0: Thin Orchestration Contract`. The milestone is complete when ExAgent can:

1. create a child session with explicit `parent_session_id`, `root_session_id`, `spawned_by_turn_id`, and `agent_role`
2. persist child-session creation as replayable runtime events
3. expose one thin orchestration entrypoint through CLI and API
4. prove that parent and child session artifacts stay isolated under `.exagent/sessions/<session_id>/`
5. support a lead-driven operating model for `spec`, `test`, `judge`, and `implementation` sessions

**Non-Goals For P0**

Do not expand into these areas before this milestone is green:

- full planner/runtime split
- mailbox or actor-style orchestration
- autonomous task graphs
- worktree automation
- join/reduce semantics across many children
- cross-process distributed scheduling
- context compaction or eval-harness work that is not directly required for orchestration isolation

## Recommended Agent Topology During Execution

Use this operating model while implementing the plan:

- `Lead / Integrator`: main thread only; owns milestone, scope, merge decisions, and final verification
- `Spec Agent`: docs-only or prompt-contract work; no runtime file ownership
- `Test Agent`: test matrix and regression coverage; can own `tests/orchestration.rs` and related assertions
- `Judge Agent`: read-only challenge/review after draft exists; does not co-author the draft
- `Implementation Agent A`: owns `src/session.rs`, `src/events.rs`, `src/transcript.rs`, and lineage-focused tests
- `Implementation Agent B`: owns `src/agent.rs`, `src/api.rs`, `src/cli.rs`, `src/main.rs`, and API/CLI tests

Parallelism rules:

- Never have two implementation agents edit `src/agent.rs` at the same time.
- Keep `Spec`, `Test`, and `Judge` read-only until the runtime contract is stable.
- Do not run more than two writer agents in parallel for P0.

### Task 1: Introduce agent-role and lineage primitives

**Files:**
- Modify: `src/session.rs`
- Modify: `src/agent.rs`
- Test: `tests/resume.rs`
- Create: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- `SessionSnapshot` round-trips `parent_session_id`, `root_session_id`, `spawned_by_turn_id`, and `agent_role`
- a newly created root session defaults to `agent_role = primary`
- a forked child session keeps the same `root_session_id` but gets a new `session_id`

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test resume
cargo test --test orchestration lineage_fields_round_trip -- --exact
```

Expected: FAIL because lineage fields and role types do not exist yet.

**Step 3: Write the minimal implementation**

Add:

- `AgentRole` enum with the smallest useful set: `Primary`, `Spec`, `Test`, `Judge`, `Implementation`
- lineage fields on `SessionSnapshot`:
  - `parent_session_id: Option<SessionId>`
  - `root_session_id: SessionId`
  - `spawned_by_turn_id: Option<TurnId>`
  - `agent_role: AgentRole`
- root-session initialization in `Agent::run_with_meta(...)`

Do not add `task_id`, branch metadata, or worktree metadata in this task.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test resume
cargo test --test orchestration lineage_fields_round_trip -- --exact
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/session.rs src/agent.rs tests/resume.rs tests/orchestration.rs
git commit -m "feat: add phase3 session lineage primitives"
```

### Task 2: Add replayable orchestration lifecycle events

**Files:**
- Modify: `src/events.rs`
- Modify: `src/agent.rs`
- Modify: `src/transcript.rs`
- Test: `tests/resume.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that prove:

- a child-session creation event round-trips through JSON
- parent replay shows the child-session spawn event without rerunning tools
- child replay includes its own creation metadata

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test resume replay_reads_persisted_events_without_rerunning_tools -- --exact
cargo test --test orchestration session_spawn_event_round_trips -- --exact
```

Expected: FAIL because orchestration lifecycle events do not exist yet.

**Step 3: Write the minimal implementation**

Add one replayable lifecycle event:

- `RuntimeEventKind::SessionSpawned { child_session_id, parent_session_id, agent_role, spawned_by_turn_id }`

Record it in the parent session event log when a child session is created. If needed, add a small transcript helper so event append/replay remains centralized.

Do not add join, cancel, reduce, or scheduler events in this task.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test resume
cargo test --test orchestration session_spawn_event_round_trips -- --exact
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/events.rs src/agent.rs src/transcript.rs tests/resume.rs tests/orchestration.rs
git commit -m "feat: add orchestration lifecycle events"
```

### Task 3: Add a thin child-session fork path on `Agent`

**Files:**
- Modify: `src/agent.rs`
- Modify: `src/session.rs`
- Modify: `src/transcript.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that prove:

- `Agent` can fork a child session from a parent session id
- the child inherits `workspace_root` and `cwd`
- the child gets a new `session_id`
- parent and child snapshots are written to different directories

**Step 2: Run tests to verify they fail**

Run: `cargo test --test orchestration agent_can_fork_child_session -- --exact`

Expected: FAIL because `Agent` has no child-session entrypoint yet.

**Step 3: Write the minimal implementation**

Add a narrow entrypoint such as:

```rust
pub async fn fork_session(
    &self,
    parent_session_id: &SessionId,
    agent_role: AgentRole,
    prompt: &str,
    spawned_by_turn_id: Option<&TurnId>,
) -> Result<AgentRunOutput>
```

Implementation rules:

- load the parent snapshot
- create a new child snapshot with a new `session_id`
- copy `workspace_root` and `cwd`
- carry forward `root_session_id`
- set `parent_session_id` to the parent
- start the child conversation with the provided prompt
- then execute through the existing `run_session(...)`

Do not let fork mutate the parent conversation transcript.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test orchestration`

Expected: PASS for child-session lineage and isolation tests.

**Step 5: Commit**

```bash
git add src/agent.rs src/session.rs src/transcript.rs tests/orchestration.rs
git commit -m "feat: add thin child-session fork path"
```

### Task 4: Surface orchestration through CLI and API

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/api.rs`
- Test: `tests/api_server.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that prove:

- CLI parses `fork <parent_session_id> <agent_role> '<prompt>'`
- API accepts a fork request with `parent_session_id` and `agent_role`
- the response returns the child `session_id`, `snapshot_path`, and `events_path`

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test api_server
cargo test --test orchestration cli_and_api_fork_surface_exist -- --exact
```

Expected: FAIL because only `run` and `resume` exist today.

**Step 3: Write the minimal implementation**

Add:

- `CliCommand::Fork { parent_session_id, agent_role, prompt }`
- either `POST /fork` or an explicit fork variant on `/run`
- a matching `AgentRunner` surface that routes to `Agent::fork_session(...)`

Prefer a dedicated fork surface over overloading `resume`.

Do not add list-sessions, join, or status dashboards in this task.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test api_server
cargo test --test orchestration cli_and_api_fork_surface_exist -- --exact
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/cli.rs src/main.rs src/api.rs tests/api_server.rs tests/orchestration.rs
git commit -m "feat: expose thin orchestration entrypoints"
```

### Task 5: Add orchestration isolation and replay regressions

**Files:**
- Modify: `tests/orchestration.rs`
- Modify: `tests/resume.rs`
- Modify: `tests/api_server.rs`

**Step 1: Write the failing tests**

Add regression coverage for:

- two child sessions forked from the same parent do not share `session_id`
- parent and child artifacts land in different `.exagent/sessions/<session_id>/` directories
- `replay_session(...)` on the parent shows both child-session spawn records in order
- `resume(...)` on one child does not mutate sibling-child snapshots

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration
cargo test --test resume
cargo test --test api_server
```

Expected: at least one orchestration regression FAILS until isolation assertions exist.

**Step 3: Write the minimal implementation**

If any regression reveals missing glue, add only the smallest supporting code needed for:

- event ordering
- child-path isolation
- replay visibility
- sibling-session isolation

Keep this task focused on correctness, not feature expansion.

**Step 4: Run tests to verify they pass**

Run: `cargo test`

Expected: PASS

**Step 5: Commit**

```bash
git add tests/orchestration.rs tests/resume.rs tests/api_server.rs
git commit -m "test: add orchestration isolation regressions"
```

## Phase 3 P0 Scope Guardrails

Do not claim Phase 3 orchestration is complete until these statements are true:

- parent/child lineage is visible in both snapshot state and event replay
- one lead session can fork at least one child session through a supported CLI/API path
- child sessions are isolated from each other on disk
- all Phase 2 runtime tests still pass
- new orchestration tests pass without loosening policy or exec-session guarantees

## Recommended Sequencing

1. Land Tasks 1 and 2 before touching CLI/API.
2. Land Task 3 before adding any operator-facing orchestration surface.
3. Land Task 4 before asking implementation agents to rely on child sessions.
4. Land Task 5 before running more than one child implementation session in parallel.

## Nice-To-Have After P0

These are worthwhile, but they should not block the thin orchestration contract:

- worktree or branch metadata on sessions
- `join` / `collect` semantics for child outputs
- orchestration status APIs
- structured judge outputs
- scenario eval harness for long orchestration sessions
- compaction-aware orchestration replay

Plan complete and saved to `docs/plans/2026-04-15-exagent-phase3-p0-thin-orchestration-implementation-plan.md`. Two execution options:

**1. Subagent-Driven (this session)** - implement one task at a time with fresh worker sessions and review checkpoints

**2. Parallel Session (separate)** - open a new session with `executing-plans` and execute the plan in a dedicated worktree
