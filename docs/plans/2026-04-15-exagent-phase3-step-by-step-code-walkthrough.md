# ExAgent Phase 3 Step-By-Step Code Walkthrough

**Date:** 2026-04-15  
**Status:** Detailed study tutorial after Phase 3 `P0`, `P1`, and `P2`  
**Audience:** Developers who want to learn the current Phase 3 runtime by reading the code in the right order instead of jumping between files

## 1. How To Use This Tutorial

This file is intentionally narrower than the other learning docs.

It does not try to explain every boundary again. Instead, it walks through one concrete runtime story:

1. a lead session forks a `spec` child
2. the child runs with its own session snapshot
3. the child records a typed structured result
4. the lead later `inspect`s and `collect`s that child

Use this tutorial with the other docs:

- start with [Phase 3 Current-State Learning Guide](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-current-state-learning-guide.md:1) if you want the big picture
- keep [Phase 3 Runtime Flows And Persistence Guide](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-runtime-flows-and-persistence-guide.md:1) nearby if you want a deeper persistence explanation
- use [Phase 3 Code Reading And Test Map](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-code-reading-and-test-map.md:1) when you want a wider file map after this walkthrough

## 2. The Example You Should Hold In Your Head

As you read, keep this example fixed:

- parent session: `session_parent`
- child session: `session_spec`
- child role: `spec`
- spawn turn: `turn_1`
- child eventually records one `StructuredResultRecorded` event
- parent later asks:
  - `inspect(parent_session_id)` to discover the child
  - `collect(session_id)` to read the child summary and typed result

If you hold that one story in your head, the Phase 3 code becomes much easier to follow.

## 3. Suggested Study Setup

Open this tutorial in one pane and run the listed commands in another pane.

Use these commands as you go:

```bash
sed -n '100,177p' src/session.rs
sed -n '15,58p' src/events.rs
sed -n '79,212p' src/transcript.rs
sed -n '108,154p' src/agent.rs
sed -n '146,185p' src/agent.rs
sed -n '1,160p' src/orchestration.rs
sed -n '1,84p' src/result_contract.rs
sed -n '1,183p' src/tools/record_structured_result.rs
```

Then keep these tests open for validation:

```bash
sed -n '413,539p' tests/orchestration.rs
sed -n '542,645p' tests/orchestration.rs
sed -n '87,154p' tests/structured_contracts.rs
sed -n '212,260p' tests/structured_contracts.rs
sed -n '343,409p' tests/resume.rs
sed -n '323,510p' tests/api_server.rs
```

## 4. Step 1: Read The Core Session Shape

Read: `src/session.rs:100-177`

Focus on four groups of fields inside `SessionSnapshot`:

- identity: `session_id`
- lineage: `parent_session_id`, `root_session_id`, `spawned_by_turn_id`
- role: `agent_role`
- runtime state: `workspace_root`, `cwd`, `conversation`, `open_exec_sessions`, `pending_approvals`

What to notice:

- `new_root(...)` in `src/session.rs:123-142` creates the initial root session and sets `agent_role` to `Primary`
- `fork_child(...)` in `src/session.rs:144-163` creates a new `session_id`, points `parent_session_id` back to the parent, preserves the effective root, copies `workspace_root` and `cwd`, and resets runtime-only state
- `normalize_lineage()` and `effective_root_session_id()` in `src/session.rs:165-177` protect replay compatibility by ensuring `root_session_id` is always populated

Why this matters:

- P0 did not add a new runtime object for children; it extended the existing durable session snapshot so every child is just another session with lineage metadata
- this is why the rest of Phase 3 can remain thin: the system reuses the existing session persistence model

Self-check:

- Why is `root_session_id` copied from the parent instead of set to the child id?
- Why does `fork_child(...)` carry over `cwd` but drop `open_exec_sessions`?

## 5. Step 2: Read The Event Vocabulary

Read: `src/events.rs:15-58`

This enum is the event log contract. Do not skim it.

The most important additions for Phase 3 are:

- `SessionSpawned` in `src/events.rs:32-37`
- `StructuredResultRecorded` in `src/events.rs:56-58`

What to notice:

- the snapshot stores current durable state
- the event log stores replayable lifecycle facts
- `SessionSpawned` is the bridge that lets a parent later discover its direct children without scanning all session snapshots on disk
- `StructuredResultRecorded` is the bridge that lets `collect(session_id)` return typed role-specific output

