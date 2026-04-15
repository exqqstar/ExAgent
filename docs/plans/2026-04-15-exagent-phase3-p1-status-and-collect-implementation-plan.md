# ExAgent Phase 3 P1 Status And Collect Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add the thinnest useful read-only inspect/collect orchestration surface so a lead operator can inspect direct child sessions and collect the latest useful output of a chosen child.

**Architecture:** Keep `Agent::run_session(...)` unchanged as the single-session execution kernel. Implement P1 as a derived read-side layer over persisted snapshots and runtime events, then expose that layer through new read-only Agent, CLI, and API entrypoints. Do not add planner logic, mailbox coordination, or child control actions in this milestone.

**Tech Stack:** Rust, Tokio, Serde, Serde JSON, Anyhow, Axum

## Current Baseline

- `src/agent.rs` supports `run_with_meta(...)`, `resume(...)`, and `fork_session(...)`.
- `src/session.rs` persists lineage and role metadata for P0.
- `src/transcript.rs` persists and replays session artifacts but has no child listing helpers.
- `src/api.rs` and `src/cli.rs` expose `run`, `resume`, and `fork`, but no inspect/collect read surfaces.
- `tests/orchestration.rs` covers lineage, spawn replay, and sibling isolation, but not inspect/collect.

## Immediate Scope

This plan covers `P1: Status And Collect`. The milestone is complete when ExAgent can:

1. list the direct child sessions of a parent session
2. expose child lineage metadata and a basic derived lifecycle status
3. expose a stable latest useful output view for a child session
4. surface inspect/collect through thin CLI and API entrypoints
5. prove that inspect/collect are read-only and do not mutate persisted session artifacts

## Non-Goals For P1

Do not expand into these areas before this milestone is green:

- planner or task decomposition behavior
- mailbox/actor orchestration
- child control actions such as cancel, retry, or resume
- worktree automation
- aggregate reduce/join semantics
- durable parent-child manifest/index files

### Task 1: Add read-side orchestration models and direct-child discovery

**Files:**
- Create: `src/orchestration.rs`
- Modify: `src/lib.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- inspect only returns direct children for a given `parent_session_id`
- child summaries include `session_id`, `parent_session_id`, `root_session_id`, `agent_role`, and `spawned_by_turn_id`
- child summaries include a derived lifecycle status based on persisted snapshot state

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration inspect_lists_direct_children_only -- --exact
```

Expected: FAIL because no inspect read-side exists yet.

**Step 3: Write the minimal implementation**

Add:

- a read-side module in `src/orchestration.rs`
- `ChildLifecycleStatus` enum with the smallest useful set: `Completed`, `Running`, `WaitingApproval`
- `ChildSessionSummary` struct
- child discovery from replayed parent `SessionSpawned` events
- child summary hydration from child snapshots

Implementation rules:

- derive `WaitingApproval` from `pending_approvals`
- derive `Running` from `open_exec_sessions` when approvals are empty
- otherwise derive `Completed`
- do not add a manifest file or extra persistence

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test orchestration inspect_lists_direct_children_only -- --exact
```

Expected: PASS

### Task 2: Add stable latest-useful-output collection

**Files:**
- Modify: `src/orchestration.rs`
- Modify: `src/transcript.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- collect prefers the latest assistant message with non-empty text
- collect falls back to the latest persisted tool result when no assistant text exists
- collect returns no output when neither exists
- collect does not mutate snapshots or rerun tools

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration collect_returns_latest_useful_output -- --exact
```

Expected: FAIL because no collect read-side exists yet.

**Step 3: Write the minimal implementation**

Add:

- `CollectedOutputKind` enum
- `CollectedOutput` struct
- `latest_useful_output` on the collected child view

Implementation rules:

- prefer latest assistant text
- fall back to latest tool result payload
- use persisted snapshot/events only
- keep all collect operations read-only

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test orchestration collect_returns_latest_useful_output -- --exact
```

Expected: PASS

### Task 3: Add Agent read-side entrypoints

**Files:**
- Modify: `src/agent.rs`
- Modify: `src/orchestration.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- `Agent` can inspect direct children for a parent session
- `Agent` can collect a child output by `session_id`
- inspect/collect are read-only and preserve snapshot file contents

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration agent_can_inspect_and_collect_children -- --exact
```

Expected: FAIL because `Agent` has no inspect/collect entrypoints yet.

**Step 3: Write the minimal implementation**

Add narrow read-only entrypoints such as:

```rust
pub fn inspect_children(&self, parent_session_id: &SessionId) -> Result<Vec<ChildSessionSummary>>
pub fn collect_session(&self, session_id: &SessionId) -> Result<CollectedChildSession>
```

Implementation rules:

- no LLM calls
- no tool execution
- no mutation of snapshots or event logs

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test orchestration agent_can_inspect_and_collect_children -- --exact
```

Expected: PASS

### Task 4: Expose inspect/collect through CLI and API

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/api.rs`
- Test: `tests/api_server.rs`
- Test: `tests/orchestration.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- CLI parses `inspect <parent_session_id>`
- CLI parses `collect <session_id>`
- API serves `POST /inspect`
- API serves `POST /collect`

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test api_server
cargo test --test orchestration cli_and_api_inspect_collect_exist -- --exact
```

Expected: FAIL because no inspect/collect surfaces exist yet.

**Step 3: Write the minimal implementation**

Add:

- `CliCommand::Inspect { parent_session_id }`
- `CliCommand::Collect { session_id }`
- matching API routes and runner methods
- JSON responses for inspect/collect

Implementation rules:

- keep inspect/collect read-only
- keep response shapes thin and directly derived from the read model
- do not overload existing fork/run routes

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test api_server
cargo test --test orchestration cli_and_api_inspect_collect_exist -- --exact
```

Expected: PASS

### Task 5: Add P1 regression coverage and full verification

**Files:**
- Modify: `tests/orchestration.rs`
- Modify: `tests/api_server.rs`
- Modify: `tests/resume.rs`

**Step 1: Write the failing tests**

Add regression coverage for:

- inspect excludes grandchildren and unrelated sessions
- inspect preserves stable parent replay order
- collect preserves latest-output precedence deterministically
- collect remains stable after child resume
- inspect/collect do not append events or rewrite snapshots
- P0 replay and sibling isolation invariants still hold

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration
cargo test --test api_server
cargo test --test resume
```

Expected: at least one P1 regression FAILS until the new assertions exist.

**Step 3: Write the minimal implementation**

If any regression reveals missing glue, add only the smallest supporting code needed for:

- deterministic latest-output collection
- direct-child filtering correctness
- read-only inspect/collect behavior

Keep this task focused on correctness, not feature expansion.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test
```

Expected: PASS

## P1 Scope Guardrails

Do not claim P1 is complete until these statements are true:

- a parent session can inspect its direct child sessions through a supported CLI/API path
- collect returns a stable latest useful output for a child session
- inspect/collect are read-only and do not mutate persisted artifacts
- all P0 orchestration tests still pass
- all existing runtime tests still pass

## Recommended Ownership Split

- Implementation A: `src/orchestration.rs`, `src/transcript.rs`, `tests/orchestration.rs`
- Implementation B: `src/agent.rs`, `src/api.rs`, `src/cli.rs`, `src/main.rs`, `tests/api_server.rs`

## Execution Note

Use the existing `Lead + Spec + Test -> Lead Synthesis -> Judge -> Implementation` workflow for this plan. Keep P1 read-only and do not let it drift into planner behavior.
