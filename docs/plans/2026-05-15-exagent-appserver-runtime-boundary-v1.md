# ExAgent AppServer Runtime Boundary V1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a small app-server boundary that centralizes thread, turn, fork, inspect, collect, and event replay behavior while keeping existing CLI and HTTP behavior stable.

**Architecture:** Introduce `src/app_server/` with protocol DTOs, override policy, `ThreadManager`, and `AppServerService`. CLI and API adapters call the service instead of constructing `Agent` or reading orchestration state directly. V1 stays non-streaming: `turn/start` and child spawning run to completion and event replay is read-only; no `events/subscribe`.

**Tech Stack:** Rust 2021, Tokio, Axum, Serde, existing `Agent`, `SessionSnapshot`, `transcript`, `orchestration`, `ExecSessionManager`, and `PolicyManager`.

### Task 1: Boundary Protocol And Override Policy

**Files:**
- Create: `src/app_server/protocol.rs`
- Create: `src/app_server/override_policy.rs`
- Create: `src/app_server/mod.rs`
- Modify: `src/lib.rs`
- Test: `tests/app_server_boundary.rs`

**Steps:**
1. Write failing tests for workspace/cwd override resolution and fork/child-spawn override behavior.
2. Run `cargo test --test app_server_boundary override_policy -- --nocapture` and confirm missing module/type failures.
3. Implement protocol DTOs and override policy helpers.
4. Re-run the targeted tests and keep them green.

### Task 2: ThreadManager And AppServerService

**Files:**
- Create: `src/app_server/thread_manager.rs`
- Create: `src/app_server/service.rs`
- Modify: `src/app_server/mod.rs`
- Test: `tests/app_server_boundary.rs`

**Steps:**
1. Write failing tests for `thread_start`, `turn_start`, `thread_spawn_child`, `inspect`, `collect`, and `events_replay` using `MockLlm`.
2. Run the targeted test file and confirm failures are due to missing service behavior.
3. Implement `ThreadManager` around existing `Agent` runtime and durable transcript/orchestration functions.
4. Implement `AppServerService` as the public runtime boundary.
5. Re-run the targeted tests.

### Task 3: CLI And API Adapter Reuse

**Files:**
- Modify: `src/api.rs`
- Modify: `src/main.rs`
- Test: `tests/api_server.rs`

**Steps:**
1. Write failing API tests for `/thread/start`, `/turn/start`, `/thread/spawn_child`, and `/events/replay`, and update adapter tests to verify route handlers call the boundary shape.
2. Run `cargo test --test api_server -- --nocapture` and confirm failures.
3. Refactor API routes to call `AppServerService` through an adapter trait.
4. Refactor CLI command handling to call `AppServerService` for run/resume/fork/inspect/collect.
5. Re-run API tests and existing CLI parser tests.

### Task 4: Full Verification And Completion Audit

**Files:**
- All changed files

**Steps:**
1. Run `cargo test`.
2. Audit every objective requirement against concrete code and test evidence.
3. Confirm v1 has no `events/subscribe` route or streaming API.
