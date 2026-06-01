# ExAgent Desktop GUI Design

## Goal

Build a single-process desktop workbench for ExAgent that feels close to Codex Desktop: users choose project folders, see sessions for each project, start or resume conversations, watch tool execution in real time, handle approvals, and inspect runtime state without leaving the app.

The first version should be directly usable for local ExAgent work. It is not a multi-client local server platform.

## Core Decisions

- Use Tauri v2 for the desktop shell.
- Use React, TypeScript, and Vite for the workbench UI.
- Use shadcn/ui as the editable low-level React component source system, with Radix primitives and Tailwind CSS tokens.
- Use Tauri commands and channels as the local in-process transport.
- Keep `AppServerService` and `protocol.rs` as the agent boundary.
- Put desktop project/session index code in the root Rust crate so it can be reused by future surfaces.
- Keep project-local rollout JSONL as the runtime source of truth.
- Use global SQLite for project registry, thread index, search metadata, rename, pin, and soft archive.
- Use `PRODUCT.md` and `DESIGN.md` as the implementation design baseline.
- Treat Codex Desktop, Linear, and macOS native apps as the primary design references.
- Treat shadcn/ui as component infrastructure, not as a visual template or product style.

## Non-Goals

- No localhost HTTP dependency for the desktop app.
- No multi-client connection manager in the first version.
- No full diff viewer or embedded editor.
- No runtime-level archive that moves rollout files.
- No autonomous planner or child-thread orchestration UI.
- No token-delta streaming unless the runtime adds it later.

## Architecture

```text
apps/desktop/src
  React workbench UI

apps/desktop/src-tauri
  Tauri commands
  folder picker
  window/app lifecycle
  channel bridge for runtime events

root Rust crate
  AppServerService
  app_server/protocol.rs
  ThreadManager
  state/index_db
  runtime/session/tools/rollout
```

The Tauri layer is a desktop facade, not a second runtime API. Commands should reuse existing protocol types when invoking agent operations:

- `thread_start`
- `thread_resume`
- `thread_read`
- `turn_start`
- `turn_interrupt`
- `events_replay`
- event subscription bridged through a Tauri channel

Desktop-only commands own desktop concerns:

- `project_add`
- `project_list`
- `project_open`
- `thread_list`
- `thread_rename`
- `thread_pin`
- `thread_archive`
- `thread_unarchive`
- `project_reindex`

## Persistence

Runtime history remains project-local:

```text
<project>/.exagent/threads/<thread_id>/rollout.jsonl
```

Desktop metadata lives in global SQLite:

```text
<platform app data>/ExAgent Desktop/exagent.sqlite
```

Minimal first-version tables:

```text
projects(
  id,
  name,
  path,
  created_at,
  last_opened_at
)

threads(
  id,
  project_id,
  rollout_path,
  user_title,
  fallback_title,
  preview,
  title_source,
  archived_at,
  pinned,
  status,
  created_at,
  updated_at,
  last_opened_at
)

thread_changed_files(
  thread_id,
  path,
  last_seen_at
)
```

Search can start with indexed `LIKE` over title and preview. SQLite FTS5 is a later upgrade.

## User Experience

Use a simplified Codex-like sidebar:

- New Chat
- Search sessions
- Projects
- Current project's sessions

## Design Style

The GUI is a product tool, not a brand or marketing surface. The style should combine:

- Codex Desktop's project/session workbench structure and agent event visibility.
- Linear's dense, quiet, high-trust product UI.
- macOS native restraint for window, sidebar, toolbar, file-picker, and local-app behavior.

The app should feel quiet, exact, and native. It should avoid SaaS landing-page visuals, decorative gradients, glass cards, oversized rounded panels, and sparse hero-page composition.

Implementation must follow:

- `PRODUCT.md`
- `DESIGN.md`

Component implementation should use shadcn/ui primitives for common controls:

- buttons
- dialogs and alert dialogs
- command palette/search
- dropdown menus
- tooltips
- scroll areas
- sheets/drawers
- tabs
- badges
- inputs and textareas
- skeleton/loading states

Do not import shadcn blocks as whole screens. Compose ExAgent-specific workbench
components around the design rules in `DESIGN.md`.

Main chat area:

- Session title and project path
- Turn transcript
- Inline assistant messages
- Inline tool output summaries with expandable details
- Inline approval cards
- Bottom prompt composer

