# ExAgent Phase 2 Runtime Design

**Date:** 2026-04-13

**Status:** Proposed

## Summary

Phase 2 should move ExAgent from a minimal agent loop into a durable runtime substrate for coding agents. The goal is not product parity with Codex-class systems. The goal is to establish the smallest set of reusable runtime primitives that make ExAgent feel durable instead of toy-like:

- sessions can stop and resume
- execution can stay alive across turns
- risky actions can be intercepted and approved
- long contexts can be compacted instead of failing hard
- runs can be replayed, inspected, and evaluated

Phase 2 should still preserve ExAgent's core advantage: a thin, teachable architecture with explicit causal flow.

## Context

Phase 1 proved the minimal agent loop:

- `user -> llm -> tool call -> tool result -> next turn`
- tool registry and schemas
- local file and command tools
- append-only transcript logging
- workspace-relative path resolution

Current core implementation lives primarily in:

- `src/agent.rs`
- `src/types.rs`
- `src/registry.rs`
- `src/tools/run_command.rs`
- `src/transcript.rs`
- `src/workspace.rs`

This is enough to prove the loop, but not enough for long-running, resumable, inspectable agent work.

## Phase 2 Goal

Build a runtime substrate that supports durable coding-agent execution while remaining small, explicit, and Rust-native.

Phase 2 is complete when ExAgent can:

1. resume an interrupted run from persisted state
2. maintain a persistent execution session across turns
3. intercept risky actions through policy and approval hooks
4. compact context when the session grows too large
5. replay and evaluate runs with structured traces

## Non-Goals

Phase 2 should not try to become a full product platform. The following are explicitly out of scope unless a later phase requires them:

- multi-agent orchestration
- MCP/connectors/app marketplace
- rich long-term memory pipelines
- production-grade OS sandboxing
- complex planners or autonomous task graphs
- polished end-user UI

## Design Principles

### 1. Primitives over features

Only build capabilities that remain useful across multiple future product shapes. Avoid embedding product assumptions too early.

### 2. Thin kernel, stronger contracts

Keep the runtime small, but make state, events, execution, and policy contracts explicit and typed.

### 3. Durability before breadth

Do not add many tools or workflows until resume, replay, policy, and persistent execution are stable.

### 4. Deterministic reconstruction where possible

A finished run should be inspectable and, where practical, replayable from structured state instead of only human-readable logs.

### 5. Approval before sandbox

ExAgent does not need full OS sandboxing in Phase 2, but it does need a real policy boundary with auditability.

### 6. Evaluation as a first-class primitive

Every runtime improvement should be testable against scenario-based evals, not only ad hoc manual checks.

## What "Bottom Layer" Means

In this design, the bottom layer is the set of runtime primitives every future ExAgent product variant will share. It is not a finished user product, and it is not a bag of every possible agent feature.

The bottom layer should answer:

- how a turn executes
- how a session is persisted and resumed
- how tools are registered and called
- how commands are executed and controlled
- how risky actions are approved or denied
- how long conversations are compacted
- how runs are replayed and evaluated

It should not yet answer:

- what the final UI looks like
- how autonomous the default assistant should be
- whether ExAgent is terminal-first, IDE-first, or CI-first
- what the long-term market moat is

## Capability Map

### A. Turn and Session State

**Why this is a primitive**

Every higher-level capability depends on a stable model for turns, events, and resumable session state.

**Phase 2 additions**

- add stable `session_id`, `turn_id`, and `event_id`
- separate assistant turns from runtime events
- persist enough state to resume without rebuilding everything from raw prompts
- support `resume` and `fork` as first-class runtime operations

**Likely files**

- modify `src/types.rs`
- modify `src/agent.rs`
- create `src/session.rs`
- create `src/events.rs`

### B. Tool Bus and Contracts

**Why this is a primitive**

Tools are the agent's interface to the outside world. The runtime needs consistent schemas, metadata, and error contracts.

**Phase 2 additions**

