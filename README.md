<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.png" alt="ExAgent app icon" width="96" height="96">
</p>

<h1 align="center">ExAgent</h1>

<p align="center">
  A local desktop workbench for coding agents: projects, durable sessions, tool approvals, subagents, goals, and live runtime inspection in one GUI.
</p>

<p align="center">
  English | <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <img src="docs/assets/exagent-desktop-chat.png" alt="ExAgent desktop GUI showing a running chat session, composer, goal control, and runtime inspector" width="1200">
</p>

## What It Is

ExAgent is a desktop-first agent workbench backed by a Rust runtime and a
Tauri/React GUI. It is built for long-running coding work inside local
projects: start a session, choose a provider/model, approve tool actions,
inspect runtime state, and resume the thread later from durable local history.

It is not just a chat UI. ExAgent includes the runtime pieces needed for
recoverable agent work: event replay, persistent shell sessions, approval-gated
tools, subagents, goal tracking, project memory, MCP tools, workflow runs, and
a desktop inspector for what the agent is doing.

## Highlights

- Desktop-first local agent workbench for coding projects
- Durable sessions that can be reopened from local project history
- GUI provider setup for API-key and OAuth-based model providers
- Approval-gated coding tools with live transcript and event inspection
- Project memory with automatic recall, explicit memory tools, local curation,
  and audit state
- Persistent shells, subagents, goals, MCP tools, workflows, and `SKILL.md`
  support

## Download

macOS builds are published on the
[GitHub Releases](https://github.com/exqqstar/ExAgent/releases) page.

For macOS, download the universal DMG asset, open it, and drag ExAgent into
Applications. The release build is signed and notarized with Developer ID.

## Quickstart

### Prerequisites

- Rust toolchain
- Node.js and npm
- A model provider credential you are comfortable using locally

### Start the desktop app

```bash
cd apps/desktop
npm ci
npm run tauri:dev
```

The desktop app launches the Tauri shell and Vite frontend. For normal use,
you configure providers, projects, and sessions from the GUI.

### Configure a provider

Open **Settings** -> **Providers**, then add an API-key provider or complete an
OAuth flow. Credentials are stored locally by the desktop app. Use a dedicated
project credential and keep local app data private.

### Add a project and start a session

Use the sidebar to add a local workspace directory, create a new session, type
into the composer, and submit a turn. When ExAgent needs approval for a command
or file mutation, the app shows an approval card in the transcript.

For a fuller operator walkthrough, see
[docs/demo/exagent-walkthrough.md](docs/demo/exagent-walkthrough.md).

## Architecture At A Glance

ExAgent is organized around a local Rust runtime exposed to the desktop through
a typed app-server boundary. The Tauri shell stays project-aware while the
runtime owns thread execution, model calls, tools, state, and live events.

- Each thread runs behind an actor-backed `ThreadRuntime`, so turns are
  serialized while snapshots, status, and events stream back to the GUI.
- `ThreadSession` assembles the long-lived pieces for one thread: agent,
  context, rollout storage, tools, goals, memory, policy, and execution
  sessions.
- The context layer keeps real conversation history separate from prompt-only
  internal context such as memory recall, goal state, skills, and project docs;
  compaction can replace long history with structured summaries.
- Local durability is append-first: each thread has a `rollout.jsonl` ledger,
  while `IndexDb` stores cross-thread indexes for projects, threads, goals,
  memory, and review state.
- The tool system separates public tool contracts in `src/tools` from per-turn
  runtime orchestration in `src/runtime/tool`; agent policy gates both tool
  visibility and execution.
- The memory system supports automatic prompt recall, explicit memory tools,
  candidate saves, local promotion/archive/forget flows, and audit state.
- The model layer normalizes provider-specific APIs into ExAgent conversation,
  tool-call, multimodal, reasoning, and streaming types.
- The workflow runtime powers structured runs such as deep search as a
  phase-based scheduler parallel to the normal chat turn loop.

## Development

Useful commands from `apps/desktop`:

```bash
npm ci
npm run tauri:dev
npm test
npm run build
```

Useful commands from the repository root:

```bash
cargo test --package exagent --locked
cargo test --package exagent-desktop --locked
cargo fmt --all -- --check
cargo clippy --package exagent --all-targets
cargo deny check licenses sources bans
```

## Project Status

ExAgent is an early local-first desktop project. It currently targets personal
workstation use rather than a hosted multi-user service.

Current non-goals:

- no production-grade sandbox isolation
- no hosted collaboration service
- no stable public SDK yet

## Repository Layout

- [apps/desktop](apps/desktop): Tauri desktop shell and React workbench
- [apps/desktop/src-tauri](apps/desktop/src-tauri): desktop Rust commands,
  settings, provider auth, and Tauri entrypoint
- [src/app_server](src/app_server): typed desktop/runtime boundary, request
  processors, live views, and projections
- [src/runtime](src/runtime): live execution kernel, thread actor, session turn
  loop, agent sampling, tool runtime, policy, and exec sessions
- [src/runtime/agent_profile](src/runtime/agent_profile): agent role catalog
  and capability policy
- [src/runtime/goal](src/runtime/goal): structured goal state, accounting, and
  continuation effects
- [src/runtime/memory](src/runtime/memory): runtime memory bridge into context
  and tools
- [src/runtime/workflow](src/runtime/workflow): structured workflow and deep
  search runtime
- [src/tools](src/tools): tool trait, registry, and built-in coding tools
- [src/state](src/state): durable rollout models, desktop index storage, and
  memory state
- [src/model](src/model): model provider adapters and conversation types
- [src/mcp](src/mcp): MCP configuration and tool integration
- [tests](tests): integration coverage for runtime, protocol, policy, tools,
  and storage
- [docs/demo](docs/demo): desktop-first walkthroughs

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, verification
commands, and pull request expectations.

Please keep secrets out of issues, pull requests, rollout files, and logs. Use
[SECURITY.md](SECURITY.md) for vulnerability reports.

## Third-Party Notices

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the dependency license
policy and rules for external reference material.

## Authors And Notices

ExAgent was created by exqqstar. See [AUTHORS.md](AUTHORS.md) for authorship
and contribution attribution, and [NOTICE](NOTICE) for distribution notices.

## License

Copyright (c) 2026 exqqstar.

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