Why this matters:

- if you only had the child snapshot, the parent would not have a stable inspect surface
- if you only had free-form assistant text, `collect` would not have a stable typed review/result contract

Self-check:

- Which Phase 3 feature depends on `SessionSpawned`?
- Which Phase 3 feature depends on `StructuredResultRecorded`?

## 6. Step 3: Read Persistence Helpers Before Agent Logic

Read: `src/transcript.rs:79-212`

Start with `session_paths(...)` in `src/transcript.rs:79-89`.

That one helper gives every session a stable directory:

- `.exagent/sessions/<session_id>/snapshot.json`
- `.exagent/sessions/<session_id>/events.jsonl`

Then read these in order:

1. `read_session_events(...)` in `src/transcript.rs:91-97`
2. `read_session_snapshot(...)` in `src/transcript.rs:99-105`
3. `direct_child_session_ids(...)` in `src/transcript.rs:107-130`
4. `latest_structured_result(...)` in `src/transcript.rs:136-147`
5. `append_runtime_event(...)` in `src/transcript.rs:149-168`
6. `record_structured_result(...)` in `src/transcript.rs:170-182`
7. `record_session_spawn(...)` in `src/transcript.rs:184-212`

What to notice in `direct_child_session_ids(...)`:

- it replays only the parent session's event log
- it filters only `SessionSpawned` events
- it only returns children whose `parent_session_id` equals the requested parent
- it deduplicates child ids

What to notice in `record_session_spawn(...)`:

- it appends a `SessionSpawned` event to the parent log
- it also appends a mirrored `SessionSpawned` event to the child log

Why that mirrored child event exists:

- the parent-side event powers `inspect`
- the child-side event keeps the child transcript self-describing for replay and debugging

Self-check:

- Why does `inspect` read parent events instead of scanning child snapshots?
- Why is `latest_structured_result(...)` implemented as a reverse scan?

## 7. Step 4: Read The Spawn Entry Point

Read: `src/agent.rs:108-154`

This is the concrete P0 entry point: `Agent::fork_session(...)`.

Walk it line by line:

1. `src/agent.rs:115-118`
   It locates and reads the parent snapshot from disk.
2. `src/agent.rs:119-120`
   It normalizes and rewrites the parent snapshot so lineage fields are clean before forking.
3. `src/agent.rs:122-123`
   It creates the child snapshot by calling `parent_snapshot.fork_child(...)`.
4. `src/agent.rs:124-130`
   It records `SessionSpawned` via transcript helpers.
5. `src/agent.rs:132`
   It immediately runs the child session with `run_session(child_snapshot)`.

What to notice:

- `fork_session(...)` does not manually assemble a child directory layout
- it does not invent a separate child-runtime abstraction
- it just produces a new durable snapshot plus replayable spawn events, then hands that snapshot to the existing session runner

That is the core design taste of Phase 3: thin orchestration on top of the existing runtime.

Self-check:

- Which line actually makes the child discoverable to `inspect` later?
- Which line actually gives the child its own durable session identity?

## 8. Step 5: Read Why Child `cwd` Actually Works

Read: `src/agent.rs:146-185`

This section matters because there was a real bug here during P0 stabilization.

Look closely at `run_session(...)`:

- `src/agent.rs:147` normalizes lineage again before execution
- `src/agent.rs:148-150` clones the base config and then overrides `workspace_root` and `cwd` from the snapshot being run
- `src/agent.rs:152-154` writes the snapshot to that session's own path
- `src/agent.rs:158-164` builds a `ToolContext` that carries the session id and later the turn id

The key idea is simple:

- the `Agent` has a base config
- the running session still gets to override the actual `workspace_root` and `cwd`
- this makes forked children execute tools in the context captured by their own snapshot, not the parent's current in-memory defaults

The regression test for this is `tests/orchestration.rs:338-410`.

Self-check:

- Why is it not enough for the child snapshot to merely store `cwd`?
- Which exact lines make that stored `cwd` operational for tools?

## 9. Step 6: Read `inspect` As A Pure Read-Side Surface

Read: `src/orchestration.rs:56-112`

This is the whole P1 inspect path.

Read it in this order:

