# ExAgent Phase 3 Concepts And Boundaries

**Date:** 2026-04-15  
**Status:** Detailed concept guide for the current Phase 3 baseline  
**Audience:** Readers who want to understand what Phase 3 means conceptually before reading runtime code

## 1. Why Phase 3 Exists

Phase 2 gave ExAgent a durable single-session runtime. That means one session could:

- keep a conversation history
- persist runtime events
- run tools
- survive resume
- preserve policy and exec-session state

Phase 3 exists because that is not enough for orchestration.

Once you want a lead session to delegate work to child sessions, you immediately need new guarantees:

- parent and child lineage must be explicit
- child artifacts must be isolated
- lead must be able to discover and inspect child sessions later
- lead must be able to collect useful outputs from child sessions
- non-writer review roles need something more stable than free-form prose

Phase 3 is therefore not “more agents” in the abstract. It is a thin orchestration layer built on top of the single-session kernel.

## 2. The Three Layers Already Landed

### P0: Thin Orchestration Contract

P0 established the minimum substrate:

- lineage fields in snapshot state
- `AgentRole`
- replayable spawn events
- `fork_session(...)`
- CLI/API fork surface
- disk isolation regressions

You can think of P0 as the answer to:

“Can one session safely create another session and preserve the relationship on disk?”

### P1: Status And Collect

P1 established the thinnest lead-facing read model:

- inspect direct children of a parent
- see child role and basic lifecycle state
- collect the latest useful output from a child

You can think of P1 as the answer to:

“After I fork children, can I later see them and read what they produced?”

### P2: Structured Review And Result Contracts

P2 established typed handoff artifacts for non-writer roles:

- versioned structured result schema
- `StructuredResultRecorded` event
- role validation for `spec`, `test`, `judge`
- additive `structured_result` in collect

You can think of P2 as the answer to:

“Can a lead consume review/design outputs without parsing arbitrary prose?”

## 3. The Most Common Confusion

The most important distinction is:

- **workflow roles**
  `Lead`, `Spec`, `Test`, `Judge`, `Implementation`
- **runtime capabilities**
  `fork`, `inspect`, `collect`, `structured_result`

The workflow roles describe **how humans or sessions collaborate during development**.

The runtime capabilities describe **what the product can persist and expose**.

Current Phase 3 supports the workflow, but it does not automate the workflow.

That means:

- there is no full planner
- there is no agent scheduler deciding who does what
- there is no automatic “spawn Spec Agent, wait, then spawn Judge Agent” system

What does exist is the data model that makes that workflow representable in the runtime.

## 4. Glossary

### Session

A persisted unit of work with:

- one `session_id`
- one `snapshot.json`
- one `events.jsonl`

### Parent Session

The session that created a child via `fork_session(...)`.

### Child Session

A session whose `parent_session_id` is set.

### Root Session

The topmost ancestor in a lineage chain. All descendants share the same `root_session_id`.

### Agent Role

A metadata label describing the session’s intended function:

- `primary`
- `spec`
- `test`
- `judge`
- `implementation`

Important: role is metadata. It is not an autonomous behavior engine.

### Inspect

A read-only topology/status view over a parent’s direct children.

### Collect

A read-only content view over a single child session.

### Structured Result

A typed, versioned result envelope for `spec`, `test`, or `judge`, persisted as an event and surfaced through `collect`.

## 5. What Each Surface Is Allowed To Answer

### `fork`

Answers:

- can a parent create a child session
- can the relationship be replayed later
- can child disk artifacts stay isolated

Does not answer:

- whether the child succeeded semantically
- what the child produced

### `inspect(parent_session_id)`

Answers:

- which direct children belong to this parent
- what role each child has
- what state each child appears to be in
- where the child’s artifacts live

Does not answer:

- full child transcript content
- typed review body
- merged output across many children

### `collect(session_id)`

Answers:

- what one child session produced that the lead can consume
- whether that session has a typed structured result
- what the fallback legacy output is

Does not answer:

- how to reduce many child sessions into one conclusion
- how to automatically decide next child work

## 6. Current Lifecycle States

The system currently derives only a very small lifecycle model:

- `waiting_approval`
- `running`
- `completed`

This is intentionally thin.

It is not trying to represent:

- blocked on dependency
- awaiting review
- partially integrated
- scheduled
- abandoned

That restraint matters because it keeps P1/P2 from drifting into scheduler semantics too early.

## 7. What P2 Added Without Crossing The Line

P2 could easily have drifted into planner territory. It did not, because the current contract stays narrow:

- only `spec`, `test`, and `judge` can publish structured review results
- `structured_result` is additive on collect, not a new orchestration controller
- inspect remains topology/status only
- the typed result is persisted as an event, not as a planner state machine

So P2 improves machine-readability without changing the runtime into a workflow engine.

## 8. What Is Still Outside The Boundary

The current codebase still does **not** define:

- a canonical task object
- child dependency edges
- lead-side reduction semantics
- orchestration retries or cancellation
- compaction-aware collect semantics
- long-session eval scenarios at the Phase 3 orchestration level

Those are exactly the kinds of things that belong in later `P3+` work.

## 9. How To Mentally Model The System Correctly

If you want a compact mental model, use this:

1. `P0` = create lineage
2. `P1` = read child sessions
3. `P2` = read typed review outputs

Or, in runtime terms:

1. spawn child
2. isolate child
3. replay child relationship
4. inspect child existence/status
5. collect child outputs
6. prefer typed review result when present

That is the current Phase 3 contract in one ladder.

## 10. Recommended Reading After This File

If this file made sense, continue with:

- [Phase 3 Runtime Flows And Persistence Guide](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-runtime-flows-and-persistence-guide.md:1)
- [Phase 3 Code Reading And Test Map](/Volumes/EXEXEX/ExAgent/docs/plans/2026-04-15-exagent-phase3-code-reading-and-test-map.md:1)
