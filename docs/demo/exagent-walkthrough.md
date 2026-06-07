# ExAgent Desktop Walkthrough

This walkthrough covers the current primary flow: running ExAgent as a desktop
GUI and operating on local projects from the workbench. The HTTP API still
exists for tests and integrations, but normal desktop usage does not require
starting a local server.

## 1. Start The Desktop App

Install frontend dependencies once:

```bash
cd apps/desktop
npm ci
```

Start the Tauri development app:

```bash
npm run tauri:dev
```

The desktop app starts the Rust runtime inside the Tauri process. The React UI
communicates with Rust through Tauri commands such as `project_add`,
`thread_start`, `turn_start`, `events_subscribe`, and `approval_decision`.

## 2. Configure A Provider

Open Settings and select Providers. Choose a provider preset, then configure
the base URL, model, and credential flow.

Supported provider paths include:

- OpenAI-compatible endpoint with an API key
- ChatGPT Pro/Plus through device OAuth
- GitHub Copilot through device OAuth
- Anthropic, Google, DeepSeek, Moonshot/Kimi, or Zhipu with provider-specific
  credentials

Use the connection test and model discovery controls to verify the setup before
starting a session. Credentials are stored locally by the desktop settings
store; resolved API keys and OAuth tokens are not persisted into rollout events.

## 3. Add A Project

Use Add project in the sidebar and choose a local workspace directory.

When a project is added, the desktop facade:

1. records the project in the desktop SQLite index
2. scans the project for existing `.exagent/threads` rollout records
3. uses the project path as `workspace_root` and `cwd` for new sessions

The SQLite index is a navigation cache. The durable thread history remains in
the project under `.exagent/threads/<thread_id>/rollout.jsonl`.

## 4. Start A Session

Click New session, type a prompt into the composer, and submit it.

The desktop app then:

1. creates or resumes a runtime thread for the active project
2. sends the prompt through `turn_start`
3. subscribes to live runtime events through `events_subscribe`
4. updates the transcript, inspector, token usage panel, and agent tree from
   runtime events

The composer can submit normal turns and configured turn modes. Per-turn model
and thinking-mode choices are passed as typed turn context, not as raw provider
credentials.

## 5. Handle Approvals

When the runtime needs permission for a risky tool action, the GUI renders an
approval card. Approving or denying the card calls `approval_decision` with the
project, thread, turn, and approval id.

Approval state is live-only. Historical approval records remain in
`rollout.jsonl` for audit and replay, but replaying a cold thread does not turn
old approvals into current actionable UI state.

## 6. Review And Manage Threads

Use the sidebar to:

- search sessions
- reopen or resume previous threads
- rename, pin, archive, or unarchive conversations
- archive all conversations for a project
- reveal a project in the file manager
- create a Git worktree project for isolated implementation work

Use the inspector to review runtime events, tool activity, token usage, and
thread status. The desktop event stream first fills gaps from persisted rollout
history, then switches to live runtime events for the loaded thread.

## 7. Configure MCP Servers And Skills

Open Settings, then use MCP and Skills sections to configure runtime
extensions. Saving runtime settings rebuilds the desktop facade so new turns use
the updated MCP server list, skill roots, and model resolver configuration.

## Advanced: HTTP Boundary

For integration testing or external clients, you can still start the HTTP
boundary directly:

```bash
cargo run -- api
```

By default it listens on `127.0.0.1:3000`. Example capability check:

```bash
curl -s http://127.0.0.1:3000/initialize \
  -H 'content-type: application/json' \
  -d '{}'
```

For the full protocol surface, see
[docs/protocol/app-server-boundary-v2.md](../protocol/app-server-boundary-v2.md).
