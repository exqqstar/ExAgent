# Minimal API And Eval Baseline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a minimal local HTTP API around the existing Rust agent so it can be exercised by repeatable evaluations without replacing the current CLI entrypoint.

**Architecture:** Keep the current `Agent` core and tool registry intact. Add a thin Axum server layer with `GET /health` and `POST /run`, construct a fresh agent per request from environment configuration, and write each run to its own transcript file under `.exagent/runs/`. Preserve the existing prompt-based CLI behavior and add an explicit `api` mode for starting the server.

**Tech Stack:** Rust, Tokio, Axum, Serde, Serde JSON, Reqwest, Tracing

### Task 1: Add the failing API tests

**Files:**
- Create: `tests/api_server.rs`
- Modify: `Cargo.toml`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn health_route_returns_ok() { /* ... */ }

#[tokio::test]
async fn run_route_returns_final_turn_and_request_transcript() { /* ... */ }
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test api_server`
Expected: FAIL because the API module and HTTP dependencies do not exist yet.

**Step 3: Write minimal implementation**

Add the HTTP dependencies and only the minimum surface needed by the tests.

**Step 4: Run test to verify it passes**

Run: `cargo test --test api_server`
Expected: PASS

### Task 2: Add request-scoped transcripts to the agent

**Files:**
- Modify: `src/agent.rs`
- Modify: `src/config.rs`
- Modify: `tests/agent_loop.rs`

**Step 1: Write the failing test**

Add a test proving a run can target a request-specific transcript path instead of always appending to one global file.

**Step 2: Run test to verify it fails**

Run: `cargo test --test agent_loop`
Expected: FAIL because the agent still writes to a single hard-coded transcript location.

**Step 3: Write minimal implementation**

Add a transcript directory to config and make the agent create a unique transcript file per run while keeping the default location under the workspace.

**Step 4: Run test to verify it passes**

Run: `cargo test --test agent_loop`
Expected: PASS

### Task 3: Wire the API into the CLI

**Files:**
- Create: `src/api.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

**Step 1: Write the failing test**

Cover the minimal `api` startup path indirectly through the router construction tests.

**Step 2: Run test to verify it fails**

Run: `cargo test --test api_server`
Expected: FAIL because the server entrypoint does not exist.

**Step 3: Write minimal implementation**

Add a router builder, a small server bootstrap, and keep the old prompt mode working when the first CLI argument is not `api`.

**Step 4: Run test to verify it passes**

Run: `cargo test --test api_server`
Expected: PASS

### Task 4: Verify the baseline

**Files:**
- Modify: `README` or docs only if needed later

**Step 1: Run focused verification**

Run:

```bash
cargo test --test api_server
cargo test --test agent_loop
```

Expected: PASS

**Step 2: Run full verification**

Run: `cargo test`
Expected: PASS

**Step 3: Summarize the evaluation strategy**

Document the recommendation to combine:
- public agent benchmarks for rough external comparison
- a project-specific fixed regression set for trustworthy local iteration
