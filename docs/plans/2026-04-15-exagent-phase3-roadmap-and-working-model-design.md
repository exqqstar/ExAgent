# ExAgent Phase 3 Roadmap And Working Model

**Date:** 2026-04-15  
**Status:** Working design note for future Phase 3 sessions  
**Related Docs:** `docs/plans/2026-04-15-phase3-session-orchestration-notes.md`, `docs/plans/2026-04-15-exagent-phase3-p0-thin-orchestration-implementation-plan.md`

## Goal

Phase 3 upgrades ExAgent from a durable single-session runtime into a usable multi-agent orchestration runtime.

The important point is not "more agents" by itself. The important point is a reliable control loop for:

`spawn -> isolate -> replay -> inspect -> collect`

Phase 3 should keep `Agent::run_session(...)` as the single-session execution kernel and layer orchestration on top of it. It should not jump early into a full planner, mailbox runtime, or autonomous task graph.

## What Phase 3 Must Deliver

At the product level, Phase 3 is mainly responsible for these capabilities:

1. parent/child session lineage
2. explicit agent roles such as `primary`, `spec`, `test`, `judge`, and `implementation`
3. replayable orchestration lifecycle events
4. thin operator-facing orchestration entrypoints through CLI and API
5. strong isolation between parent, child, and sibling-child session artifacts
6. enough inspect/collect surface that a lead session can actually manage child work

If ExAgent can only fork child sessions but cannot inspect or collect them, Phase 3 is not really complete from an operator point of view.

## Milestone Shape

### P0: Thin Orchestration Contract

This milestone establishes the minimum orchestration substrate:

- session lineage fields
- `AgentRole`
- `SessionSpawned` events
- `Agent::fork_session(...)`
- CLI/API fork surface
- replay and sibling isolation regressions

This is the correct first milestone because it creates the contract without prematurely introducing planning logic.

### P1: Status And Collect

This should be the next recommended milestone.

Add the thinnest useful lead-facing management surface:

- inspect which child sessions belong to a parent
- read each child role and basic lifecycle status
- expose a stable "latest useful output" view for a child session
- add CLI/API collect or inspect endpoints for operators

Non-goals for P1:

- autonomous planning
- mailbox coordination
- distributed scheduling
- large fan-in reduce semantics

### P2: Structured Review And Result Contracts

Once inspect/collect exists, define more explicit contracts for non-writer agents:

- structured outputs for `spec`, `test`, and `judge`
- stable review/result payloads that a lead session can consume
- clearer handoff rules between design, validation, and implementation

This milestone should make orchestration easier to automate later without forcing automation too early.

### P3+: Planner Or Higher-Order Coordination

Only consider planner-style features after the earlier milestones are stable. These later milestones may include:

- limited task decomposition
- richer collect/reduce semantics
- compaction-aware orchestration
- eval harnesses for long-running orchestration sessions

These are intentionally later-stage concerns.

## Standard Working Model

This is the preferred Phase 3 execution pattern:

`Lead + Spec Agent + Test Agent -> Lead Synthesis -> Judge Review -> Implementation Agents -> Final Review`

## Trial Workflow For Future Phase 3 Sessions

This working model should currently be treated as a preferred Phase 3 experiment, not a repository-wide mandatory policy.

That means:

- it should be used intentionally for Phase 3 milestones
- it does not automatically apply to unrelated repository work
- it should be evaluated over a few milestones before being promoted into any broader project policy

The reason for keeping it experimental first is simple: the workflow itself is part of what Phase 3 is trying to validate. The team should compare whether this model actually improves scope control, regression coverage, and integration quality relative to a single-threaded lead-only workflow.

### Recommended Trial Rules

When running this experimental workflow in a future Phase 3 session:

- the session should explicitly name one milestone
- `Spec Agent` and `Test Agent` should start as read-only planning inputs
- `Judge Agent` should review a coherent lead draft, not co-author the first draft
- implementation agents should not start until goals, non-goals, acceptance criteria, and ownership are locked
- writer agents should be capped at 1-2 concurrent writers with non-overlapping write scopes

