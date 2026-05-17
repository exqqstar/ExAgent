# Remove Legacy Agent Execution Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the pre-runtime Agent execution and orchestration surface so all active execution enters through `ThreadRuntime` and `ThreadSession`.

**Architecture:** Keep the `run` convenience adapter only because it already creates a thread and starts a turn through the runtime boundary. Remove fork/child-spawn/inspect/collect and the direct `Agent` snapshot/event writer path until child thread spawn can be rebuilt as a runtime-native operation.

**Tech Stack:** Rust, Tokio, Axum, Serde, ExAgent app-server boundary, `ThreadRuntime`, `ThreadSession`.

### Task 1: Add Failing Public Surface Tests

**Files:**
- Modify: `tests/api_server.rs`

Add tests that assert removed legacy HTTP routes return `404 Not Found`:

- `POST /fork`
- `POST /inspect`
- `POST /collect`
- `POST /thread/spawn_child`

Add a CLI parser test that `fork`, `inspect`, and `collect` are rejected.

Expected before implementation: tests fail because the routes and CLI commands still exist.

### Task 2: Remove Legacy Boundary DTOs And Trait Methods

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/service.rs`

Remove legacy orchestration DTOs and boundary methods:

- `ForkParams`
- `InspectParams`
- `CollectParams`
- `ThreadSpawnChildParams`
- `ThreadSpawnChildResponse`
- `BoundaryCapability::ThreadSpawnChild`
- `BoundaryOp::ThreadSpawnChild`
- `BoundaryOpResponse::ThreadChildSpawned`

### Task 3: Remove Legacy API And CLI Entrypoints

**Files:**
- Modify: `src/api.rs`
- Modify: `src/cli.rs`
- Modify: `src/cli_adapter.rs`

Remove HTTP routes:

- `/fork`
- `/inspect`
- `/collect`
- `/thread/spawn_child`

Remove CLI commands:

- `fork`
- `inspect`
- `collect`

### Task 4: Remove Direct Agent Execution Kernel

**Files:**
- Modify: `src/agent.rs`

Delete public Agent methods that execute outside `ThreadSession`:

- `run`
- `run_with_meta`
- `resume`
- `resume_with_turn_cwd`
- `resume_with_turn_id_cwd`
- `fork_session`
- `inspect_children`
- `collect_session`
- `run_legacy_session_snapshot`

Keep:

- constructors
- `run_live_turn`
- live execution helpers used by `ThreadSession`

### Task 5: Delete Legacy Orchestration And Structured Result Surface

**Files:**
- Delete: `src/orchestration.rs`
- Delete: `src/result_contract.rs`
- Delete: `src/tools/record_structured_result.rs`
- Modify: `src/lib.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/events.rs`
- Modify: `src/transcript.rs`
- Modify: `src/app_server/thread_manager.rs`

Remove structured result and child orchestration event handling that only served the removed child-session workflow.

### Task 6: Remove Obsolete Tests And Docs References

**Files:**
- Delete or rewrite tests that only cover removed paths:
  - `tests/orchestration.rs`
  - `tests/resume.rs`
  - `tests/structured_contracts.rs`
  - legacy portions of `tests/agent_loop.rs`
  - legacy portions of `tests/policy.rs`
  - fork/inspect/collect portions of API/CLI tests

Update docs to state child spawn is intentionally removed pending runtime-native design.

### Task 7: Verify

Run:

```bash
cargo fmt -- --check
git diff --check
cargo check
cargo test
```

Architecture guard:

```bash
rg "run_legacy_session_snapshot|fork_session|ThreadSpawnChild|record_structured_result|orchestration" src tests
rg "append_json_line|append_runtime_event|write_json" src/agent.rs src/runtime src/tools tests
```

Expected: no legacy execution/orchestration identifiers remain in `src`. Direct snapshot/event writes should remain only in `ThreadSession`, tests, and known non-live compatibility-free areas.