1. `inspect_children(...)` in `src/orchestration.rs:56-64`
2. `inspect_child_session(...)` in `src/orchestration.rs:80-102`
3. `derive_lifecycle_status(...)` in `src/orchestration.rs:104-112`

What to notice:

- `inspect_children(...)` starts from `direct_child_session_ids(...)`
- `inspect_child_session(...)` then loads each child snapshot and materializes a summary
- `status` is derived from snapshot state only:
  - pending approvals -> `waiting_approval`
  - open exec sessions -> `running`
  - otherwise -> `completed`

What inspect does not do:

- it does not parse assistant text
- it does not read structured results
- it does not aggregate grandchildren
- it does not mutate any state

The best test for this shape is `tests/orchestration.rs:413-539`.

As you read that test, notice what it proves:

- only direct children of the requested parent are returned
- role and lineage fields are surfaced
- status comes from the child snapshot state
- snapshot and event paths are included

Self-check:

- Why is the grandchild excluded from `inspect(parent_session_id)`?
- Why is `inspect` topology-first rather than content-first?

## 10. Step 7: Read The Structured Result Schema Before The Tool

Read: `src/result_contract.rs:8-84`

Start with the constant:

- `STRUCTURED_RESULT_SCHEMA_VERSION` in `src/result_contract.rs:8`

Then read:

- `JudgeRecommendation` in `src/result_contract.rs:10-16`
- `StructuredResultPayload` in `src/result_contract.rs:18-38`
- `StructuredSessionResult` in `src/result_contract.rs:40-57`
- `validate_role(...)` in `src/result_contract.rs:59-83`

What to notice:

- P2 did not add an untyped blob
- it added a typed envelope with:
  - schema version
  - session identity
  - role identity
  - source turn identity
  - a role-specific payload

`validate_role(...)` is the critical guardrail:

- `Spec` payloads must come from `AgentRole::Spec`
- `Test` payloads must come from `AgentRole::Test`
- `Judge` payloads must come from `AgentRole::Judge`

Why the version matters:

- roadmap language asked for stable payloads
- a schema version gives the runtime a clean place to reject incompatible future shapes instead of silently accepting them

Self-check:

- Why is `schema_version` on the result itself instead of hidden in API docs?
- Why is `source_turn_id` useful when a child can resume multiple times?

## 11. Step 8: Read The Tool That Persists Structured Results

Read: `src/tools/record_structured_result.rs:15-183`

This file is easier if you split it into three parts:

1. argument schema
2. tool wrapper
3. persistence function

First read the input types:

- `RecordStructuredResultArgs` in `src/tools/record_structured_result.rs:15-25`
- `StructuredResultPayloadArgs` in `src/tools/record_structured_result.rs:27-47`
- the conversion impl in `src/tools/record_structured_result.rs:49-85`

Then read the tool wrapper:

- `RecordStructuredResultTool` in `src/tools/record_structured_result.rs:87-139`

Finally read the core function:

- `record_structured_result(...)` in `src/tools/record_structured_result.rs:141-183`

Walk the core function in order:

1. `src/tools/record_structured_result.rs:145-150`
   Read the runtime session id and session snapshot from disk.
2. `src/tools/record_structured_result.rs:152-160`
   Reject roles outside `spec`, `test`, and `judge`.
3. `src/tools/record_structured_result.rs:162-173`
   Build a `StructuredSessionResult` using the snapshot and tool args.
4. `src/tools/record_structured_result.rs:174`
   Validate payload kind against session role.
5. `src/tools/record_structured_result.rs:175-180`
   Persist the result as a `StructuredResultRecorded` event.

Why this design matters:

- the typed contract is still session-local
- the session role remains authoritative
- the event log stays the single replay source of truth

The best tests for this are:

- `tests/structured_contracts.rs:87-154`
- `tests/structured_contracts.rs:156-210`

Self-check:

- Why does the tool reread the session snapshot instead of trusting tool arguments for role and parent id?
- Why is persistence implemented as an event append rather than a direct snapshot mutation?

## 12. Step 9: Read `collect` As The Join Point

Read: `src/orchestration.rs:66-159`

This is the whole P1 and P2 read-side join point.

Read it in order:

