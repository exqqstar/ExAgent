# ADR-0009: Use Project-Local Rollout With Desktop SQLite Index

## Status

Accepted

## Context

ExAgent Desktop needs a Codex-like project workbench: users choose project folders, see the historical sessions for each project, rename sessions, search, pin, archive, and resume work.

The runtime already persists durable thread state as append-only JSONL rollouts under `.exagent/threads/<thread_id>/rollout.jsonl`. That rollout is the recovery, replay, and audit source of truth. A desktop sidebar, however, needs indexed metadata: project list, thread title, preview, update time, archive state, pin state, search text, changed-file summaries, and last-opened state.

Codex uses the same broad pattern: rollout JSONL files remain the durable session records, while SQLite mirrors rollout metadata for listing, filtering, search, and metadata updates. ExAgent should copy that separation, but keep rollouts project-local because the product model is "one folder is one project, and sessions belong under that project."

## Decision

Keep project-local rollout files as the durable source of truth:

```text
<project>/.exagent/threads/<thread_id>/rollout.jsonl
```

Add a global desktop SQLite index for project and thread discovery:

```text
<platform app data>/ExAgent Desktop/exagent.sqlite
```

The SQLite index stores desktop-facing metadata and cache rows only. It does not replace rollout replay, thread recovery, or model-visible history. Runtime-derived index rows must be rebuildable from project-local rollout files. User-managed desktop metadata, such as title overrides, pin state, and soft archive state, is local SQLite state and is not recoverable from rollout if the desktop database is deleted.

First-version archive is a SQLite soft archive:

- Set `archived_at` in the desktop index.
- Hide archived threads from the default session list.
- Do not move or rewrite rollout files.

First-version rename and pin are also desktop metadata:

- Store user-facing title overrides in SQLite.
- Preserve rollout-derived fallback titles and previews.
- Store pin state in SQLite.

The index layer should live in the root Rust crate, not only inside Tauri desktop code. Desktop will call it first, but thread listing, search, rename, archive, and reindexing are thread discovery/index capabilities that future CLI, HTTP, or JSON-RPC surfaces may reuse.

## Consequences

- Project directories remain self-contained for runtime history: copying a project can carry `.exagent/threads` with it.
- Desktop session lists and search can be fast without scanning every rollout on every render.
- SQLite corruption or deletion should not destroy runtime history; the index can be rebuilt from rollouts.
- Desktop-only metadata such as pin and soft archive is local to the user's app data and will not automatically follow the project to another machine.
- The app needs explicit reindex/backfill logic to reconcile missing DB rows, deleted rollout files, moved project paths, and newly discovered `.exagent/threads`.
- `.exagent` may contain prompts, commands, paths, and tool output; projects should ignore it by default unless the user intentionally wants to version agent history.
- A future ADR may upgrade archive from SQLite soft archive to project-local archive storage, for example `.exagent/archived_threads`, if archive needs to become portable project state.

## Affected Modules

- `src/state/rollout.rs`
- `src/app_server/protocol.rs`
- `src/app_server/thread_manager.rs`
- future `src/state/index_db.rs`
- future `src/app_server/thread_index.rs`
- future `apps/desktop/src-tauri`

## Related Docs

- `docs/architecture/adr/0001-use-rollout-jsonl-as-source-of-truth.md`
- `docs/protocol/app-server-boundary-v2.md`
- `docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md`
- `codex-app-server-reference-pack/README.md`
- `codex-app-server-reference-pack/02-app-server-flow.md`
