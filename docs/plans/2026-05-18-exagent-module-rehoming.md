# ExAgent Module Rehoming Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rehome the current flat top-level modules into architecture-aligned directories without changing runtime behavior.

**Architecture:** Keep the current session-centered runtime design intact. This is a structure-only refactor: entrypoints stay thin, `app_server` remains the typed boundary, `runtime` owns live execution, `tools` owns tool dispatch and implementations, `state` owns persisted facts, and `model` owns LLM-facing conversation types and adapters.

**Tech Stack:** Rust 2021, Tokio, Axum, Serde/serde_json, Reqwest, schemars, existing cargo test suite.

**Implementation Status:** Implemented in the current working tree on 2026-05-18.

## Acceptance Criteria

- `cargo fmt -- --check` passes.
- `cargo check` passes.
- `cargo test` passes.
- Public compatibility imports continue to work for existing tests and likely downstream callers:
  - `crate::api`
  - `crate::cli`
  - `crate::cli_adapter`
  - `crate::agent`
  - `crate::registry`
  - `crate::session`
  - `crate::events`
  - `crate::transcript`
  - `crate::llm`
  - `crate::types`
  - `crate::exec_session`
  - `crate::policy`
- New internal canonical paths are available:
  - `crate::entrypoints::{api, cli, cli_adapter}`
  - `crate::runtime::{agent, thread_runtime, tool_call_runtime, exec_session, policy, thread_session}`
  - `crate::tools::registry`
  - `crate::state::{session, events, transcript}`
  - `crate::model::{llm, types}`
- `README.md` architecture diagram no longer mentions `legacy Agent live-turn runner`.
- Architecture docs mention the new module layout and keep the source-of-truth invariant: `ThreadSession` owns live state and conversation mutation.
- No behavior changes to thread start, turn start, replay, subscribe, approval, or exec session flows.

## Target Module Layout

```text
src/
  main.rs
  lib.rs

  entrypoints/
    mod.rs
    api.rs
    cli.rs
    cli_adapter.rs

  app_server/
    mod.rs
    protocol.rs
    service.rs
    thread_manager.rs
    override_policy.rs
    error.rs

  runtime/
    mod.rs
    agent.rs
    thread_runtime.rs
    tool_call_runtime.rs
    exec_session.rs
    policy.rs
    thread_session/
      mod.rs
      turn.rs
      events.rs

  tools/
    mod.rs
    registry.rs
    read_file.rs
    write_file.rs
    run_command.rs

  state/
    mod.rs
    session.rs
    events.rs
    transcript.rs

  model/
    mod.rs
    llm.rs
    types.rs

  config.rs
  workspace.rs
```

## Task 1: Add Characterization Guards For Canonical Module Paths

**Files:**
- Create or modify: `tests/module_layout.rs`

**Step 1: Add compile-time import coverage**

Create a lightweight integration test that imports the new canonical paths and the old compatibility paths. It does not need runtime assertions beyond constructing type references; the point is to make module moves fail fast at compile time.

Expected imports:

```rust
use exagent::entrypoints::{api, cli, cli_adapter};
use exagent::model::{llm, types};
use exagent::runtime::{agent, exec_session, policy, thread_runtime};
use exagent::state::{events, session, transcript};
use exagent::tools::registry;

use exagent::{agent as compat_agent, events as compat_events, llm as compat_llm};
use exagent::{policy as compat_policy, registry as compat_registry};
use exagent::{session as compat_session, transcript as compat_transcript};
use exagent::{types as compat_types, exec_session as compat_exec_session};
```

**Step 2: Run the focused test**

Run:

```bash
cargo test --test module_layout -- --nocapture
```

Expected before implementation: fail to compile because canonical modules do not exist.

## Task 2: Move Entrypoints Under `entrypoints/`

**Files:**
- Move: `src/api.rs` -> `src/entrypoints/api.rs`
- Move: `src/cli.rs` -> `src/entrypoints/cli.rs`
- Move: `src/cli_adapter.rs` -> `src/entrypoints/cli_adapter.rs`
- Create: `src/entrypoints/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

**Implementation:**

`src/entrypoints/mod.rs` should declare:

```rust
pub mod api;
pub mod cli;
pub mod cli_adapter;
```

`src/lib.rs` should expose both canonical and compatibility paths:

```rust
pub mod entrypoints;

pub use entrypoints::api;
pub use entrypoints::cli;
pub use entrypoints::cli_adapter;
```

Update `src/main.rs` only if needed. Existing `exagent::cli` and `exagent::api` should continue to work through the re-exports.

**Verification:**

```bash
cargo check
```

Expected: errors only from modules not moved yet, if the import guard already references future modules.

## Task 3: Move State Modules Under `state/`

**Files:**
- Move: `src/session.rs` -> `src/state/session.rs`
- Move: `src/events.rs` -> `src/state/events.rs`
- Move: `src/transcript.rs` -> `src/state/transcript.rs`
- Create: `src/state/mod.rs`
- Modify: `src/lib.rs`
- Modify imports as required.

**Implementation:**

`src/state/mod.rs`:

```rust
pub mod events;
pub mod session;
pub mod transcript;
```

`src/lib.rs` compatibility:

```rust
pub mod state;

