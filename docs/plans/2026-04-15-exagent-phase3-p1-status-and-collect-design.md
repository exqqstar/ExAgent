# ExAgent Phase 3 P1 Status And Collect Design

**Date:** 2026-04-15  
**Status:** Draft for P1 implementation  
**Baseline:** `main` at `1e691ae` includes Phase 3 P0 thin orchestration

## Baseline Summary

Phase 3 P0 already provides:

- parent/child session lineage in `SessionSnapshot`
- `AgentRole`
- replayable `SessionSpawned` events
- `Agent::fork_session(...)`
- thin CLI/API fork entrypoints
- parent/child artifact isolation and replay regressions

What P0 does **not** provide is any operator-facing read surface for child work after spawn. A lead session can fork child sessions, but it cannot ask:

- which child sessions belong to this parent
- what role each child has
- whether a child looks completed, blocked on approval, or still running an exec session
- what the latest useful child output is

That is the gap P1 must close.

## P1 Goal

Add the thinnest useful read-only management surface so a lead operator can inspect direct child sessions and collect a stable latest-output view without rerunning tools or introducing planner behavior.

## Approaches

### Option A: Derive inspect/collect views from parent replay plus child snapshots/events

Read the parent session's replayed `SessionSpawned` events to discover direct children in stable spawn order, then derive child lifecycle/output views from each child snapshot plus persisted runtime events.

Pros:

- reuses the P0 persistence contract
- keeps orchestration read-side thin
- avoids new write-path coupling
- preserves stable child ordering from replay
- matches the existing replay-first architecture

Cons:

- still requires opening each child snapshot/events file
- derived status is intentionally approximate rather than scheduler-grade

### Option B: Maintain an explicit parent-child manifest on spawn

Persist a dedicated child index under the parent session directory.

Pros:

- faster lookup
- simpler read path

Cons:

- adds new write-path consistency surface
- duplicates lineage that already exists in snapshots and events
- too heavy for P1

### Option C: Reconstruct everything only from parent event replay

Pros:

- strong replay alignment

Cons:

- still needs child snapshot reads for status/output, so replay alone is not enough
- awkward for direct parent-child filtering and lifecycle derivation

## Recommendation

Choose **Option A** for P1.

P1 should stay read-only and derive its management views from the persistence contract established in P0. That keeps the implementation thin, avoids introducing a second orchestration index, and gives us a clean path to a richer read model later if performance or scale ever require it.

## P1 Goals

1. Let a lead inspect the direct child sessions of a parent session.
2. Expose each child session's identity and orchestration metadata: `session_id`, `parent_session_id`, `root_session_id`, `agent_role`, and `spawned_by_turn_id`.
3. Expose a basic derived lifecycle status for each child session using persisted snapshot state.
4. Expose a stable "latest useful output" view for child sessions without rerunning tools.
5. Surface the above through thin CLI/API read-only entrypoints.
6. Keep the implementation replay-friendly and non-invasive to the P0 execution kernel.

## Non-Goals

1. Do not add planner behavior or task decomposition.
2. Do not add mailbox/actor orchestration.
3. Do not add child control actions such as cancel, retry, resume, or re-fork.
4. Do not add branch/worktree automation.
5. Do not add reduce/join semantics across many child results.
6. Do not add a durable parent-child manifest or scheduler-specific runtime state.

## Proposed Read Model

P1 should add a derived read-side model with two operator views:

- `inspect_children(parent_session_id)`
  Returns one summary per direct child, including lineage metadata, status, snapshot path, and events path.
- `collect_session(session_id)`
  Returns one child summary plus a stable `latest_useful_output` payload for that child session.

Recommended derived status for P1:

- `waiting_approval` when `pending_approvals` is non-empty
- `running` when `open_exec_sessions` is non-empty and approvals are empty
- `completed` otherwise

Recommended latest output rule:

1. Prefer the most recent assistant message with non-empty text.
2. Otherwise fall back to the most recent persisted `ToolResult`.
3. Otherwise return no collected output.

This keeps P1 deterministic and fully derived from persisted state.

## CLI/API Surface

Recommended CLI:

- `inspect <parent_session_id>`
- `collect <session_id>`

Recommended API:

- `POST /inspect`
- `POST /collect`

Both surfaces should be read-only and should not mutate snapshots, rerun tools, or require LLM access.

## Acceptance Criteria

1. A parent session can list its direct children through CLI/API.
2. Each child summary includes lineage fields and role metadata.
3. Each child summary includes a derived lifecycle status.
4. `collect` returns a stable latest useful output for each child session.
5. `inspect` and `collect` do not mutate session artifacts.
6. `inspect` and `collect` only return direct children of the requested parent.
7. Existing Phase 3 P0 replay/isolation behavior stays green.

## Risks

1. "Latest useful output" can become ambiguous if we do not define a deterministic precedence rule.
2. Status derivation can overreach if we attempt to model more than persisted runtime state supports.
3. It is easy to accidentally return descendants instead of direct children if filtering uses `root_session_id` instead of `parent_session_id`.
4. API surface can become noisy if P1 tries to add control actions together with read-only introspection.

## Workflow Tasks For This Session

- `Spec`: lock the P1 goals, non-goals, acceptance criteria, and response shapes.
- `Test`: define regression risks, test matrix, and direct-child/latest-output invariants.
- `Judge`: challenge the draft for scope creep, weak status semantics, and accidental planner behavior.