- add tool metadata beyond name and description
- classify tools by capability or risk level
- standardize tool failure taxonomy
- make approval-aware tool execution possible

**Likely files**

- modify `src/registry.rs`
- modify `src/tools/mod.rs`
- modify tool implementations under `src/tools/`

**Phase 2 tool design principles**

- prefer a small number of high-leverage tools over many overlapping tools
- make each tool do one thing well with a narrow contract
- keep tool inputs structured and outputs machine-usable
- attach policy and risk metadata to tools or tool calls
- make tool results verification-friendly instead of only human-readable
- keep tools workspace-aware and scope-bounded by default

**Recommended tool categories for a coding agent**

- inspect tools: read files, list files, search text, inspect diffs
- edit tools: targeted write or patch operations
- execute tools: command execution and persistent sessions
- verify tools: test, build, lint, or structured wrappers around those flows
- control tools: approval requests, user input, interrupt or resume operations

In practice, the most important tools are not the largest set of tools. They are the tools that let the agent complete the engineering loop with the fewest ambiguous steps.

### C. Execution Runtime

**Why this is a primitive**

Coding agents need more than one-shot commands. Durable engineering work requires persistent process state.

**Phase 2 additions**

- persistent exec sessions keyed by session id
- streamed stdout/stderr events
- optional stdin writes to active sessions
- explicit cancel/terminate behavior
- background process awareness

**Likely files**

- modify `src/tools/run_command.rs`
- create `src/exec_session.rs`
- create runtime event types for streamed output

### D. Policy and Approval

**Why this is a primitive**

Without a policy boundary, ExAgent can execute commands, but it cannot safely operate as a real coding runtime.

**Phase 2 additions**

- command classification before execution
- allow/deny/review policy outcomes
- approval request and approval response contracts
- audit trail for decisions

**Likely files**

- create `src/policy.rs`
- modify `src/agent.rs`
- modify `src/tools/run_command.rs`
- modify config to support policy modes

### E. Context Management and Compaction

**Why this is a primitive**

Long-running sessions fail without basic context budgeting. Compaction is required before advanced memory systems.

**Phase 2 additions**

- token or byte budget tracking
- compaction trigger policy
- compacted session summaries
- selective reinjection of required tool outputs or decisions

**Likely files**

- create `src/context.rs`
- modify `src/agent.rs`
- modify `src/transcript.rs` or successor persistence layer

### F. Observability and Evaluation

**Why this is a primitive**

If ExAgent cannot explain what happened, compare runs, or detect regressions, complexity will rise faster than confidence.

**Phase 2 additions**

- structured event traces
- per-step timing and result metadata
- replay tooling
- fixed eval scenarios for runtime regressions

**Likely files**

- modify `src/transcript.rs`
- create `tests/resume.rs`
- create `tests/exec_session.rs`
- create `tests/policy.rs`
- create eval fixtures under `tests/` or `fixtures/`

## Coding Agent Capability View

From a coding-agent perspective, Phase 2 should be prioritized around the core working loop:

`understand code -> edit files -> run commands -> inspect failures -> fix -> verify completion`

This is a better prioritization lens than raw feature count. A coding agent becomes credible when this loop is reliable, durable, and inspectable.

### Simple Capability Framework: 3 + 2

At the simplest level, a coding agent can be summarized in five dimensions:

- `understand`: read tasks, code, errors, and relevant context
- `execute`: make changes and run actions in the environment
- `verify`: determine whether the change is actually correct
- `sustain`: keep working across time, long context, and interruptions
- `control`: operate inside explicit safety and collaboration boundaries

The first three are the core work loop.

`understand -> execute -> verify`

The last two are runtime support layers.

`sustain + control`

Most other agent features are extensions of these five dimensions:

- memory and compaction mostly support `understand` and `sustain`
- resume and replay mostly support `sustain`
- tools mostly amplify `execute` and `verify`
- policy and approval mostly support `control`
- planning helps `understand`, but does not replace `execute` or `verify`

This is the recommended top-level frame for discussing ExAgent capability growth.

### Priority 1: Action Reliability

