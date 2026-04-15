# ExAgent Phase 3 P2 Structured Review And Result Contracts Implementation Plan

**Goal:** Add a replayable structured result contract for `spec`, `test`, and `judge` sessions, then surface it through the existing collect path without broadening inspect or introducing planner behavior.

**Architecture:** Keep `Agent::run_session(...)` as the single-session kernel. Introduce a small typed result-contract module, persist structured results as first-class runtime events, and extend the orchestration read model so `collect` returns typed result data when present. The current runtime has no existing terminal result-append hook beyond assistant turns and tool results, so a narrow tool is the minimal fallback write path. Do not add new operator write endpoints.

**Tech Stack:** Rust, Tokio, Serde, Serde JSON, Schemars, Anyhow, Axum

## Current Baseline

- `src/orchestration.rs` exposes `inspect_children(...)` and `collect_session(...)`.
- `collect_session(...)` currently returns only legacy output via `latest_useful_output`.
- `src/events.rs` has no first-class structured-result event.
- the default tool registry exposes only file and command tools.
- P1 tests already cover inspect ordering, collect fallback, replay, resume, and isolation.

## Immediate Scope

This plan covers `P2: Structured Review And Result Contracts`. The milestone is complete when ExAgent can:

1. let `spec`, `test`, and `judge` sessions publish versioned structured results
2. persist those results in replayable session events
3. expose the latest structured result through `collect(session_id)`
4. reject role-mismatched structured results deterministically
5. keep P1 inspect/collect behavior compatible for legacy sessions

## Non-Goals For P2

Do not expand into these areas before this milestone is green:

- planner or task decomposition behavior
- mailbox/actor orchestration
- operator write endpoints for result submission
- reduce/join semantics across multiple children
- structured outputs for `primary` or `implementation`
- persistence redesign beyond the existing event log and session artifacts

### Task 1: Add typed structured-result models and event variants

**Files:**
- Create: `src/result_contract.rs`
- Modify: `src/events.rs`
- Modify: `src/lib.rs`
- Test: `tests/structured_contracts.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- a structured result envelope round-trips through serde
- role-specific payload variants for `spec`, `test`, and `judge` round-trip cleanly
- `StructuredResultRecorded` round-trips as a runtime event

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test structured_contracts structured_result_event_round_trips -- --exact
```

Expected: FAIL because the structured result module and event do not exist yet.

**Step 3: Write the minimal implementation**

Add:

- `StructuredSessionResult`
- a role-specific payload enum
- a judge recommendation enum
- a `StructuredResultRecorded` event variant

Implementation rules:

- keep the envelope versioned
- keep role and payload kind explicit
- do not widen the contract to `primary` or `implementation`

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test structured_contracts structured_result_event_round_trips -- --exact
```

Expected: PASS

### Task 2: Add event-backed persistence helpers for structured results

**Files:**
- Modify: `src/transcript.rs`
- Modify: `tests/structured_contracts.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- structured results can be appended to a child session event log
- replay from disk returns the persisted structured result
- when multiple structured results exist, the latest persisted one wins

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test structured_contracts transcript_replays_latest_structured_result -- --exact
```

Expected: FAIL because transcript has no structured-result helpers yet.

**Step 3: Write the minimal implementation**

Add transcript helpers such as:

- `record_structured_result(...)`
- `latest_structured_result(...)`

Implementation rules:

- structured review output should be sourced from the event log
- do not rewrite snapshots during read-side extraction
- use last persisted structured-result event as the canonical value

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test structured_contracts transcript_replays_latest_structured_result -- --exact
```

Expected: PASS

### Task 3: Add the `record_structured_result` tool and role validation

**Files:**
- Create: `src/tools/record_structured_result.rs`
- Modify: `src/tools/mod.rs`
- Modify: `src/registry.rs`
- Modify: `src/agent.rs`
- Modify: `src/lib.rs`
- Test: `tests/structured_contracts.rs`
- Test: `tests/agent_loop.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- a `spec`/`test`/`judge` child session can record a structured result through the tool
- the tool captures `source_turn_id` from runtime context
- role mismatches fail and do not persist a structured result

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test structured_contracts tool_records_structured_result_for_matching_role -- --exact
```