pub use state::events;
pub use state::session;
pub use state::transcript;
```

Prefer updating internal imports to canonical paths where they clarify ownership:

```rust
use crate::state::session::SessionSnapshot;
use crate::state::events::RuntimeEvent;
```

Do not chase every compatibility import if it creates unnecessary churn; compatibility exports are acceptable during this refactor.

**Verification:**

```bash
cargo check
```

Expected: compile or show only remaining unmoved module path issues.

## Task 4: Move Model Modules Under `model/`

**Files:**
- Move: `src/llm.rs` -> `src/model/llm.rs`
- Move: `src/types.rs` -> `src/model/types.rs`
- Create: `src/model/mod.rs`
- Modify: `src/lib.rs`
- Modify imports as required.

**Implementation:**

`src/model/mod.rs`:

```rust
pub mod llm;
pub mod types;
```

`src/lib.rs` compatibility:

```rust
pub mod model;

pub use model::llm;
pub use model::types;
```

Keep `crate::types::*` working through the re-export for now. The canonical long-term path is `crate::model::types::*`.

**Verification:**

```bash
cargo check
```

Expected: compile or show only remaining unmoved module path issues.

## Task 5: Move Runtime Support Modules Under `runtime/`

**Files:**
- Move: `src/agent.rs` -> `src/runtime/agent.rs`
- Move: `src/exec_session.rs` -> `src/runtime/exec_session.rs`
- Move: `src/policy.rs` -> `src/runtime/policy.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/lib.rs`
- Modify imports as required.

**Implementation:**

`src/runtime/mod.rs` should include:

```rust
pub mod agent;
pub mod exec_session;
pub mod policy;
pub mod thread_runtime;
pub(crate) mod tool_call_runtime;
pub mod thread_session;
```

`src/lib.rs` compatibility:

```rust
pub use runtime::agent;
pub use runtime::exec_session;
pub use runtime::policy;
```

The canonical path for internal runtime code should become:

```rust
use crate::runtime::agent::Agent;
use crate::runtime::exec_session::ExecSessionManager;
use crate::runtime::policy::PolicyManager;
```

**Verification:**

```bash
cargo check
```

Expected: compile or show only remaining tool registry issues.

## Task 6: Move Tool Registry Under `tools/`

**Files:**
- Move: `src/registry.rs` -> `src/tools/registry.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/lib.rs`
- Modify imports as required.

**Implementation:**

`src/tools/mod.rs` should include:

```rust
pub mod read_file;
pub mod registry;
pub mod run_command;
pub mod write_file;
```

`src/lib.rs` compatibility:

```rust
pub use tools::registry;
```

Update tool modules to use:

```rust
use crate::tools::registry::ToolContext;
```

Update `default_tool_registry` return type and construction to use the re-export or canonical path consistently.

**Verification:**

```bash
cargo check
```

Expected: pass.

## Task 7: Update Docs And README Architecture

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture/2026-05-18-exagent-session-centered-agent-architecture.md`
- Optionally modify: `docs/plans/2026-05-18-exagent-session-centered-agent-runtime-refactor.md`

**Implementation:**

Update the README architecture diagram from the older:

```text
Session -> legacy Agent live-turn runner
```

to:

```text
Session -> Agent.sample_assistant_turn
Session -> ToolCallRuntime
ToolCallRuntime -> ToolRegistry
ToolRegistry -> Tools
```

Add a short repository layout section that names:

- `entrypoints/`
- `app_server/`
- `runtime/`
- `tools/`
- `state/`
- `model/`

Keep the docs focused on structure and invariants, not a full design rewrite.

**Verification:**

```bash
rg "legacy Agent live-turn runner|legacy live-turn runner" README.md docs/architecture docs/plans
```

Expected: no stale references except historical notes explicitly marked as previous architecture.

## Task 8: Full Verification

**Files:**
- No new code edits unless failures identify necessary fixes.

**Commands:**

```bash
cargo fmt -- --check
cargo check
cargo test
cargo test --test module_layout -- --nocapture
cargo test --test architecture_guards -- --nocapture
git diff --check
```

Expected: all pass.

Also run targeted architecture searches:

```bash
rg "pub mod agent;|pub mod registry;|pub mod session;|pub mod events;|pub mod transcript;|pub mod llm;|pub mod types;|pub mod exec_session;|pub mod policy;" src/lib.rs
```

Expected: old top-level `pub mod ...;` declarations should not remain for rehomed files. Compatibility should be through `pub use`, not duplicate module declarations.

## Task 9: Final Review

**Files:**
- Review all changed files.

**Checklist:**

- The diff is mostly file moves/import updates.
- No behavior logic changed in `turn.rs`, `thread_runtime.rs`, tool implementations, or protocol structs except import paths.
- `lib.rs` clearly separates canonical modules from compatibility re-exports.
- The new structure maps cleanly to the architecture:
  - `entrypoints`: user I/O adapters
  - `app_server`: external typed boundary
  - `runtime`: live execution kernel
  - `tools`: tool definitions and dispatch
  - `state`: durable facts
  - `model`: LLM adapter and message/tool-call model

**Commit checkpoint:**

Only commit if the user asks for commits in this workspace. Suggested commit message:

```bash
git commit -m "refactor: rehome modules by architecture layer"
```
