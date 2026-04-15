# ExAgent Phase 3 P2 Structured Review And Result Contracts Design

**Date:** 2026-04-15  
**Status:** Draft for P2 implementation  
**Baseline:** `main` at `af4a093` includes Phase 3 P1 status and collect

## Baseline Summary

Phase 3 P1 already provides:

- parent/child lineage and replayable spawn events
- `inspect` for direct child topology and lifecycle status
- `collect` for a child session's latest useful legacy output
- thin CLI/API read surfaces for inspect/collect
- sibling isolation and replay regressions

What P1 does **not** provide is a stable machine-readable payload for non-writer roles. A lead can collect child text, but it still has to parse free-form prose from `spec`, `test`, or `judge` sessions. That keeps the workflow human-readable, but not contract-driven.

That is the gap P2 must close.

## P2 Goal

Add a minimal, replayable structured result contract for `spec`, `test`, and `judge` sessions so a lead can consume typed outputs through the existing collect surface without introducing planner behavior or a broader control plane.

## Approaches

### Option A: Event-backed structured result written by an agent tool

Add a narrow `record_structured_result` tool. A `spec`, `test`, or `judge` session can call it to persist a typed result envelope into its own event log. `collect` then reads the latest structured result event and returns it alongside the existing legacy output view.

Pros:

- makes the result explicit instead of inferred
- keeps the contract replayable from persisted events
- avoids introducing new operator write endpoints
- lets child sessions publish their own handoff artifact

Cons:

- adds one new write tool
- requires role validation and schema checks

### Option B: External operator submission through CLI/API

Add a manual `record_result` CLI/API surface and let the lead or operator persist structured payloads after child completion.

Pros:

- avoids adding an LLM-facing tool
- easy to reason about operationally

Cons:

- shifts the result contract away from the child session itself
- adds operator write surface to a milestone that should stay thin
- makes automation later more awkward

### Option C: Infer structured payloads from assistant text

Parse JSON or fenced sections from free-form assistant output and treat that as the structured result.

Pros:

- no new persistence event or tool

Cons:

- fragile and nondeterministic
- hard to validate across resume/replay
- exactly the kind of heuristic contract P2 should avoid

## Recommendation

Choose **Option A** for P2, but only after making one constraint explicit: the current runtime has no existing terminal session-result append path beyond assistant turns and tool results. There is no thinner built-in completion hook that can persist a child-owned typed result today.

Given that constraint, a narrow LLM-facing tool is the smallest correct write path. It keeps `inspect` and `collect` operator-facing and read-only, avoids new operator write endpoints, and gives `collect` a deterministic event-backed source of truth. In other words, the tool is not the default preference for P2; it is the minimal fallback required by the current runtime contract.

## P2 Goals

1. Add a versioned structured result envelope for `spec`, `test`, and `judge` sessions.
2. Make that result replayable from the session event log.
3. Let `collect(session_id)` return the structured result when present, without breaking the P1 legacy output view.
4. Preserve `inspect` as topology/status only.
5. Keep `primary` and `implementation` sessions out of scope for structured result publishing in P2.
6. Keep the implementation thin and compatible with the P1 persistence/read model.

## Non-Goals

1. Do not add planner or task-graph behavior.
2. Do not add mailbox or actor-style orchestration.
3. Do not add join/reduce semantics across many children.
4. Do not add operator-side control actions such as retry, cancel, or resume from inspect/collect.
5. Do not redesign session persistence around manifests or external indexes.
6. Do not require every role to emit structured output in P2.

## Proposed Contract

P2 should add a typed envelope with common metadata:

- `schema_version`
- `agent_role`
- `session_id`
- `parent_session_id`
- `source_turn_id`
- `summary`
- `assumptions`
- `risks`
- `open_questions`

P2 should then define role-specific payloads:

- `spec`
  - `goals`
  - `non_goals`
  - `acceptance_criteria`
  - `contract_boundaries`
- `test`
  - `regression_risks`
  - `test_matrix`
  - `coverage_gaps`
- `judge`
  - `scope_issues`
  - `missing_criteria`
  - `blockers`
  - `recommendation`

The session role and the payload kind must match. A `judge` session cannot write a `spec` result, and a `primary` or `implementation` session cannot publish a P2 structured review result.

## Persistence And Read Model

Recommended persistence source:

- append `StructuredResultRecorded { result }` to the child session event log

Recommended read model:

- `inspect(parent_session_id)` remains unchanged
- `collect(session_id)` keeps `latest_useful_output`
- `collect(session_id)` additionally returns `structured_result` when the child has recorded one
- `structured_result` is an additive field on the existing collect envelope, not a replacement response shape
- CLI and API must serialize the same collect envelope
- when multiple structured results exist for the same session, the latest persisted event wins

This keeps the contract replay-first and avoids writing derived fields back into snapshots.

## CLI/API Surface

P2 should **not** add new operator-facing write endpoints by default.

Recommended operator surface:

- keep existing `inspect` unchanged
- extend the existing `collect` response shape with optional `structured_result`
- keep the same collect JSON envelope across Rust, CLI, and API serialization

Recommended agent-facing surface:

- add `record_structured_result` to the default tool registry

## Acceptance Criteria

1. A `spec`, `test`, or `judge` session can persist one structured result through a narrow typed contract.
2. The persisted result round-trips through event replay after restart.
3. `collect` returns the structured result when present and still returns legacy output when absent.
4. `inspect` remains a topology/status surface and does not become a review container.
5. Role mismatch attempts fail deterministically and do not persist invalid structured results.
6. Existing P1 inspect/collect behavior stays compatible for sessions without structured payloads.
7. P0/P1 fork, resume, replay, and isolation regressions stay green.

## Risks

1. If the result schema is too broad, P2 will start looking like a planner contract.
2. If `collect` merges structured and legacy output ambiguously, lead consumption becomes unstable.
3. If the tool can write results for the wrong role, the contract loses meaning.
4. If P2 writes derived read-side data back into snapshots, read-only guarantees from P1 will blur.

## Workflow Tasks For This Session

- `Spec`: lock goals, non-goals, envelope shape, and scope boundaries
- `Test`: lock precedence, replay, resume, and role-mismatch regressions
- `Judge`: challenge whether the tool write path is the thinnest acceptable way to make contracts real