Expected: FAIL because no such tool or role validation exists yet.

**Step 3: Write the minimal implementation**

Add:

- a tool input schema with common envelope fields plus a role-specific payload
- validation against the session snapshot's `agent_role`
- runtime context support for the current `turn_id`
- registration in the default tool registry

Implementation rules:

- tool writes only to the current session
- role and payload kind must match
- `primary` and `implementation` cannot publish a P2 structured review result

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test structured_contracts tool_records_structured_result_for_matching_role -- --exact
```

Expected: PASS

### Task 4: Extend collect to expose structured results

**Files:**
- Modify: `src/orchestration.rs`
- Modify: `src/agent.rs`
- Modify: `src/api.rs`
- Modify: `tests/orchestration.rs`
- Modify: `tests/api_server.rs`
- Modify: `tests/resume.rs`

**Step 1: Write the failing tests**

Add tests that assert:

- `collect(session_id)` returns `structured_result` when present
- legacy sessions without structured result still return only `latest_useful_output`
- API collect responses serialize `structured_result` with the same JSON shape as the Rust model
- after resume, the latest structured result wins deterministically

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test orchestration collect_returns_structured_result_when_present -- --exact
cargo test --test api_server collect_route_serializes_structured_result -- --exact
```

Expected: FAIL because `collect` currently exposes only legacy output.

**Step 3: Write the minimal implementation**

Add:

- optional `structured_result` to `CollectedChildSession`
- orchestration logic to read the latest structured result from transcript events
- API response support through the existing collect surface

Implementation rules:

- keep `inspect` unchanged
- keep `latest_useful_output` behavior compatible for P1 callers
- freeze the collect shape: `structured_result` is additive and `latest_useful_output` remains present for backward compatibility
- require CLI/API parity for collect serialization
- make the precedence rule explicit: lead consumers should prefer `structured_result` when present, but legacy output remains available

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --test orchestration collect_returns_structured_result_when_present -- --exact
cargo test --test api_server collect_route_serializes_structured_result -- --exact
```

Expected: PASS

### Task 5: Add P2 regression coverage and full verification

**Files:**
- Modify: `tests/structured_contracts.rs`
- Modify: `tests/orchestration.rs`
- Modify: `tests/api_server.rs`
- Modify: `tests/resume.rs`

**Step 1: Write the failing tests**

Add regression coverage for:

- result replay after restart
- last-write-wins semantics after resume
- role mismatch produces a tool error and no persisted result
- P1 legacy collect behavior remains stable without structured result
- API/CLI parity for collect serialization

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test structured_contracts
cargo test --test orchestration
cargo test --test api_server
cargo test --test resume
```

Expected: at least one P2 regression FAILS until the new code is in place.

**Step 3: Write the minimal implementation**

If any regression reveals missing glue, add only the smallest supporting code needed for:

- deterministic structured-result persistence
- collect integration
- role validation
- replay-safe precedence

Keep this task focused on correctness, not feature expansion.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test
```

Expected: PASS

## P2 Scope Guardrails

Do not claim P2 is complete until these statements are true:

- `spec`, `test`, and `judge` sessions can publish typed structured results
- `collect(session_id)` returns that structured result when present
- `inspect(parent_session_id)` remains a topology/status surface
- replay and resume preserve the latest structured result deterministically
- legacy P1 collect behavior still works when structured results are absent
- no planner or operator write-control behavior has been added

## Recommended Ownership Split

- Implementation A: `src/result_contract.rs`, `src/events.rs`, `src/transcript.rs`, `tests/structured_contracts.rs`
- Implementation B: `src/tools/record_structured_result.rs`, `src/registry.rs`, `src/agent.rs`, `src/orchestration.rs`, `src/api.rs`, regression tests

## Execution Note

Use the existing `Lead + Spec + Test -> Lead Synthesis -> Judge -> Implementation` workflow. The P2 contract should stay narrow enough that it feels like a typed handoff layer, not an orchestration planner.
