# ExAgent Phase 3 Code Reading And Test Map

**Date:** 2026-04-15  
**Status:** Detailed code and test walkthrough for current Phase 3  
**Audience:** Readers who want to move from concepts into concrete source files

## 1. Fastest Reading Order

If your goal is understanding rather than immediate modification, use this order:

1. `docs/plans/2026-04-15-exagent-phase3-roadmap-and-working-model-design.md`
2. `src/session.rs`
3. `src/events.rs`
4. `src/transcript.rs`
5. `src/agent.rs`
6. `src/orchestration.rs`
7. `src/result_contract.rs`
8. `src/tools/record_structured_result.rs`
9. `src/api.rs`
10. `tests/orchestration.rs`
11. `tests/structured_contracts.rs`
12. `tests/api_server.rs`
13. `tests/resume.rs`

That order follows the actual dependency direction:

- concepts first
- persistence contracts next
- runtime flow next
- read-side behavior next
- tests last so assertions make sense

## 2. `src/session.rs`

This file is the root of the Phase 3 data model.

Key things to learn here:

- `AgentRole`
- lineage fields on `SessionSnapshot`
- `fork_child(...)`
- normalization of root lineage

Questions this file answers:

- what is a parent vs child session
- how does a child inherit root lineage
- where is role metadata stored

When reading, pay attention to:

- `parent_session_id`
- `root_session_id`
- `spawned_by_turn_id`
- `agent_role`

These four fields are the minimal orchestration identity layer.

## 3. `src/events.rs`

This file defines the replayable orchestration history.

For Phase 3, the most important event kinds are:

- `SessionSpawned`
- `StructuredResultRecorded`

You should mentally map them to:

- topology event
- typed handoff event

Questions this file answers:

- what facts are replayable
- which orchestration transitions are first-class events
- which parts of Phase 3 are append-only instead of snapshot-only

## 4. `src/transcript.rs`

This file is where Phase 3 becomes operational.

Important helpers:

- `session_paths(...)`
- `read_session_snapshot(...)`
- `read_session_events(...)`
- `direct_child_session_ids(...)`
- `latest_structured_result(...)`
- `record_session_spawn(...)`
- `record_structured_result(...)`

Questions this file answers:

- where session artifacts live on disk
- how direct children are discovered
- how structured results are persisted and recovered

If you understand this file, you understand most of the persistence story.

## 5. `src/agent.rs`

This file still contains the single-session kernel.

For Phase 3, focus on:

- `fork_session(...)`
- `inspect_children(...)`
- `collect_session(...)`
- `run_session(...)`

What to notice:

- orchestration read methods are thin wrappers
- fork reuses the same runtime kernel
- tool execution now carries `turn_id` in `ToolContext`

That last detail matters because P2 structured results capture `source_turn_id`.

## 6. `src/orchestration.rs`

This is the read-side orchestration layer.

Important types:

- `ChildLifecycleStatus`
- `ChildSessionSummary`
- `CollectedOutput`
- `CollectedChildSession`

Important functions:

- `inspect_children(...)`
- `collect_session(...)`

Questions this file answers:

- how inspect is derived from parent events plus child snapshots
- how collect merges child summary, structured result, and legacy output
- where lifecycle status is derived

This file is the best place to understand the current lead-facing contract.

## 7. `src/result_contract.rs`

This file is the entire P2 semantic contract.

Important items:

- `STRUCTURED_RESULT_SCHEMA_VERSION`
- `StructuredSessionResult`
- `StructuredResultPayload`
- `JudgeRecommendation`
- `validate_role(...)`

Questions this file answers:

- what a typed review artifact looks like
- what fields are common vs role-specific
- what makes a payload valid

If you need to change `spec/test/judge` payload shapes later, this is the first file to open.

## 8. `src/tools/record_structured_result.rs`

This file is the narrowest P2 write path.

Important behaviors:

- parse tool args
- convert payload args to canonical payload
- read the current session snapshot
- enforce role restrictions
- append `StructuredResultRecorded`

Questions this file answers:

- how does a child session publish typed output
- why can only `spec/test/judge` do it
- how does `source_turn_id` get attached

## 9. `src/api.rs`

This file exposes the operator-facing surfaces.

Relevant paths:

- `/fork`
- `/inspect`
- `/collect`

Key idea:

- API is thin glue over the same runtime and orchestration read-side
- collect response mirrors Rust data structures rather than inventing a second contract

That is why API tests are important: they guard response-shape drift.

## 10. Test Map Overview

The tests are not random. They divide cleanly by responsibility.

### `tests/orchestration.rs`

Best for learning:

- lineage round-trip
- spawn replay
- sibling isolation
- inspect direct-child semantics
- collect latest useful output semantics

Read this file when you want to know the intended orchestration behavior.

### `tests/structured_contracts.rs`

Best for learning:

- structured result serde round-trip
- event round-trip
- persistence helper behavior
- tool success path
- role mismatch rejection
- collect integration with structured result

Read this file when you want to know the intended P2 contract behavior.

### `tests/api_server.rs`

Best for learning:

- CLI parsing
- API request/response shapes
- collect serialization with and without `structured_result`

Read this file when you want to know the external operator surface.

### `tests/resume.rs`

Best for learning:

- session reuse semantics
- replay after resume
- latest structured result wins after resume

Read this file when you want to understand append-only behavior across multiple turns and resumed sessions.

## 11. Reading Strategy By Question

If your question is:

- “How does child lineage work?”
  Read `src/session.rs`, `src/events.rs`, `tests/orchestration.rs`
- “How does inspect discover children?”
  Read `src/transcript.rs`, `src/orchestration.rs`, `tests/orchestration.rs`
- “How does collect choose what to return?”
  Read `src/orchestration.rs`, `tests/orchestration.rs`, `tests/api_server.rs`
- “How does typed review output work?”
  Read `src/result_contract.rs`, `src/tools/record_structured_result.rs`, `tests/structured_contracts.rs`
- “How does resume affect typed results?”
  Read `src/agent.rs`, `tests/resume.rs`

This question-driven reading order is often faster than walking every file top to bottom.

## 12. Suggested Modification Order For Future Work

If you later extend this system, the safest modification order is usually:

1. contract types
2. events
3. transcript persistence helpers
4. orchestration read-side
5. tool write path
6. API surface
7. tests

That order keeps source-of-truth changes ahead of operator-surface changes.

## 13. The Most Important Invariants To Keep In Mind

These invariants are the real “do not break” list:

- child sessions are isolated on disk
- direct-child discovery comes from parent replay order
- inspect stays topology/status only
- collect works on one child session at a time
- structured result is additive, not a replacement for legacy output
- latest structured result wins after resume
- role mismatch must not persist invalid typed results

When reading tests, you will see these invariants repeated over and over in different forms.

## 14. Final Advice For Learning

Do not try to memorize every function first.

Instead, learn the system in this order:

1. what problem each milestone solved
2. what file owns which contract
3. what event or snapshot field proves that contract
4. which test locks it in

If you can answer those four questions for P0, P1, and P2, then you already understand the current Phase 3 baseline at a high level.
