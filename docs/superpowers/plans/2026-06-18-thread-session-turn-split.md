# Thread Session Turn Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `src/runtime/thread_session/turn.rs` into focused `turn/` submodules without changing runtime behavior.

**Architecture:** Keep `ThreadSession` and shared context state in `thread_session/mod.rs` and `runtime/context.rs`. Move only code already inside `turn.rs` into child modules under `thread_session/turn/`, so Rust module privacy remains narrow and no external runtime modules are pulled into thread session.

**Tech Stack:** Rust, Tokio, existing ExAgent runtime tests.

---

## File Structure

- Delete: `src/runtime/thread_session/turn.rs`
- Create: `src/runtime/thread_session/turn/mod.rs`
- Create: `src/runtime/thread_session/turn/sampling.rs`
- Create: `src/runtime/thread_session/turn/context_start.rs`
- Create: `src/runtime/thread_session/turn/compaction_flow.rs`
- Create: `src/runtime/thread_session/turn/goal_effects.rs`
- Create: `src/runtime/thread_session/turn/external_input.rs`
- Create: `src/runtime/thread_session/turn/recording.rs`
- Create: `src/runtime/thread_session/turn/turn_config.rs`
- Create: `src/runtime/thread_session/turn/tests.rs`

## Boundaries

- [x] Move only code already present in `turn.rs`.
- [x] Keep `src/runtime/context.rs` at runtime level.
- [x] Keep `src/runtime/compaction.rs` as the compaction engine.
- [x] Keep `src/runtime/goal/runtime.rs` as the goal runtime engine.
- [x] Keep tests together in `turn/tests.rs`.
- [x] Do not introduce a new turn runner abstraction in this pass.

## Tasks

- [x] Create the `turn/` module folder and route submodules through `turn/mod.rs`.
- [x] Move sampling loop code into `sampling.rs`.
- [x] Move turn-start context preparation into `context_start.rs`.
- [x] Move turn-local compaction flow into `compaction_flow.rs`.
- [x] Move goal effect/report helpers into `goal_effects.rs`.
- [x] Move approval and user-input response handling into `external_input.rs`.
- [x] Move rollout/event recording helpers into `recording.rs`.
- [x] Move per-turn config/profile helpers into `turn_config.rs`.
- [x] Move tests unchanged into `tests.rs`.
- [x] Run `cargo fmt --all -- --check`.
- [x] Run `cargo test -p exagent --lib runtime::thread_session::turn::tests`.
- [x] Run `cargo test --test thread_runtime`.
- [x] Run `cargo test --test architecture_guards`.