The first priority is making ExAgent reliably perform the basic engineering loop instead of only producing plausible next steps.

This includes:

- stable file read and write behavior
- consistent tool-call and tool-result contracts
- predictable command execution behavior
- explicit stop conditions when work is actually complete

If this is weak, added memory or planning features will mostly amplify unreliable behavior.

### Priority 2: Execution Durability

The second priority is ensuring work can continue across time instead of collapsing into one-shot commands.

This includes:

- persistent exec sessions
- resume after interruption
- interrupt, continue, and cancel semantics
- consistent session reconstruction after partial progress

This is what separates a toy loop from a runtime that can support real coding work.

### Priority 3: Verification Rigor

The third priority is making ExAgent prove work instead of only claiming completion.

This includes:

- structured build, test, and lint outputs
- explicit failure classification
- verification-aware follow-up turns
- scenario-based evals for regressions

Without this layer, ExAgent is closer to a code generator than a coding agent.

### Tool Strategy

Tools deserve special attention because they are the main way a coding agent turns intent into work. A stronger model without the right tools still cannot finish engineering tasks reliably.

Tool quality matters more than tool count. For Phase 2, ExAgent should prioritize tools that strengthen the main coding loop instead of broad product parity.

**Tool priority for Phase 2**

- first, strengthen inspect tools so the agent can read the repo accurately
- second, strengthen edit tools so changes are targeted and predictable
- third, strengthen execution tools so commands can persist across turns
- fourth, strengthen verification tools so build, test, and lint outcomes become structured evidence
- fifth, add control-oriented tools only where needed for approval or interruption

**Recommended Phase 2 tool set**

- `read_file`
- `search_text` or equivalent repo search
- `list_files`
- `apply_patch` or precise edit primitive
- `run_command`
- `exec_session_write` or equivalent stdin/session continuation primitive
- `request_approval` or equivalent approval hook surface

This is enough to support most coding-agent benchmark loops without prematurely expanding into a large tool platform.

**What to avoid in Phase 2**

- many overlapping edit tools with unclear differences
- product-specific tools that only serve one narrow workflow
- tools that return only prose instead of structured results
- tools whose side effects are too large or poorly bounded

The right question is not "what tools can we add?" but "what minimum tool set lets the agent complete coding loops reliably and verifiably?"

### Priority 4: Context Continuity

The fourth priority is preserving enough working context for long tasks to continue safely.

This includes:

- compaction triggers
- compacted summaries
- selective reinjection of critical decisions and outputs
- resume-friendly context rebuilding

In Phase 2, context continuity is more important than sophisticated long-term memory.

### Priority 5: Safe Autonomy

The fifth priority is allowing action while retaining control.

This includes:

- policy classification before execution
- approval hooks for risky actions
- audit logs for decisions and execution paths
- configurable runtime guardrails

This is the minimum safety layer required before ExAgent can be trusted in real workflows.

### What Should Not Be Prioritized Yet

The following may matter later, but they should not lead Phase 2:

- long-term semantic memory
- complex planners
- multi-agent orchestration
- connectors and dynamic tool ecosystems
- broad feature parity with large productized agents

### Capability Translation for Phase 2

From the coding-agent lens, the Phase 2 runtime should be judged by five ability goals:

1. `execute_reliably`
2. `verify_rigorously`
3. `resume_durably`
4. `compact_safely`
5. `operate_under_policy`

The architectural modules in this document exist to support these five abilities, not the other way around.

## Table Stakes for Phase 2

The minimum set of capabilities required for ExAgent to stop feeling like a toy runtime is:

1. `resume`
2. `persistent_exec_session`
3. `policy_and_approval_hooks`
4. `basic_compaction`
5. `structured_replay`
6. `scenario_eval_harness`

These are table stakes because they answer whether the system can sustain real work, not just whether it can complete a short happy-path demo.

From the coding-agent perspective, these table stakes can be restated as:

- the agent can keep acting
- the agent can keep verifying
- the agent can keep going after interruption
- the agent can keep working when context grows
- the agent can keep operating under explicit control