### Recommended Session Kickoff Template

Use a kickoff prompt close to this shape:

```text
You are the Lead / Integrator for an ExAgent Phase 3 experimental workflow session.

This session is explicitly allowed to use subagents.
Use the Phase 3 trial workflow from:
docs/plans/2026-04-15-exagent-phase3-roadmap-and-working-model-design.md

This is an experiment for Phase 3, not a repository-wide mandatory policy.

For this session:
- milestone: <fill in one milestone>
- first use Spec Agent and Test Agent as read-only planning inputs
- then do Lead synthesis
- then use Judge Agent for review
- only after goals / non-goals / acceptance criteria are locked may implementation agents start

Start by outputting:
1. baseline summary
2. milestone goals
3. milestone non-goals
4. tasks for Spec / Test / Judge
5. risks before implementation
```

### Lead / Integrator

Owned by the main thread.

Responsibilities:

- choose one milestone
- define goals and non-goals
- decide ownership boundaries
- merge outputs from other agents
- run final verification

This role should not be delegated away.

### Spec Agent

Read-only by default.

Responsibilities:

- draft milestone goals
- write or refine `goals / non-goals / acceptance criteria`
- identify dependency and scope risks
- clarify contract boundaries

The Spec Agent does not make the final decision. It produces a draft for synthesis.

### Test Agent

Read-only during planning; may become a writer only after the runtime contract is stable.

Responsibilities:

- identify regression risk
- propose test matrix and acceptance checklist
- map which files should carry orchestration coverage
- call out missing replay/isolation assertions

This agent is valuable early because it can improve quality without blocking on implementation.

### Judge Agent

Read-only.

Responsibilities:

- challenge the draft plan after a coherent draft exists
- detect scope creep
- call out missing acceptance criteria
- challenge weak assumptions
- identify places where milestones are being mixed together

Judge should review the draft, not co-author the first version of the draft.

### Implementation Agents

Only after scope is locked.

Recommended split:

- Implementation Agent A: `src/session.rs`, `src/events.rs`, `src/transcript.rs`, lineage/replay tests
- Implementation Agent B: `src/agent.rs`, `src/api.rs`, `src/cli.rs`, `src/main.rs`, API/operator tests

Rules:

- avoid overlapping write sets
- never have multiple writers in the same highly coupled control-flow file at once
- do not parallelize implementation until goals, non-goals, and acceptance criteria are explicit

## When To Use Agents

Use `Spec Agent` and `Test Agent` early when:

- the next milestone is not fully specified
- acceptance criteria are still fuzzy
- there is risk of hidden regression surface

Use `Judge Agent` after a draft exists when:

- the draft might be too large
- contracts are still ambiguous
- the milestone may have mixed goals

Use implementation agents only when:

- one milestone is clearly scoped
- non-goals are written down
- acceptance criteria are explicit
- ownership can be split by file or subsystem

Do not start implementation agents if any of those conditions are still missing.

## Recommended Next Session Startup

For the next serious Phase 3 milestone, use this startup order:

1. restate current baseline
2. choose a single milestone
3. open `Spec Agent`
4. open `Test Agent`
5. synthesize a unified draft plan in the lead thread
6. open `Judge Agent` against that draft
7. lock the plan
8. open 1-2 implementation agents if the write boundaries are clean
9. finish with lead-driven full verification

## Decision Guardrails

Do not expand a Phase 3 milestone if it starts drifting into:

- full planner/runtime split
- mailbox or actor architecture
- autonomous task graphs
- cross-process scheduling
- worktree automation
- broad reduce/join semantics

Those may be valid later, but they should not be used to justify making the current milestone larger.

## Short Version

Phase 3 is not "let many agents run wild."

Phase 3 is:

- reliable session lineage
- replayable orchestration events
- isolated child execution
- operator-visible inspect/collect control
- a disciplined workflow where read-only agents sharpen the plan before writer agents touch core runtime files