1. `collect_session(...)` in `src/orchestration.rs:66-78`
2. `latest_useful_output(...)` in `src/orchestration.rs:114-138`
3. `latest_assistant_text(...)` in `src/orchestration.rs:140-148`
4. `latest_tool_result(...)` in `src/orchestration.rs:150-159`

What `collect_session(...)` returns:

- `child`: the inspect-style topology and status summary
- `structured_result`: optional typed role-specific result
- `latest_useful_output`: optional human-readable fallback output

What to notice about precedence:

- assistant text wins if present
- tool result is only used as fallback
- structured result is additive and separate

That means `collect` answers two different questions at once:

- "What is the latest human-meaningful output from this child?"
- "Did this child produce a typed result contract?"

The tests that lock this in are:

- `tests/orchestration.rs:542-645`
- `tests/structured_contracts.rs:212-260`
- `tests/resume.rs:343-409`

The resume test is especially important because it proves both surfaces stay stable across multiple child runs:

- latest structured result wins
- latest useful output still reflects the latest conversational output

Self-check:

- Why is `structured_result` not folded into `latest_useful_output`?
- Why does `collect` call `inspect_child_session(...)` first?

## 13. Step 10: Read The Public Surface Tests

After you understand the runtime internals, read the interface tests.

### Runtime Tests

Read:

- `tests/orchestration.rs:413-539`
- `tests/orchestration.rs:542-645`
- `tests/structured_contracts.rs:21-84`
- `tests/structured_contracts.rs:87-154`
- `tests/structured_contracts.rs:212-260`
- `tests/resume.rs:291-409`

What these confirm:

- spawn replay is deterministic
- inspect returns only direct children
- collect output precedence is stable
- structured results round-trip through event replay
- role mismatch does not persist invalid results
- resumed children still collect correctly

### API Tests

Read:

- `tests/api_server.rs:323-372`
- `tests/api_server.rs:376-432`
- `tests/api_server.rs:436-510`

What these confirm:

- `POST /inspect` returns the inspect summary shape
- `POST /collect` returns the collect shape
- structured results serialize over the API without changing the base collect contract

At this point, you have seen the same contract from four angles:

- snapshot fields
- runtime events
- read-side orchestration helpers
- tests and API serialization

That is enough to actually understand the current Phase 3 baseline.

## 14. One-Pass Learning Order

If you only have 30 minutes, use this exact order:

1. `src/session.rs:100-177`
2. `src/events.rs:15-58`
3. `src/transcript.rs:107-147`
4. `src/transcript.rs:184-212`
5. `src/agent.rs:108-154`
6. `src/agent.rs:146-154`
7. `src/orchestration.rs:56-78`
8. `src/result_contract.rs:40-83`
9. `src/tools/record_structured_result.rs:141-183`
10. `tests/structured_contracts.rs:87-154`
11. `tests/orchestration.rs:413-539`
12. `tests/resume.rs:343-409`

If you finish those 12 reads in order, you will already understand more than by skimming the whole repo randomly.

## 15. Questions You Should Be Able To Answer After Reading

If you can answer these without opening the docs again, you understand the current system:

1. Where does a child session store its durable identity and lineage?
2. Which event makes a child discoverable from the parent side?
3. Why does `inspect` use parent replay instead of scanning all sessions?
4. Which function turns a parent snapshot into a child snapshot?
5. Which function makes the child's `cwd` operational during tool execution?
6. Which state fields drive lifecycle status derivation?
7. Why is `collect` not just a wrapper around `inspect`?
8. What is the difference between `structured_result` and `latest_useful_output`?
9. Where does role validation happen for typed results?
10. Why does the runtime store structured results in the event log?
11. What does resume change about `collect` semantics?
12. What is still outside the current boundary and left for `P3+`?

## 16. What To Read Next

After this walkthrough, the best next steps are:

- run [Phase 3 Hands-On Filesystem Lab](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-hands-on-filesystem-lab.md:1)
  This turns the snapshot and event model into something you have actually touched on disk.
- reread [Phase 3 Concepts And Boundaries](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-concepts-and-boundaries.md:1) and see whether the boundary language now feels concrete
- reread [Phase 3 Runtime Flows And Persistence Guide](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-runtime-flows-and-persistence-guide.md:1) and map each flow to the exact functions you just read
- use [Phase 3 Code Reading And Test Map](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-code-reading-and-test-map.md:1) as a reference index rather than as your first read
