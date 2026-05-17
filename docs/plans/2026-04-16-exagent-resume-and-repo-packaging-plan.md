# ExAgent Resume And Repo Packaging Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Package the current ExAgent codebase into a resume-ready project and a public-facing repository that a recruiter or interviewer can understand in under two minutes.

**Architecture:** Keep the runtime implementation unchanged for now. Add outward-facing packaging around the existing Rust agent runtime: a concise README, public-safe setup instructions, proof of correctness, and interview-friendly project framing. Public release should happen only after the repository has a clear landing page, safe environment handling, and an explicit project status statement.

**Tech Stack:** Rust, Tokio, Axum, Serde, reqwest, cargo test, Markdown documentation

## Current-State Summary

The repository already has strong engineering signals:

- Rust agent runtime with CLI and HTTP API entrypoints
- Durable session persistence and event replay
- Persistent exec session support
- Policy / approval boundary for risky commands
- Multi-session orchestration primitives: `fork`, `inspect`, `collect`
- Structured result contracts for `spec`, `test`, and `judge`
- Passing automated test suite (`cargo test`)

The repository is currently weak in outward presentation:

- no `README.md`
- no `LICENSE`
- no public quickstart
- no demo transcript or screenshot
- no explicit "project status / non-goals / roadmap" landing page for external readers

## Positioning Recommendation

Treat ExAgent as an **agent runtime / orchestration infrastructure project**, not as a generic "AI chatbot" or "toy agent demo".

Recommended narrative:

- "Built a Rust-based durable agent runtime with replayable event logs, resumable sessions, approval-gated tool execution, and thin multi-agent orchestration primitives."

Best-fit resume lanes:

- AI infrastructure engineer
- backend engineer with systems/runtime focus
- agent platform / orchestration engineer
- Rust backend engineer

Avoid presenting it as:

- "I made a chatbot"
- "I wrapped OpenAI API in Rust"
- "A complete autonomous multi-agent platform"

The codebase is strong because it is intentionally scoped. The right story is: **clear runtime substrate, explicit boundaries, real persistence, real tests, controlled roadmap**.

### Task 1: Lock The External Story

**Files:**
- Review: `docs/plans/2026-04-15-exagent-phase3-current-state-learning-guide.md`
- Review: `docs/architecture/2026-04-15-phase2-runtime-interview-summary.md`
- Create later: `README.md`

**Step 1: Choose the primary job-market framing**

Pick one primary framing and keep every public artifact aligned with it:

- `AI infrastructure / agent runtime` if targeting AI infra or agent platform roles
- `Rust backend / systems runtime` if targeting broader backend roles

**Step 2: Write the one-sentence pitch before writing README**

Target format:

`ExAgent is a Rust-based agent runtime that adds durable session state, replayable events, approval-gated tool execution, and thin multi-session orchestration on top of an LLM tool loop.`

**Step 3: Define what the project is not**

Explicitly state:

- not a full planner
- not a production sandbox
- not a finished autonomous multi-agent framework

This improves credibility.

**Step 4: Verify the story against the codebase**

Run: `cargo test`
Expected: all tests pass and the project can honestly claim verified runtime behavior.

### Task 2: Build The README Landing Page

**Files:**
- Create: `README.md`

**Step 1: Write the first screen for busy readers**

Top section must answer four questions immediately:

- What is ExAgent?
- Why does it exist?
- What is already implemented?
- Why is the project technically interesting?

Recommended README opening structure:

1. project title + one-sentence pitch
2. 3-5 bullets of core capabilities
3. architecture diagram or compact flow
4. quickstart
5. current status and roadmap

**Step 2: Show capability, not aspiration**

README feature bullets should be grounded in existing code only:

- durable session snapshots + event logs
- resume existing sessions
- persistent exec sessions
- approval workflow for risky commands
- CLI/API orchestration via `fork`, `inspect`, `collect`
- structured child result contracts

**Step 3: Add a "Why Rust" section**

Explain the explicit runtime state model, async-safe shared state, typed events, and long-term evolvability.

**Step 4: Add a "Current Status" section**

State clearly:

- what Phase 3 already supports
- what is intentionally not implemented yet