## Recommended Phase 2 Milestones

### P0: Durable Runtime

P0 should create the minimum credible substrate.

**Deliverables**

- structured session/event model
- persisted snapshots or resumable state records
- resume and replay commands
- persistent command execution session
- structured verification artifacts for build, test, and lint steps
- policy classification and approval hook
- basic compaction trigger and summary artifact
- first runtime regression eval suite

**Success criteria**

- a run can be interrupted and resumed later
- a shell session can survive across multiple turns
- verification results can be persisted and fed back into the next turn
- risky commands can be paused for approval instead of executing immediately
- long sessions continue through compaction instead of failing due to growth
- the same scenario can be replayed and inspected from persisted artifacts

### P1: Runtime Hardening

P1 should strengthen failure handling and operator trust.

**Deliverables**

- cancellation semantics
- clearer runtime error taxonomy
- better tool metadata and diagnostics
- config-driven policy modes
- improved compaction reinjection rules
- trace diff tooling for comparing runs

**Success criteria**

- failures are categorized instead of only stringified
- cancellations leave session state consistent
- replay artifacts make regressions visible
- policy behavior can be configured without code changes

### P2: Leverage Layer

P2 is where ExAgent can start differentiating at the runtime layer without prematurely jumping to full product surface area.

Candidate directions:

- code-aware compaction instead of generic summarization
- replay diff as a first-class debugging workflow
- approval simulation for policy tuning
- embedding-friendly SDK surface for IDE or CI use

Phase 2 should only pursue one or two of these after P0 and P1 are stable.

## Proposed Architecture

The runtime should evolve from a single in-memory loop to a session-oriented flow:

1. create or resume a session
2. rebuild active context from session state and compacted artifacts
3. ask the model for the next assistant turn
4. translate tool calls into runtime actions
5. run actions through policy classification
6. either execute, deny, or request approval
7. stream execution events into the session log
8. append tool results back into the conversation state
9. compact if the context budget is exceeded
10. persist state after each meaningful boundary

This keeps the outer loop simple while adding durability and control at the edges.

## Data Flow

### Normal execution

`user input -> session load -> context build -> llm completion -> tool calls -> policy check -> execution -> tool result -> session persist -> next turn`

### Approval path

`tool call -> policy check -> review required -> approval event persisted -> operator response -> execute or deny -> tool result`

### Resume path

`load session snapshot + event log -> reconstruct active context -> continue next turn`

### Replay path

`read persisted events -> rebuild timeline -> inspect steps and outputs without rerunning commands`

## Error Handling

Phase 2 should move away from generic string-only failure handling and toward explicit categories:

- model transport failure
- malformed tool call
- policy denial
- approval timeout or rejection
- execution timeout
- execution cancellation
- persistence failure
- context compaction failure

Errors should always be persisted as structured runtime events, even when also returned to the model.

## Testing and Evaluation

Phase 2 should be driven by scenario tests rather than only unit tests.

Required test categories:

- session resume after interruption
- persistent exec session across turns
- policy interception of dangerous commands
- approval accept and deny paths
- compaction trigger and reinjection behavior
- replay reconstruction from persisted artifacts
- regression scenarios comparing before and after behavior

Recommended eval baselines:

- create/edit/test/fix loop in a temporary workspace
- interrupted run resumed mid-task
- denied command handled cleanly by the model
- long session that requires compaction before completion

## Deferred Until After Phase 2

These are useful, but should remain deferred unless a specific product direction demands them:

- long-term semantic memory
- multi-agent orchestration
- connector ecosystems
- dynamic tool discovery
- full planner/runtime split
- production-grade sandbox backends

## Recommended Next Step

After this design is accepted, the next document should be a Phase 2 implementation plan that breaks P0 into small TDD-oriented tasks, starting with:

1. session and event model
2. persistence and resume
3. persistent exec session
4. verification result model and feedback path
5. policy and approval hook
6. basic compaction
7. replay and eval harness