Responsive inspector:

- Wide windows show the right inspector by default.
- Medium windows collapse the inspector into a drawer.
- Narrow windows prioritize chat and make the sidebar collapsible.

Inspector content:

- Turn progress and thread status
- Environment: workspace, cwd, policy, model/thinking mode when available
- Token usage
- Changed files list
- Recent runtime events summary

## Session Titles

Session title resolution:

1. Use `threads.user_title` when present.
2. Otherwise use rollout-derived `fallback_title`.
3. Otherwise use the first or latest user message summary.
4. Otherwise show a short thread id.

Renaming writes only SQLite metadata. It does not rewrite rollout files.

## Tool Output And Approvals

Render tool work as inline summary plus expandable details:

- `tool_result`: one-line tool summary, expandable metadata/result.
- `exec_output`: collapsed by default; expanded view shows command output with truncation.
- `approval_requested`: inline card with reason, command/cwd when available, and Approve/Deny/Interrupt actions.
- `runtime_error`: visible error block with expandable details.
- `token_count` and `compaction_written`: inspector-first, not primary chat content.

The first version shows changed files as a list only. It does not include a diff viewer.

## Data Flow

Startup:

1. Initialize SQLite index.
2. Load projects from SQLite.
3. Open the last project if available.
4. Reindex the selected project in the background.

Add project:

1. Tauri opens a native folder picker.
2. The selected folder is stored as a project row.
3. The app scans `.exagent/threads`.
4. Missing thread index rows are backfilled from rollout metadata.

Open session:

1. Load thread metadata from SQLite.
2. Call `thread_resume` or `thread_read` through `AppServerService`.
3. Subscribe to runtime events through a Tauri channel.
4. Render the latest `ThreadView` plus incoming events.

Start turn:

1. Submit `turn_start`.
2. Receive an in-progress turn response.
3. Stream runtime events to the UI.
4. Update SQLite thread metadata: status, updated time, preview, token snapshot, changed files.

Archive:

1. Set `threads.archived_at`.
2. Hide the thread from the default session list.
3. Leave rollout files untouched.

## Error Handling

- If SQLite is unavailable, the app should show a blocking startup error because the desktop workbench depends on indexed project/session state.
- If a project path no longer exists, keep the project row but mark it unavailable and prompt the user to locate or remove it.
- If a SQLite thread row points to a missing rollout file, mark it stale and omit it from default lists after reindex.
- If a rollout exists but is not indexed, backfill it.
- If `thread_resume` fails, keep the session visible and show a recoverable error with a reindex action.
- If event subscription fails, fall back to `events_replay` and expose reconnect.

## Testing

Root crate:

- Unit tests for SQLite migrations and CRUD.
- Reindex tests from synthetic `.exagent/threads/<id>/rollout.jsonl`.
- Thread list filtering tests for project, archived state, pinned ordering, and search.
- Soft archive and rename tests proving rollout files are not modified.
- Unarchive tests proving only SQLite metadata changes.

Desktop facade:

- Tauri command tests for project add/list and thread list.
- Channel bridge test from runtime event receiver to frontend event payload.

Frontend:

- Component tests for sidebar session grouping, inline tool cards, approval cards, and inspector collapse behavior.
- End-to-end smoke test: add project, start thread, submit turn with mock LLM/tool events, archive and unarchive through metadata.

## Follow-Up Decisions

- Whether to add runtime-level `thread/list`, `thread/name/set`, and `thread/archive` to public app-server protocol.
- Whether archive should become project-portable by moving rollouts under `.exagent/archived_threads`.
- Whether search should use SQLite FTS5.
- Whether changed files should grow into a full diff drawer.
- Whether external clients should use HTTP, JSON-RPC over stdio/socket, or an in-process client handle.

## References

- `docs/architecture/adr/0009-use-project-local-rollout-with-desktop-sqlite-index.md`
- `codex-app-server-reference-pack/README.md`
- `codex-app-server-reference-pack/02-app-server-flow.md`
- `codex-app-server-reference-pack/05-adaptation-guide.md`
- OpenAI Codex app-server README: `https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md`
- OpenAI Codex in-process app-server host: `https://github.com/openai/codex/blob/main/codex-rs/app-server/src/in_process.rs`
- OpenAI Codex rollout recorder: `https://github.com/openai/codex/blob/main/codex-rs/rollout/src/recorder.rs`