This prevents overselling.

**Step 5: Verify the README by skimmability**

Manual check:

- a recruiter should understand the project in 30 seconds
- an engineer should know how to run tests in 60 seconds

### Task 3: Make The Repository Safe To Publish

**Files:**
- Create: `.env.example`
- Create: `LICENSE`
- Modify if needed: `.gitignore`
- Modify later: `README.md`

**Step 1: Publish only after environment expectations are explicit**

Document the required variables:

- `OPENAI_BASE_URL`
- `OPENAI_API_KEY`
- `OPENAI_MODEL`
- `EXAGENT_POLICY_MODE` if needed for examples

Do not rely on readers inferring them from source.

**Step 2: Add a minimal `.env.example`**

Use placeholders only. No real URLs or credentials beyond safe examples.

**Step 3: Add a license before making the repo public**

Without a license, others can read the code but do not have clear reuse rights. MIT is usually the simplest default for portfolio projects unless you want stricter terms.

**Step 4: Re-check public exposure**

Run:

- `rg -n "OPENAI_API_KEY|sk-|Bearer|SECRET|TOKEN|PASSWORD" -S src tests docs`

Expected:

- no real secrets
- only variable names or test fixtures

### Task 4: Add Proof For External Readers

**Files:**
- Create: `docs/demo/exagent-walkthrough.md`
- Optional create: `assets/exagent-architecture.png`
- Optional create: `assets/exagent-demo.gif`
- Modify later: `README.md`

**Step 1: Add one concrete walkthrough**

Show one realistic flow:

1. run a root session
2. fork a child session
3. inspect children
4. collect latest output

Even a text walkthrough is enough at first.

**Step 2: Add one architecture artifact**

A simple diagram showing:

`CLI/API -> Agent runtime -> Tool registry -> snapshot.json / events.jsonl`

This materially improves recruiter comprehension.

**Step 3: Link proof from README**

README should not be wall-of-text only. It should link to:

- architecture note
- walkthrough
- tests

### Task 5: Prepare Resume And Interview Packaging

**Files:**
- Optional create: `docs/portfolio/exagent-resume-notes.md`

**Step 1: Draft 2-3 resume bullets**

Recommended bullet shape:

- `Built ExAgent, a Rust-based agent runtime with durable session snapshots, replayable event logs, and approval-gated command execution for safer tool use.`
- `Designed thin multi-agent orchestration primitives including child-session fork/inspect/collect flows and typed result contracts for spec/test/judge roles.`
- `Implemented and validated runtime behavior through a passing Rust integration test suite covering persistence, replay, orchestration, policy, exec sessions, and tool dispatch.`

**Step 2: Prepare a 60-second interview version**

Talk track:

- problem: toy agents break down without runtime state and control boundaries
- solution: build a durable runtime substrate in Rust
- key design choices: snapshot + event log, approval boundary, persistent exec sessions, thin orchestration first
- current limitation: no planner yet, intentionally

**Step 3: Tailor the bullet emphasis**

If targeting AI infra:

- emphasize orchestration, contracts, runtime substrate

If targeting backend / systems:

- emphasize durability, explicit state modeling, replayability, concurrency safety

## Public Or Private Decision Rule

Default recommendation: **make the repository public after a lightweight packaging pass**, not immediately and not months later.

Make it public when these conditions are true:

- `README.md` exists and is understandable
- environment variables are documented
- `LICENSE` exists
- `cargo test` passes
- project status and non-goals are stated honestly

Keep it private temporarily if:

- you want to refactor the public story first
- commit history contains material you do not want exposed
- there are local notes or artifacts you have not reviewed yet

## Execution Order

1. Write `README.md`
2. Add `.env.example`
3. Add `LICENSE`
4. Add one walkthrough or demo artifact
5. Re-run `cargo test`
6. Make the repo public
7. Add the polished resume bullet to your CV and LinkedIn

## Definition Of Done

This packaging effort is complete when:

- the repo can be sent as a portfolio link without verbal explanation
- the README tells a truthful and technically strong story
- a reviewer can run the tests and understand the architecture
- your resume bullet matches what the repo visibly proves
