# ExAgent Desktop GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a single-process Tauri desktop workbench for ExAgent with project folders, per-project sessions, chat, live tool output, approvals, changed-file summaries, and responsive inspector state.

**Architecture:** The desktop app uses Tauri commands and channels as an in-process transport. Tauri calls a root-crate desktop facade, which reuses `AppServerService`, `protocol.rs`, and a root-crate SQLite index. Runtime history stays in project-local `.exagent/threads/<thread_id>/rollout.jsonl`; SQLite stores project/session index metadata, title overrides, pin state, soft archive, search cache, and changed-file summaries.

**Tech Stack:** Rust, Tokio, Axum-free in-process service calls, SQLx SQLite, Tauri v2, React, TypeScript, Vite, Tailwind CSS, shadcn/ui, TanStack Query, Zustand, Radix primitives, lucide-react, Vitest, Playwright.

---

## Source Documents

- `PRODUCT.md`
- `DESIGN.md`
- `docs/architecture/adr/0009-use-project-local-rollout-with-desktop-sqlite-index.md`
- `docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md`
- `docs/protocol/app-server-boundary-v2.md`
- `codex-app-server-reference-pack/02-app-server-flow.md`
- `codex-app-server-reference-pack/05-adaptation-guide.md`
- shadcn/ui Vite installation and theming docs

## Goal Feature Usage

This file is structured so the goal feature can run one task at a time.

- Use each `Task N` title as a goal objective.
- Treat each task's acceptance criteria as the goal completion condition.
- Do not start a later task until the current task's verification commands pass.
- Commit after each task when implementing on a clean branch.
- If a task reveals a design conflict, update the ADR/spec before continuing.

## Overall Acceptance Criteria

- `cargo test` passes.
- `cargo fmt --check` passes.
- `git diff --check` passes.
- `npm test --workspace apps/desktop` or the equivalent desktop test command passes.
- `npm run build --workspace apps/desktop` passes.
- `cargo tauri build` or `npm run tauri build --workspace apps/desktop` reaches a successful desktop build on the local platform.
- A user can add a project folder, see indexed sessions from `.exagent/threads`, start a new chat, resume an existing chat, submit a turn, observe live events, see tool output summaries, soft-archive a session, rename a session, pin a session, and see changed files in the inspector.
- The desktop app does not require a localhost HTTP server.
- No UI code calls `ThreadRuntime` directly.
- UI implementation follows `PRODUCT.md` and `DESIGN.md`.

## Implementation Checkpoint: 2026-06-01

First usable workbench slice has been implemented.

Delivered:

- Root-crate SQLite desktop index for project registry, thread listing, search metadata, rename, pin, archive, and rollout reindexing.
- Root-crate desktop facade that reuses `AppServerService` instead of duplicating runtime control logic in Tauri.
- Typed approval-decision protocol path from desktop UI through app-server boundary into the live runtime.
- Tauri v2 desktop shell with in-process commands for project, thread, turn, approval, event replay, and event subscription operations.
- React/Vite/shadcn workbench UI with project sidebar, session list, search, new session, rename, pin, archive, chat transcript, composer, approval card, inspector, changed files, and event summary panels.
- `package-lock.json` for reproducible desktop frontend dependency installation.
- Stable dev launcher: `npm run tauri:dev --prefix apps/desktop` runs `tauri dev --no-watch` against Vite on `localhost:1420`.
- Debug macOS desktop bundles:
  - `/Volumes/EXEXEX/ExAgent/target/debug/bundle/macos/ExAgent Desktop.app`
  - `/Volumes/EXEXEX/ExAgent/target/debug/bundle/dmg/ExAgent Desktop_0.1.0_aarch64.dmg`

Verified:

```bash
cargo fmt --check
cargo test
git diff --check
npm test --prefix apps/desktop
npm run build --prefix apps/desktop
npm run tauri:dev --prefix apps/desktop
npm run tauri:build --prefix apps/desktop -- --debug
```

Known first-slice limits:

- Changed-file summaries are wired in the UI model but still depend on richer runtime event/index extraction in a later slice.
- Archive is intentionally local SQLite soft archive per ADR-0009.
- `tauri:dev:watch` is available for Rust-side hot reload, but the default `tauri:dev` disables Tauri's watcher to avoid workspace `node_modules` and `dist` changes repeatedly restarting the desktop shell.

## File Structure

Root crate changes:

- Modify `Cargo.toml`: add workspace membership and SQLite dependencies.
- Modify `src/lib.rs`: export the new reusable desktop/index modules.
- Modify `src/state/mod.rs`: add `index_db`.
- Create `src/state/index_db/mod.rs`: public index DB API and DTOs.
- Create `src/state/index_db/schema.rs`: SQLite schema and migration runner.
- Create `src/state/index_db/store.rs`: project/thread CRUD, list, search, rename, pin, archive.
- Create `src/state/index_db/reindex.rs`: project rollout scanner and metadata extraction.
- Create `src/state/index_db/time.rs`: UTC timestamp helpers.
- Create `src/app_server/desktop_facade.rs`: non-Tauri facade combining `AppServerService` and `IndexDb`.
- Modify `src/app_server/mod.rs`: export `desktop_facade`.
- Modify `src/app_server/protocol.rs`: add approval decision boundary types in Task 7.
- Modify `src/app_server/thread_manager.rs`: implement approval decision handling in Task 7.
- Test `tests/index_db.rs`.
- Test `tests/desktop_facade.rs`.
- Test `tests/approval_decision.rs`.
- Modify `tests/module_layout.rs`: ensure new public module paths compile.

Desktop app changes:

- Create `apps/desktop/package.json`.
- Create `apps/desktop/index.html`.
- Create `apps/desktop/tsconfig.json`.
- Create `apps/desktop/vite.config.ts`.
- Create `apps/desktop/src/main.tsx`.
- Create `apps/desktop/src/App.tsx`.
- Create `apps/desktop/components.json`.
- Create `apps/desktop/src/api/exagentClient.ts`.
- Create `apps/desktop/src/stores/workbenchStore.ts`.
- Create `apps/desktop/src/types.ts`.
- Create `apps/desktop/src/lib/utils.ts`.
- Create `apps/desktop/src/components/ui/*`: shadcn/ui low-level components.
- Create `apps/desktop/src/components/AppShell.tsx`.
- Create `apps/desktop/src/components/Sidebar.tsx`.
- Create `apps/desktop/src/components/ChatView.tsx`.
- Create `apps/desktop/src/components/Composer.tsx`.
- Create `apps/desktop/src/components/Inspector.tsx`.
- Create `apps/desktop/src/components/TurnItem.tsx`.
- Create `apps/desktop/src/components/ApprovalCard.tsx`.
- Create `apps/desktop/src/components/ChangedFiles.tsx`.
- Create `apps/desktop/src/styles.css`.
- Create `apps/desktop/src-tauri/Cargo.toml`.
- Create `apps/desktop/src-tauri/tauri.conf.json`.
- Create `apps/desktop/src-tauri/src/main.rs`.
- Create `apps/desktop/src-tauri/src/lib.rs`.
- Create `apps/desktop/src-tauri/src/state.rs`.
- Create `apps/desktop/src-tauri/src/commands.rs`.
- Create `apps/desktop/src-tauri/src/events.rs`.

## Task 0: Product And Design Baseline

**Files:**
- Read: `PRODUCT.md`
- Read: `DESIGN.md`
- Read: `docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md`

- [ ] **Step 1: Read the design baseline**

Run:

```bash
sed -n '1,220p' PRODUCT.md
sed -n '1,260p' DESIGN.md
sed -n '1,180p' docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md
```

Expected: the implementer can identify the product register, references, anti-references, palette, typography, layout, and component rules.

- [ ] **Step 2: Record implementation constraints before editing UI**

Before touching `apps/desktop/src`, write a short note in the task log or PR body that confirms:

```text
Register: product
References: Codex Desktop, Linear, macOS native
Style: quiet, exact, native
Do not use: gradients, glass cards, oversized radii, landing-page layout
Primary layout: sidebar + chat + responsive inspector
```

- [ ] **Step 3: Verify no conflicting UI framework is introduced**

Before adding frontend dependencies, confirm the dependency set remains:

```text
React, TypeScript, Vite, TanStack Query, Zustand, Radix primitives, lucide-react
```

shadcn/ui is allowed as a source component system because it copies editable component code into the desktop app. Do not add a large runtime component framework unless a later ADR changes this decision.

**Acceptance Criteria:**

- Every UI task explicitly follows the same product and visual baseline.
- No task starts from a blank aesthetic.
- Codex Desktop, Linear, and macOS native are the documented references.
- shadcn/ui is documented as component infrastructure, not a visual template.

## Task 1: Workspace And Dependency Baseline

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/state/mod.rs`
- Test: `tests/module_layout.rs`

- [ ] **Step 1: Add a failing module layout test for the future index module**

Add this assertion to `tests/module_layout.rs`:

```rust
use exagent::state::index_db;
use exagent::index_db as compat_index_db;

#[test]
fn index_db_module_paths_compile() {
    let names = [
        std::any::type_name::<index_db::ProjectRecord>(),
        std::any::type_name::<index_db::ThreadRecord>(),
        std::any::type_name::<compat_index_db::ProjectRecord>(),
    ];
    assert!(names.iter().all(|name| !name.is_empty()));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test --test module_layout index_db_module_paths_compile
```

Expected: failure because `exagent::state::index_db` does not exist.

- [ ] **Step 3: Add workspace and dependency entries**

Edit `Cargo.toml`:

```toml
[workspace]
members = [".", "apps/desktop/src-tauri"]
resolver = "2"
```

Add dependencies:

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "sqlite", "time"] }
```

- [ ] **Step 4: Add empty module exports**

Create `src/state/index_db/mod.rs`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: crate::types::ThreadId,
    pub project_id: String,
    pub rollout_path: std::path::PathBuf,
}
```

Modify `src/state/mod.rs`:

```rust
pub mod events;
pub mod index_db;
pub mod rollout;
pub mod session;
pub mod transcript;
```

Modify `src/lib.rs`:

```rust
pub use state::index_db;
```

- [ ] **Step 5: Verify the module layout test passes**

Run:

```bash
cargo test --test module_layout index_db_module_paths_compile
```

Expected: pass.

- [ ] **Step 6: Verify full Rust formatting**

Run:

```bash
cargo fmt --check
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml src/lib.rs src/state/mod.rs src/state/index_db/mod.rs tests/module_layout.rs
git commit -m "chore: add desktop index module baseline"
```

**Acceptance Criteria:**

- Root crate is a workspace root.
- `exagent::state::index_db` and `exagent::index_db` compile.
- No runtime behavior changes yet.

## Task 2: SQLite Schema And IndexDb Open

**Files:**
- Modify: `src/state/index_db/mod.rs`
- Create: `src/state/index_db/schema.rs`
- Create: `src/state/index_db/time.rs`
- Test: `tests/index_db.rs`

- [ ] **Step 1: Write failing schema tests**

Create `tests/index_db.rs`:

```rust
use exagent::index_db::IndexDb;
use tempfile::tempdir;

#[tokio::test]
async fn index_db_open_creates_schema() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("exagent.sqlite");

    let db = IndexDb::open(&db_path).await.unwrap();
    let version = db.schema_version().await.unwrap();

    assert_eq!(version, 1);
    assert!(db_path.exists());
}

#[tokio::test]
async fn index_db_open_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("exagent.sqlite");

    IndexDb::open(&db_path).await.unwrap();
    let db = IndexDb::open(&db_path).await.unwrap();

    assert_eq!(db.schema_version().await.unwrap(), 1);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test --test index_db index_db_open_creates_schema index_db_open_is_idempotent
```

Expected: failure because `IndexDb` is missing.

- [ ] **Step 3: Implement schema module**

Create `src/state/index_db/schema.rs`:

```rust
use sqlx::{Executor, SqlitePool};

pub const SCHEMA_VERSION: i64 = 1;

pub async fn migrate(pool: &SqlitePool) -> sqlx::Result<()> {
    pool.execute("PRAGMA foreign_keys = ON").await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_opened_at INTEGER NOT NULL
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY NOT NULL,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  rollout_path TEXT NOT NULL,
  user_title TEXT,
  fallback_title TEXT NOT NULL,
  preview TEXT NOT NULL,
  title_source TEXT NOT NULL,
  archived_at INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_opened_at INTEGER,
  UNIQUE(project_id, id)
)
        "#,
    )
    .await?;
    pool.execute(
        r#"
CREATE TABLE IF NOT EXISTS thread_changed_files (
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  last_seen_at INTEGER NOT NULL,
  PRIMARY KEY(thread_id, path)
)
        "#,
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_threads_project_visible ON threads(project_id, archived_at, pinned, updated_at)",
    )
    .await?;
    pool.execute(
        "CREATE INDEX IF NOT EXISTS idx_threads_search ON threads(project_id, user_title, fallback_title, preview)",
    )
    .await?;
    pool.execute(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .await?;
    Ok(())
}
```

Create `src/state/index_db/time.rs`:

```rust
use time::OffsetDateTime;

pub fn now_unix_seconds() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
```

- [ ] **Step 4: Implement IndexDb open**

Replace `src/state/index_db/mod.rs` with:

```rust
mod schema;
mod time;

use std::path::{Path, PathBuf};

use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

#[derive(Clone)]
pub struct IndexDb {
    pool: SqlitePool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: crate::types::ThreadId,
    pub project_id: String,
    pub rollout_path: PathBuf,
}

impl IndexDb {
    pub async fn open(path: impl AsRef<Path>) -> sqlx::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(sqlx::Error::Io)?;
        }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        schema::migrate(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn schema_version(&self) -> sqlx::Result<i64> {
        let row = sqlx::query("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await?;
        row.try_get(0)
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test --test index_db index_db_open_creates_schema index_db_open_is_idempotent
```

Expected: pass.

- [ ] **Step 6: Run formatting and diff checks**

Run:

```bash
cargo fmt --check
git diff --check
```

Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml src/state/index_db tests/index_db.rs
git commit -m "feat: add desktop sqlite index schema"
```

**Acceptance Criteria:**

- SQLite file is created at the requested path.
- Schema version is `1`.
- Reopening an existing DB is idempotent.

## Task 3: Project Registry CRUD

**Files:**
- Create: `src/state/index_db/store.rs`
- Modify: `src/state/index_db/mod.rs`
- Test: `tests/index_db.rs`

- [ ] **Step 1: Write failing project CRUD tests**

Append to `tests/index_db.rs`:

```rust
use exagent::index_db::ProjectUpsert;

#[tokio::test]
async fn project_registry_upserts_and_lists_projects_by_last_opened() {
    let dir = tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let alpha = dir.path().join("alpha");
    let beta = dir.path().join("beta");
    tokio::fs::create_dir_all(&alpha).await.unwrap();
    tokio::fs::create_dir_all(&beta).await.unwrap();

    let first = db
        .upsert_project(ProjectUpsert {
            name: "Alpha".into(),
            path: alpha.clone(),
        })
        .await
        .unwrap();
    let second = db
        .upsert_project(ProjectUpsert {
            name: "Beta".into(),
            path: beta.clone(),
        })
        .await
        .unwrap();

    db.touch_project(&first.id).await.unwrap();
    let projects = db.list_projects().await.unwrap();

    assert_eq!(projects[0].id, first.id);
    assert_eq!(projects[1].id, second.id);
    assert_eq!(projects[0].path, alpha.canonicalize().unwrap());
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test --test index_db project_registry_upserts_and_lists_projects_by_last_opened
```

Expected: failure because project CRUD methods are missing.

- [ ] **Step 3: Add store module and project DTO**

Modify `src/state/index_db/mod.rs`:

```rust
mod schema;
mod store;
mod time;

pub use store::{ProjectUpsert, ThreadListFilter};
```

Create `src/state/index_db/store.rs`:

```rust
use std::path::PathBuf;

use sqlx::Row;

use super::{time, IndexDb, ProjectRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectUpsert {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadListFilter {
    pub project_id: String,
    pub include_archived: bool,
    pub search: Option<String>,
}

pub(crate) fn project_id_from_path(path: &std::path::Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.display().to_string().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("project_{hash:016x}")
}

impl IndexDb {
    pub async fn upsert_project(&self, input: ProjectUpsert) -> anyhow::Result<ProjectRecord> {
        let path = tokio::fs::canonicalize(input.path).await?;
        let now = time::now_unix_seconds();
        let id = project_id_from_path(&path);
        sqlx::query(
            r#"
INSERT INTO projects (id, name, path, created_at, last_opened_at)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(path) DO UPDATE SET
  name = excluded.name,
  last_opened_at = excluded.last_opened_at
            "#,
        )
        .bind(&id)
        .bind(input.name)
        .bind(path.display().to_string())
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(ProjectRecord { id, name: path.file_name().and_then(|name| name.to_str()).unwrap_or("Project").to_string(), path })
    }

    pub async fn touch_project(&self, project_id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE projects SET last_opened_at = ? WHERE id = ?")
            .bind(time::now_unix_seconds())
            .bind(project_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn list_projects(&self) -> anyhow::Result<Vec<ProjectRecord>> {
        let rows = sqlx::query(
            "SELECT id, name, path FROM projects ORDER BY last_opened_at DESC, name ASC",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProjectRecord {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    path: PathBuf::from(row.try_get::<String, _>("path")?),
                })
            })
            .collect()
    }
}
```

- [ ] **Step 4: Fix returned project name**

The initial implementation above returns a path-derived name after upsert. Replace the final `Ok(ProjectRecord { ... })` in `upsert_project` with a select:

```rust
let row = sqlx::query("SELECT id, name, path FROM projects WHERE id = ?")
    .bind(&id)
    .fetch_one(self.pool())
    .await?;
Ok(ProjectRecord {
    id: row.try_get("id")?,
    name: row.try_get("name")?,
    path: PathBuf::from(row.try_get::<String, _>("path")?),
})
```

- [ ] **Step 5: Run focused test**

Run:

```bash
cargo test --test index_db project_registry_upserts_and_lists_projects_by_last_opened
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/state/index_db tests/index_db.rs
git commit -m "feat: add desktop project registry"
```

**Acceptance Criteria:**

- Projects are keyed by canonical path.
- Listing is ordered by `last_opened_at DESC`.
- Re-adding the same path updates the existing row.

## Task 4: Rollout Reindex And Thread Listing

**Files:**
- Create: `src/state/index_db/reindex.rs`
- Modify: `src/state/index_db/mod.rs`
- Modify: `src/state/index_db/store.rs`
- Test: `tests/index_db.rs`

- [ ] **Step 1: Write failing reindex test**

Append to `tests/index_db.rs`:

```rust
use exagent::state::rollout::{rollout_paths, RolloutItem, RolloutStore, ThreadMeta};
use exagent::types::{ConversationMessage, ThreadId};

#[tokio::test]
async fn reindex_project_discovers_rollout_threads() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let project_row = db
        .upsert_project(ProjectUpsert {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();

    let thread_id = ThreadId::new("thread_reindex_1");
    let paths = rollout_paths(&project, &thread_id);
    RolloutStore::new(paths.rollout_path.clone())
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: project.clone(),
                initial_cwd: project.clone(),
                created_at: "2026-06-01T00:00:00Z".into(),
            }),
            RolloutItem::ResponseItem(ConversationMessage::user("Design the desktop GUI")),
        ])
        .await
        .unwrap();

    let report = db.reindex_project(&project_row.id, &project).await.unwrap();
    let threads = db
        .list_threads(exagent::index_db::ThreadListFilter {
            project_id: project_row.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap();

    assert_eq!(report.indexed_threads, 1);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, thread_id);
    assert_eq!(threads[0].fallback_title, "Design the desktop GUI");
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test --test index_db reindex_project_discovers_rollout_threads
```

Expected: failure because reindexing and thread listing are missing.

- [ ] **Step 3: Expand thread record types**

Replace `ThreadRecord` in `src/state/index_db/mod.rs` with:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: crate::types::ThreadId,
    pub project_id: String,
    pub rollout_path: PathBuf,
    pub user_title: Option<String>,
    pub fallback_title: String,
    pub preview: String,
    pub archived_at: Option<i64>,
    pub pinned: bool,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ReindexReport {
    pub scanned_threads: usize,
    pub indexed_threads: usize,
    pub stale_threads: usize,
}
```

- [ ] **Step 4: Implement thread list in store**

Add to `src/state/index_db/store.rs`:

```rust
use crate::types::ThreadId;
use super::ThreadRecord;

impl IndexDb {
    pub async fn list_threads(&self, filter: ThreadListFilter) -> anyhow::Result<Vec<ThreadRecord>> {
        let mut sql = String::from(
            "SELECT id, project_id, rollout_path, user_title, fallback_title, preview, archived_at, pinned, status, created_at, updated_at FROM threads WHERE project_id = ?",
        );
        if !filter.include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        if filter.search.as_ref().is_some_and(|value| !value.trim().is_empty()) {
            sql.push_str(" AND (instr(COALESCE(user_title, ''), ?) > 0 OR instr(fallback_title, ?) > 0 OR instr(preview, ?) > 0)");
        }
        sql.push_str(" ORDER BY pinned DESC, updated_at DESC, created_at DESC");

        let mut query = sqlx::query(&sql).bind(filter.project_id);
        if let Some(search) = filter.search.filter(|value| !value.trim().is_empty()) {
            query = query.bind(search.clone()).bind(search.clone()).bind(search);
        }

        let rows = query.fetch_all(self.pool()).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ThreadRecord {
                    id: ThreadId::new(row.try_get::<String, _>("id")?),
                    project_id: row.try_get("project_id")?,
                    rollout_path: PathBuf::from(row.try_get::<String, _>("rollout_path")?),
                    user_title: row.try_get("user_title")?,
                    fallback_title: row.try_get("fallback_title")?,
                    preview: row.try_get("preview")?,
                    archived_at: row.try_get("archived_at")?,
                    pinned: row.try_get::<i64, _>("pinned")? != 0,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }
}
```

- [ ] **Step 5: Implement reindex scanner**

Create `src/state/index_db/reindex.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::state::rollout::{RolloutItem, RolloutStore};

use super::{time, IndexDb, ReindexReport};

pub async fn reindex_project(
    db: &IndexDb,
    project_id: &str,
    project_path: &Path,
) -> anyhow::Result<ReindexReport> {
    let threads_root = project_path.join(".exagent").join("threads");
    if !tokio::fs::try_exists(&threads_root).await.unwrap_or(false) {
        return Ok(ReindexReport {
            scanned_threads: 0,
            indexed_threads: 0,
            stale_threads: 0,
        });
    }

    let mut scanned_threads = 0;
    let mut indexed_threads = 0;
    let mut entries = tokio::fs::read_dir(&threads_root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let rollout_path = entry.path().join("rollout.jsonl");
        if !tokio::fs::try_exists(&rollout_path).await.unwrap_or(false) {
            continue;
        }
        scanned_threads += 1;
        if let Some(summary) = summarize_rollout(&rollout_path).await? {
            upsert_thread_summary(db, project_id, rollout_path, summary).await?;
            indexed_threads += 1;
        }
    }

    Ok(ReindexReport {
        scanned_threads,
        indexed_threads,
        stale_threads: 0,
    })
}

struct RolloutSummary {
    thread_id: crate::types::ThreadId,
    fallback_title: String,
    preview: String,
    created_at: i64,
    updated_at: i64,
}

async fn summarize_rollout(path: &Path) -> anyhow::Result<Option<RolloutSummary>> {
    let items = RolloutStore::read_items(path).await?;
    let Some(meta) = items.iter().find_map(|item| match item {
        RolloutItem::ThreadMeta(meta) => Some(meta),
        _ => None,
    }) else {
        return Ok(None);
    };
    let first_user = items.iter().find_map(|item| match item {
        RolloutItem::ResponseItem(message) if message.role == crate::types::MessageRole::User => {
            Some(message.content.clone())
        }
        _ => None,
    });
    let title = first_user
        .as_deref()
        .map(shorten_title)
        .unwrap_or_else(|| short_thread_id(meta.thread_id.as_str()));
    let preview = first_user.unwrap_or_else(|| title.clone());
    let updated_at = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_else(time::now_unix_seconds);
    Ok(Some(RolloutSummary {
        thread_id: meta.thread_id.clone(),
        fallback_title: title,
        preview,
        created_at: parse_created_at(&meta.created_at).unwrap_or(updated_at),
        updated_at,
    }))
}

fn shorten_title(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(80).collect()
}

fn short_thread_id(id: &str) -> String {
    format!("Session {}", id.chars().take(12).collect::<String>())
}

fn parse_created_at(value: &str) -> Option<i64> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

async fn upsert_thread_summary(
    db: &IndexDb,
    project_id: &str,
    rollout_path: PathBuf,
    summary: RolloutSummary,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'rollout', 0, 'idle', ?, ?)
ON CONFLICT(id) DO UPDATE SET
  project_id = excluded.project_id,
  rollout_path = excluded.rollout_path,
  fallback_title = excluded.fallback_title,
  preview = excluded.preview,
  updated_at = excluded.updated_at
        "#,
    )
    .bind(summary.thread_id.as_str())
    .bind(project_id)
    .bind(rollout_path.display().to_string())
    .bind(summary.fallback_title)
    .bind(summary.preview)
    .bind(summary.created_at)
    .bind(summary.updated_at)
    .execute(db.pool())
    .await?;
    Ok(())
}
```

Modify `src/state/index_db/mod.rs`:

```rust
mod reindex;

impl IndexDb {
    pub async fn reindex_project(
        &self,
        project_id: &str,
        project_path: &std::path::Path,
    ) -> anyhow::Result<ReindexReport> {
        reindex::reindex_project(self, project_id, project_path).await
    }
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test --test index_db reindex_project_discovers_rollout_threads
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add src/state/index_db tests/index_db.rs
git commit -m "feat: index project rollouts"
```

**Acceptance Criteria:**

- Reindex discovers project-local `.exagent/threads/*/rollout.jsonl`.
- Thread list is ordered by `pinned DESC, updated_at DESC`.
- Fallback title derives from user content or a short thread id.

## Task 5: Thread Metadata Operations

**Files:**
- Modify: `src/state/index_db/store.rs`
- Test: `tests/index_db.rs`

- [ ] **Step 1: Write failing metadata operation tests**

Append to `tests/index_db.rs`:

```rust
#[tokio::test]
async fn thread_metadata_rename_pin_archive_and_search_do_not_touch_rollout() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let project_row = db
        .upsert_project(ProjectUpsert {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();
    let thread_id = ThreadId::new("thread_metadata_1");
    let paths = rollout_paths(&project, &thread_id);
    RolloutStore::new(paths.rollout_path.clone())
        .append_items(&[
            RolloutItem::ThreadMeta(ThreadMeta {
                thread_id: thread_id.clone(),
                workspace_root: project.clone(),
                initial_cwd: project.clone(),
                created_at: "2026-06-01T00:00:00Z".into(),
            }),
            RolloutItem::ResponseItem(ConversationMessage::user("Searchable session title")),
        ])
        .await
        .unwrap();
    let before = tokio::fs::read_to_string(&paths.rollout_path).await.unwrap();
    db.reindex_project(&project_row.id, &project).await.unwrap();

    db.rename_thread(&thread_id, "Custom Title").await.unwrap();
    db.set_thread_pinned(&thread_id, true).await.unwrap();
    db.archive_thread(&thread_id).await.unwrap();
    assert!(db
        .list_threads(exagent::index_db::ThreadListFilter {
            project_id: project_row.id.clone(),
            include_archived: false,
            search: None,
        })
        .await
        .unwrap()
        .is_empty());

    db.unarchive_thread(&thread_id).await.unwrap();
    let search = db
        .list_threads(exagent::index_db::ThreadListFilter {
            project_id: project_row.id,
            include_archived: false,
            search: Some("Custom".into()),
        })
        .await
        .unwrap();
    let after = tokio::fs::read_to_string(&paths.rollout_path).await.unwrap();

    assert_eq!(search.len(), 1);
    assert_eq!(search[0].user_title.as_deref(), Some("Custom Title"));
    assert!(search[0].pinned);
    assert_eq!(before, after);
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test --test index_db thread_metadata_rename_pin_archive_and_search_do_not_touch_rollout
```

Expected: failure because metadata operations are missing.

- [ ] **Step 3: Implement metadata operations**

Add to `src/state/index_db/store.rs`:

```rust
impl IndexDb {
    pub async fn rename_thread(
        &self,
        thread_id: &crate::types::ThreadId,
        title: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET user_title = ?, title_source = 'user' WHERE id = ?")
            .bind(title.trim())
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_thread_pinned(
        &self,
        thread_id: &crate::types::ThreadId,
        pinned: bool,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET pinned = ? WHERE id = ?")
            .bind(if pinned { 1_i64 } else { 0_i64 })
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn archive_thread(&self, thread_id: &crate::types::ThreadId) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET archived_at = ? WHERE id = ?")
            .bind(time::now_unix_seconds())
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn unarchive_thread(&self, thread_id: &crate::types::ThreadId) -> anyhow::Result<()> {
        sqlx::query("UPDATE threads SET archived_at = NULL WHERE id = ?")
            .bind(thread_id.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Make search use effective title**

In `list_threads`, ensure the search condition includes `user_title`, `fallback_title`, and `preview`:

```rust
sql.push_str(" AND (instr(COALESCE(user_title, ''), ?) > 0 OR instr(fallback_title, ?) > 0 OR instr(preview, ?) > 0)");
```

- [ ] **Step 5: Run focused test**

Run:

```bash
cargo test --test index_db thread_metadata_rename_pin_archive_and_search_do_not_touch_rollout
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/state/index_db tests/index_db.rs
git commit -m "feat: manage desktop thread metadata"
```

**Acceptance Criteria:**

- Rename, pin, archive, and unarchive mutate SQLite only.
- Rollout file content stays byte-for-byte unchanged.
- Archived threads are hidden unless `include_archived` is true.
- Search matches user title, fallback title, and preview.

## Task 6: Desktop Facade In Root Crate

**Files:**
- Create: `src/app_server/desktop_facade.rs`
- Modify: `src/app_server/mod.rs`
- Test: `tests/desktop_facade.rs`

- [ ] **Step 1: Write failing facade test**

Create `tests/desktop_facade.rs`:

```rust
use exagent::app_server::desktop_facade::{DesktopFacade, NewProjectRequest};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::index_db::IndexDb;
use exagent::llm::MockLlm;
use exagent::registry::ToolRegistry;
use tempfile::tempdir;

#[tokio::test]
async fn desktop_facade_adds_project_and_starts_thread() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );
    let facade = DesktopFacade::new(service, db);

    let project_record = facade
        .add_project(NewProjectRequest {
            name: "Project".into(),
            path: project.clone(),
        })
        .await
        .unwrap();
    let started = facade.start_thread(&project_record.id).await.unwrap();

    assert_eq!(project_record.path, project.canonicalize().unwrap());
    assert_eq!(started.thread.turns.len(), 0);
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test --test desktop_facade desktop_facade_adds_project_and_starts_thread
```

Expected: failure because `desktop_facade` is missing.

- [ ] **Step 3: Implement desktop facade skeleton**

Create `src/app_server/desktop_facade.rs`:

```rust
use std::path::PathBuf;

use anyhow::{anyhow, Result};

use crate::app_server::protocol::{ThreadStartParams, ThreadStartResponse};
use crate::app_server::AppServerService;
use crate::index_db::{IndexDb, ProjectRecord, ProjectUpsert};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectRequest {
    pub name: String,
    pub path: PathBuf,
}

pub struct DesktopFacade {
    service: AppServerService,
    index: IndexDb,
}

impl DesktopFacade {
    pub fn new(service: AppServerService, index: IndexDb) -> Self {
        Self { service, index }
    }

    pub async fn add_project(&self, request: NewProjectRequest) -> Result<ProjectRecord> {
        self.index
            .upsert_project(ProjectUpsert {
                name: request.name,
                path: request.path,
            })
            .await
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        self.index.list_projects().await
    }

    pub async fn start_thread(&self, project_id: &str) -> Result<ThreadStartResponse> {
        let project = self
            .index
            .list_projects()
            .await?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| anyhow!("unknown project id: {project_id}"))?;
        let response = self.service.thread_start(ThreadStartParams {
            workspace_root: Some(project.path.display().to_string()),
            cwd: Some(project.path.display().to_string()),
        })?;
        self.index.reindex_project(&project.id, &project.path).await?;
        Ok(response)
    }
}
```

Modify `src/app_server/mod.rs`:

```rust
pub mod desktop_facade;
```

- [ ] **Step 4: Run focused facade test**

Run:

```bash
cargo test --test desktop_facade desktop_facade_adds_project_and_starts_thread
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/app_server/desktop_facade.rs src/app_server/mod.rs tests/desktop_facade.rs
git commit -m "feat: add desktop app facade"
```

**Acceptance Criteria:**

- Desktop facade lives in root crate and has no Tauri dependency.
- Starting a thread through the facade uses `AppServerService`.
- Project paths are passed as `workspace_root` and `cwd`.

## Task 7: Approval Decision Boundary

**Files:**
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/service.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Test: `tests/approval_decision.rs`

**Design constraint:** The GUI must not call `RunCommandTool` directly. It submits an approval decision to the app-server boundary. The runtime records `ApprovalDecision` and clears pending approval state. If the approved command is one-shot, the first implementation may execute it directly and record a `ToolResult` event; if resuming the exact model tool loop requires deeper changes, keep that as a follow-up and make the UI show the command execution result as the approved action result.

- [ ] **Step 1: Write failing approval decision test**

Create `tests/approval_decision.rs`:

```rust
use exagent::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionStatus, ThreadStartParams, TurnStartParams,
};
use exagent::app_server::AppServerService;
use exagent::config::AgentConfig;
use exagent::events::RuntimeEventKind;
use exagent::llm::MockLlm;
use exagent::policy::PolicyMode;
use exagent::registry::ToolRegistry;
use exagent::tools::run_command::RunCommandTool;
use exagent::types::{AssistantTurn, ToolCall};
use tempfile::tempdir;

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RunCommandTool);
    registry
}

#[tokio::test]
async fn approval_decision_clears_waiting_approval() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            policy_mode: PolicyMode::Enforced,
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("request approval".into()),
            tool_calls: vec![ToolCall {
                id: "call_risky".into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "rm -rf scratch" }),
            }],
        }])),
        registry,
    );
    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap()
        .thread;
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.id.clone(),
            prompt: "try risky command".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap()
        .turn;

    let approval_id = loop {
        let replay = service
            .events_replay(exagent::app_server::protocol::EventsReplayParams {
                thread_id: thread.id.clone(),
                workspace_root: None,
                after_event_id: None,
                limit: None,
                include_snapshot: false,
                event_kinds: vec![],
            })
            .unwrap();
        if let Some(id) = replay.events.iter().find_map(|event| match &event.kind {
            RuntimeEventKind::ApprovalRequested { approval_id, .. } => Some(approval_id.clone()),
            _ => None,
        }) {
            break id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

    service
        .approval_decision(ApprovalDecisionParams {
            thread_id: thread.id.clone(),
            turn_id: Some(turn.id),
            approval_id,
            decision: ApprovalDecisionStatus::Denied,
            note: Some("desktop denied".into()),
            workspace_root: None,
        })
        .await
        .unwrap();

    let read = service
        .thread_read(exagent::app_server::protocol::ThreadReadParams {
            thread_id: thread.id,
            workspace_root: None,
        })
        .unwrap();
    assert_ne!(
        read.thread.status,
        exagent::app_server::protocol::ThreadStatus::WaitingApproval
    );
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test --test approval_decision approval_decision_clears_waiting_approval
```

Expected: failure because approval decision API is missing.

- [ ] **Step 3: Add protocol types**

Add to `src/app_server/protocol.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionStatus {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalDecisionParams {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub approval_id: crate::session::ApprovalId,
    pub decision: ApprovalDecisionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub workspace_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalDecisionResponse {
    pub thread_id: ThreadId,
    pub approval_id: crate::session::ApprovalId,
    pub status: ApprovalDecisionStatus,
}
```

- [ ] **Step 4: Add AppServerService method**

Extend `AppServerBoundary` and `AppServerService` in `src/app_server/service.rs`:

```rust
async fn approval_decision(
    &self,
    params: ApprovalDecisionParams,
) -> Result<ApprovalDecisionResponse>;
```

Delegate to `ThreadManager`.

- [ ] **Step 5: Implement ThreadManager decision path**

Add `ThreadManager::approval_decision` that:

1. Resolves workspace using existing override policy.
2. Resolves loaded runtime for the thread.
3. Rejects missing thread.
4. Calls a runtime method that clears the pending approval and records `ApprovalDecision`.

Expected decision mapping:

```rust
let status = match params.decision {
    ApprovalDecisionStatus::Approved => crate::session::ApprovalStatus::Approved,
    ApprovalDecisionStatus::Denied => crate::session::ApprovalStatus::Denied,
};
```

- [ ] **Step 6: Implement runtime/session approval decision operation**

Add a runtime control operation similar to `Interrupt`:

```rust
ThreadOp::ApprovalDecision {
    turn_id: Option<TurnId>,
    approval_id: ApprovalId,
    status: ApprovalStatus,
    note: Option<String>,
}
```

In `ThreadSession`, validate that the approval exists in the overlay, clear it, cancel policy pending command for denied decisions, and append:

```rust
RuntimeEventKind::ApprovalDecision {
    approval_id,
    status,
    note,
}
```

- [ ] **Step 7: Run focused approval test**

Run:

```bash
cargo test --test approval_decision approval_decision_clears_waiting_approval
```

Expected: pass.

- [ ] **Step 8: Add serialization test**

Add to `tests/api_server.rs` or `tests/app_server_boundary.rs`:

```rust
#[test]
fn approval_decision_params_deserialize_snake_case_status() {
    let value = serde_json::json!({
        "thread_id": "thread_1",
        "approval_id": "approval_1",
        "decision": "denied",
        "workspace_root": "."
    });
    let params: exagent::app_server::protocol::ApprovalDecisionParams =
        serde_json::from_value(value).unwrap();
    assert!(matches!(
        params.decision,
        exagent::app_server::protocol::ApprovalDecisionStatus::Denied
    ));
}
```

- [ ] **Step 9: Commit**

```bash
git add src/app_server src/runtime tests/approval_decision.rs tests/api_server.rs
git commit -m "feat: add approval decision boundary"
```

**Acceptance Criteria:**

- Desktop can submit approval decisions without calling tools directly.
- Deny clears waiting approval and records `ApprovalDecision`.
- Approval decision route is typed and test-covered.
- If approved command execution is not fully resumed in this task, the limitation is documented in the GUI with a visible status and tracked as a follow-up decision before shipping a "full approval execution" claim.

## Task 8: Tauri Desktop Scaffold

**Files:**
- Create: `apps/desktop/package.json`
- Create: `apps/desktop/index.html`
- Create: `apps/desktop/tsconfig.json`
- Create: `apps/desktop/vite.config.ts`
- Create: `apps/desktop/src/main.tsx`
- Create: `apps/desktop/src/App.tsx`
- Create: `apps/desktop/src/styles.css`
- Create: `apps/desktop/src-tauri/Cargo.toml`
- Create: `apps/desktop/src-tauri/tauri.conf.json`
- Create: `apps/desktop/src-tauri/src/main.rs`
- Create: `apps/desktop/src-tauri/src/lib.rs`

- [ ] **Step 1: Create desktop package**

Create `apps/desktop/package.json`:

```json
{
  "name": "exagent-desktop",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "test": "vitest run",
    "tauri": "tauri",
    "tauri:dev": "tauri dev",
    "tauri:build": "tauri build"
  },
  "dependencies": {
    "@radix-ui/react-dialog": "^1.1.0",
    "@radix-ui/react-tooltip": "^1.1.0",
    "@tanstack/react-query": "^5.0.0",
    "@tauri-apps/api": "^2.0.0",
    "@tauri-apps/plugin-dialog": "^2.0.0",
    "lucide-react": "^0.468.0",
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "zustand": "^5.0.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.0.0",
    "@testing-library/jest-dom": "^6.0.0",
    "@testing-library/react": "^16.0.0",
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.0.0",
    "typescript": "^5.0.0",
    "vite": "^6.0.0",
    "vitest": "^2.0.0"
  }
}
```

- [ ] **Step 2: Create Vite files**

Create `apps/desktop/index.html`:

```html
<div id="root"></div>
<script type="module" src="/src/main.tsx"></script>
```

Create `apps/desktop/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "useDefineForClassFields": true,
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "allowJs": false,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "allowSyntheticDefaultImports": true,
    "strict": true,
    "forceConsistentCasingInFileNames": true,
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx"
  },
  "include": ["src"]
}
```

Create `apps/desktop/vite.config.ts`:

```ts
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true
  }
});
```

- [ ] **Step 3: Create minimal React app**

Create `apps/desktop/src/main.tsx`:

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import "./styles.css";

const queryClient = new QueryClient();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>
);
```

Create `apps/desktop/src/App.tsx`:

```tsx
export default function App() {
  return <main className="app-shell">ExAgent Desktop</main>;
}
```

Create `apps/desktop/src/styles.css`:

```css
:root {
  color-scheme: dark;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: #101010;
  color: #f2f2f2;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 320px;
  min-height: 100vh;
  background: #101010;
}

.app-shell {
  min-height: 100vh;
  display: grid;
  place-items: center;
}
```

- [ ] **Step 4: Create Tauri crate**

Create `apps/desktop/src-tauri/Cargo.toml`:

```toml
[package]
name = "exagent-desktop"
version = "0.1.0"
edition = "2021"

[lib]
name = "exagent_desktop"
crate-type = ["staticlib", "cdylib", "rlib"]

[dependencies]
anyhow = "1"
exagent = { path = "../../.." }
serde = { version = "1", features = ["derive"] }
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"
tokio = { version = "1", features = ["full"] }
```

Create `apps/desktop/src-tauri/tauri.conf.json`:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "ExAgent Desktop",
  "version": "0.1.0",
  "identifier": "dev.exagent.desktop",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "ExAgent",
        "width": 1280,
        "height": 860,
        "minWidth": 900,
        "minHeight": 620
      }
    ]
  },
  "bundle": {
    "active": true,
    "targets": "all"
  }
}
```

Create `apps/desktop/src-tauri/src/main.rs`:

```rust
fn main() {
    exagent_desktop::run();
}
```

Create `apps/desktop/src-tauri/src/lib.rs`:

```rust
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("failed to run ExAgent Desktop");
}
```

- [ ] **Step 5: Install dependencies and verify frontend build**

Run:

```bash
cd apps/desktop
npm install
npm run build
```

Expected: Vite build succeeds and writes `apps/desktop/dist`.

- [ ] **Step 6: Verify Tauri crate compiles**

Run:

```bash
cargo check -p exagent-desktop
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock apps/desktop
git commit -m "feat: scaffold exagent desktop app"
```

**Acceptance Criteria:**

- `apps/desktop` builds with Vite.
- `exagent-desktop` crate compiles.
- Tauri app runs a minimal window without starting HTTP API.

## Task 9: Tauri Commands And Channel Bridge

**Files:**
- Create: `apps/desktop/src-tauri/src/state.rs`
- Create: `apps/desktop/src-tauri/src/commands.rs`
- Create: `apps/desktop/src-tauri/src/events.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Test: `apps/desktop/src-tauri/src/commands.rs`

- [ ] **Step 1: Add Tauri state**

Create `apps/desktop/src-tauri/src/state.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use exagent::app_server::desktop_facade::DesktopFacade;
use exagent::app_server::AppServerService;
use exagent::index_db::IndexDb;
use tauri::Manager;
use tokio::sync::Mutex;

pub struct DesktopState {
    facade: Arc<DesktopFacade>,
}

impl DesktopState {
    pub async fn new(app: &tauri::App) -> anyhow::Result<Self> {
        let data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from(".exagent-desktop"));
        let db = IndexDb::open(data_dir.join("exagent.sqlite")).await?;
        Ok(Self {
            facade: Arc::new(DesktopFacade::new(AppServerService::new(), db)),
        })
    }

    pub fn facade(&self) -> Arc<DesktopFacade> {
        self.facade.clone()
    }
}

pub type ManagedDesktopState = Mutex<Option<DesktopState>>;
```

- [ ] **Step 2: Add command module**

Create `apps/desktop/src-tauri/src/commands.rs`:

```rust
use std::path::PathBuf;

use exagent::app_server::desktop_facade::NewProjectRequest;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::ManagedDesktopState;

#[derive(Debug, Deserialize)]
pub struct AddProjectRequest {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct CommandError {
    pub message: String,
}

impl From<anyhow::Error> for CommandError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

#[tauri::command]
pub async fn project_add(
    state: State<'_, ManagedDesktopState>,
    request: AddProjectRequest,
) -> Result<exagent::index_db::ProjectRecord, CommandError> {
    let guard = state.lock().await;
    let desktop = guard.as_ref().ok_or_else(|| CommandError {
        message: "desktop state is not initialized".into(),
    })?;
    desktop
        .facade()
        .add_project(NewProjectRequest {
            name: request.name,
            path: request.path,
        })
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn project_list(
    state: State<'_, ManagedDesktopState>,
) -> Result<Vec<exagent::index_db::ProjectRecord>, CommandError> {
    let guard = state.lock().await;
    let desktop = guard.as_ref().ok_or_else(|| CommandError {
        message: "desktop state is not initialized".into(),
    })?;
    desktop.facade().list_projects().await.map_err(CommandError::from)
}
```

- [ ] **Step 3: Wire commands in Tauri builder**

Modify `apps/desktop/src-tauri/src/lib.rs`:

```rust
mod commands;
mod events;
mod state;

use state::{DesktopState, ManagedDesktopState};
use tokio::sync::Mutex;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ManagedDesktopState::new(None))
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                let desktop = DesktopState::new(app).await?;
                *handle.state::<ManagedDesktopState>().lock().await = Some(desktop);
                Ok::<(), anyhow::Error>(())
            })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project_add,
            commands::project_list
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ExAgent Desktop");
}
```

- [ ] **Step 4: Add event bridge type**

Create `apps/desktop/src-tauri/src/events.rs`:

```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopEvent {
    Runtime {
        event: exagent::events::RuntimeEvent,
    },
    Error {
        message: String,
    },
}
```

- [ ] **Step 5: Verify Tauri crate compiles**

Run:

```bash
cargo check -p exagent-desktop
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri
git commit -m "feat: add tauri desktop commands"
```

**Acceptance Criteria:**

- Tauri app owns app data DB initialization.
- Commands call root-crate services.
- Command errors serialize into user-visible messages.
- Event bridge has a typed desktop event envelope.

## Task 10: Frontend Client And Store

**Files:**
- Create: `apps/desktop/src/types.ts`
- Create: `apps/desktop/src/api/exagentClient.ts`
- Create: `apps/desktop/src/stores/workbenchStore.ts`
- Test: `apps/desktop/src/api/exagentClient.test.ts`

- [ ] **Step 1: Define frontend types**

Create `apps/desktop/src/types.ts`:

```ts
export type ProjectRecord = {
  id: string;
  name: string;
  path: string;
};

export type ThreadRecord = {
  id: string;
  project_id: string;
  rollout_path: string;
  user_title: string | null;
  fallback_title: string;
  preview: string;
  archived_at: number | null;
  pinned: boolean;
  status: string;
  created_at: number;
  updated_at: number;
};

export type RuntimeEvent = {
  event_id: string;
  thread_id: string;
  turn_id?: string;
  kind: { type: string; [key: string]: unknown };
};
```

- [ ] **Step 2: Create invoke client**

Create `apps/desktop/src/api/exagentClient.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";
import type { ProjectRecord } from "../types";

export async function listProjects(): Promise<ProjectRecord[]> {
  return invoke<ProjectRecord[]>("project_list");
}

export async function addProject(input: {
  name: string;
  path: string;
}): Promise<ProjectRecord> {
  return invoke<ProjectRecord>("project_add", {
    request: input
  });
}

export function titleForThread(thread: {
  user_title: string | null;
  fallback_title: string;
  id: string;
}): string {
  return thread.user_title?.trim() || thread.fallback_title || thread.id.slice(0, 12);
}
```

- [ ] **Step 3: Create store**

Create `apps/desktop/src/stores/workbenchStore.ts`:

```ts
import { create } from "zustand";
import type { ProjectRecord, ThreadRecord } from "../types";

type WorkbenchState = {
  selectedProjectId: string | null;
  selectedThreadId: string | null;
  projects: ProjectRecord[];
  threads: ThreadRecord[];
  inspectorOpen: boolean;
  setProjects(projects: ProjectRecord[]): void;
  setThreads(threads: ThreadRecord[]): void;
  selectProject(projectId: string): void;
  selectThread(threadId: string): void;
  setInspectorOpen(open: boolean): void;
};

export const useWorkbenchStore = create<WorkbenchState>((set) => ({
  selectedProjectId: null,
  selectedThreadId: null,
  projects: [],
  threads: [],
  inspectorOpen: true,
  setProjects: (projects) => set({ projects }),
  setThreads: (threads) => set({ threads }),
  selectProject: (selectedProjectId) => set({ selectedProjectId, selectedThreadId: null }),
  selectThread: (selectedThreadId) => set({ selectedThreadId }),
  setInspectorOpen: (inspectorOpen) => set({ inspectorOpen })
}));
```

- [ ] **Step 4: Add pure helper test**

Create `apps/desktop/src/api/exagentClient.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { titleForThread } from "./exagentClient";

describe("titleForThread", () => {
  it("prefers user title over fallback", () => {
    expect(
      titleForThread({
        user_title: "Custom",
        fallback_title: "Fallback",
        id: "thread_abcdef"
      })
    ).toBe("Custom");
  });

  it("falls back to short id", () => {
    expect(
      titleForThread({
        user_title: null,
        fallback_title: "",
        id: "thread_abcdefghijkl"
      })
    ).toBe("thread_abcde");
  });
});
```

- [ ] **Step 5: Run frontend tests**

Run:

```bash
cd apps/desktop
npm test
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src
git commit -m "feat: add desktop frontend client state"
```

**Acceptance Criteria:**

- Frontend has a typed Tauri command client.
- Frontend has a minimal workbench store.
- Title fallback behavior is test-covered.

## Task 11: Workbench Layout

**Files:**
- Create: `apps/desktop/src/components/AppShell.tsx`
- Create: `apps/desktop/src/components/Sidebar.tsx`
- Create: `apps/desktop/src/components/ChatView.tsx`
- Create: `apps/desktop/src/components/Composer.tsx`
- Create: `apps/desktop/src/components/Inspector.tsx`
- Modify: `apps/desktop/src/App.tsx`
- Modify: `apps/desktop/src/styles.css`
- Test: `apps/desktop/src/components/AppShell.test.tsx`

- [ ] **Step 1: Add layout component test**

Create `apps/desktop/src/components/AppShell.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppShell } from "./AppShell";

describe("AppShell", () => {
  it("renders core workbench regions", () => {
    render(<AppShell />);
    expect(screen.getByRole("button", { name: /new chat/i })).toBeTruthy();
    expect(screen.getByPlaceholderText(/search sessions/i)).toBeTruthy();
    expect(screen.getByLabelText(/chat transcript/i)).toBeTruthy();
    expect(screen.getByLabelText(/runtime inspector/i)).toBeTruthy();
  });
});
```

- [ ] **Step 2: Create layout components**

Create `apps/desktop/src/components/AppShell.tsx`:

```tsx
import { ChatView } from "./ChatView";
import { Inspector } from "./Inspector";
import { Sidebar } from "./Sidebar";

export function AppShell() {
  return (
    <div className="workbench">
      <Sidebar />
      <ChatView />
      <Inspector />
    </div>
  );
}
```

Create `apps/desktop/src/components/Sidebar.tsx`:

```tsx
import { Plus, Search } from "lucide-react";

export function Sidebar() {
  return (
    <aside className="sidebar">
      <button className="sidebar-command" type="button">
        <Plus size={16} />
        <span>New Chat</span>
      </button>
      <label className="search-box">
        <Search size={15} />
        <input placeholder="Search sessions" />
      </label>
      <section>
        <h2>Projects</h2>
        <div className="empty-row">No project selected</div>
      </section>
      <section>
        <h2>Sessions</h2>
        <div className="empty-row">No sessions yet</div>
      </section>
    </aside>
  );
}
```

Create `apps/desktop/src/components/ChatView.tsx`:

```tsx
import { Composer } from "./Composer";

export function ChatView() {
  return (
    <main className="chat">
      <header className="chat-header">
        <strong>ExAgent</strong>
        <span>Choose a project to begin</span>
      </header>
      <section className="transcript" aria-label="Chat transcript">
        <div className="empty-state">Start or resume a session.</div>
      </section>
      <Composer />
    </main>
  );
}
```

Create `apps/desktop/src/components/Composer.tsx`:

```tsx
import { ArrowUp } from "lucide-react";

export function Composer() {
  return (
    <form className="composer">
      <textarea aria-label="Prompt" placeholder="Ask ExAgent" rows={3} />
      <button type="submit" aria-label="Send prompt">
        <ArrowUp size={18} />
      </button>
    </form>
  );
}
```

Create `apps/desktop/src/components/Inspector.tsx`:

```tsx
export function Inspector() {
  return (
    <aside className="inspector" aria-label="Runtime inspector">
      <section>
        <h2>Progress</h2>
        <p>Idle</p>
      </section>
      <section>
        <h2>Environment</h2>
        <p>No project selected</p>
      </section>
      <section>
        <h2>Changed Files</h2>
        <p>No changes</p>
      </section>
    </aside>
  );
}
```

Modify `apps/desktop/src/App.tsx`:

```tsx
import { AppShell } from "./components/AppShell";

export default function App() {
  return <AppShell />;
}
```

- [ ] **Step 3: Add responsive CSS**

Replace `apps/desktop/src/styles.css` with focused workbench styles:

```css
:root {
  color-scheme: dark;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: #101010;
  color: #f2f2f2;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 320px;
  min-height: 100vh;
  background: #101010;
}

button,
input,
textarea {
  font: inherit;
}

.workbench {
  min-height: 100vh;
  display: grid;
  grid-template-columns: 280px minmax(0, 1fr) 320px;
  background: #101010;
}

.sidebar,
.inspector {
  background: #191919;
  border-color: #2a2a2a;
  padding: 14px;
  min-width: 0;
}

.sidebar {
  border-right: 1px solid #2a2a2a;
}

.inspector {
  border-left: 1px solid #2a2a2a;
}

.sidebar h2,
.inspector h2 {
  margin: 18px 0 8px;
  font-size: 12px;
  color: #9b9b9b;
  text-transform: uppercase;
  letter-spacing: 0;
}

.sidebar-command,
.composer button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border: 1px solid #3a3a3a;
  border-radius: 8px;
  color: #f2f2f2;
  background: #242424;
  min-height: 36px;
}

.sidebar-command {
  width: 100%;
}

.search-box {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  border: 1px solid #303030;
  border-radius: 8px;
  padding: 0 10px;
  min-height: 36px;
  color: #9b9b9b;
}

.search-box input {
  min-width: 0;
  flex: 1;
  color: #f2f2f2;
  background: transparent;
  border: 0;
  outline: 0;
}

.chat {
  min-width: 0;
  display: grid;
  grid-template-rows: 52px minmax(0, 1fr) auto;
}

.chat-header {
  border-bottom: 1px solid #2a2a2a;
  padding: 0 18px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.chat-header span,
.empty-row,
.empty-state,
.inspector p {
  color: #9b9b9b;
}

.transcript {
  min-height: 0;
  overflow: auto;
  padding: 24px;
}

.composer {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 40px;
  gap: 10px;
  border-top: 1px solid #2a2a2a;
  padding: 14px 18px;
  background: #141414;
}

.composer textarea {
  width: 100%;
  resize: vertical;
  border: 1px solid #3a3a3a;
  border-radius: 10px;
  padding: 12px;
  color: #f2f2f2;
  background: #202020;
}

@media (max-width: 1199px) {
  .workbench {
    grid-template-columns: 260px minmax(0, 1fr);
  }

  .inspector {
    display: none;
  }
}

@media (max-width: 899px) {
  .workbench {
    grid-template-columns: minmax(0, 1fr);
  }

  .sidebar {
    display: none;
  }
}
```

- [ ] **Step 4: Run frontend tests and build**

Run:

```bash
cd apps/desktop
npm test
npm run build
```

Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src
git commit -m "feat: build desktop workbench layout"
```

**Acceptance Criteria:**

- Layout has sidebar, chat, composer, and inspector.
- Inspector hides below 1200px.
- Sidebar hides below 900px.
- Tests verify core regions render.

## Task 12: Chat Event Rendering

**Files:**
- Create: `apps/desktop/src/components/TurnItem.tsx`
- Create: `apps/desktop/src/components/ApprovalCard.tsx`
- Create: `apps/desktop/src/components/ChangedFiles.tsx`
- Modify: `apps/desktop/src/components/ChatView.tsx`
- Modify: `apps/desktop/src/components/Inspector.tsx`
- Test: `apps/desktop/src/components/TurnItem.test.tsx`

- [ ] **Step 1: Add event rendering tests**

Create `apps/desktop/src/components/TurnItem.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TurnItem } from "./TurnItem";

describe("TurnItem", () => {
  it("renders assistant text", () => {
    render(<TurnItem event={{ type: "assistant_turn", turn: { text: "Hello" } }} />);
    expect(screen.getByText("Hello")).toBeTruthy();
  });

  it("renders approval actions", () => {
    render(
      <TurnItem
        event={{
          type: "approval_requested",
          approval_id: "approval_1",
          tool_name: "run_command",
          reason: "risky command"
        }}
      />
    );
    expect(screen.getByRole("button", { name: /approve/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /deny/i })).toBeTruthy();
  });
});
```

- [ ] **Step 2: Implement TurnItem**

Create `apps/desktop/src/components/TurnItem.tsx`:

```tsx
import { ApprovalCard } from "./ApprovalCard";

type EventKind = {
  type: string;
  [key: string]: unknown;
};

export function TurnItem({ event }: { event: EventKind }) {
  if (event.type === "assistant_turn") {
    const turn = event.turn as { text?: string | null };
    return <article className="turn-item assistant">{turn.text ?? ""}</article>;
  }

  if (event.type === "tool_result") {
    const result = event.result as { tool_name?: string; content?: string };
    return (
      <details className="turn-item tool">
        <summary>{result.tool_name ?? "tool"} completed</summary>
        <pre>{result.content ?? ""}</pre>
      </details>
    );
  }

  if (event.type === "exec_output") {
    return (
      <details className="turn-item tool">
        <summary>Command output</summary>
        <pre>{String(event.chunk ?? "")}</pre>
      </details>
    );
  }

  if (event.type === "approval_requested") {
    return (
      <ApprovalCard
        approvalId={String(event.approval_id)}
        toolName={String(event.tool_name)}
        reason={String(event.reason)}
      />
    );
  }

  if (event.type === "runtime_error") {
    return <article className="turn-item error">{String(event.message ?? "Runtime error")}</article>;
  }

  return null;
}
```

Create `apps/desktop/src/components/ApprovalCard.tsx`:

```tsx
import { Check, X } from "lucide-react";

export function ApprovalCard({
  approvalId,
  toolName,
  reason
}: {
  approvalId: string;
  toolName: string;
  reason: string;
}) {
  return (
    <article className="turn-item approval">
      <header>
        <strong>{toolName}</strong>
        <span>{approvalId}</span>
      </header>
      <p>{reason}</p>
      <div className="approval-actions">
        <button type="button" aria-label="Approve">
          <Check size={16} />
          <span>Approve</span>
        </button>
        <button type="button" aria-label="Deny">
          <X size={16} />
          <span>Deny</span>
        </button>
      </div>
    </article>
  );
}
```

Create `apps/desktop/src/components/ChangedFiles.tsx`:

```tsx
export function ChangedFiles({ files }: { files: string[] }) {
  if (files.length === 0) {
    return <p>No changes</p>;
  }
  return (
    <ul className="changed-files">
      {files.map((file) => (
        <li key={file}>{file}</li>
      ))}
    </ul>
  );
}
```

- [ ] **Step 3: Add styles**

Append to `apps/desktop/src/styles.css`:

```css
.turn-item {
  max-width: 760px;
  border: 1px solid #303030;
  border-radius: 8px;
  padding: 12px;
  margin-bottom: 12px;
  background: #171717;
}

.turn-item.assistant {
  border-color: transparent;
  background: transparent;
  line-height: 1.6;
}

.turn-item.tool pre {
  overflow: auto;
  max-height: 280px;
  white-space: pre-wrap;
  color: #cfcfcf;
}

.turn-item.error {
  border-color: #7f3535;
  background: #241414;
}

.turn-item.approval {
  border-color: #8a6a2a;
  background: #211b10;
}

.turn-item.approval header,
.approval-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.approval-actions {
  justify-content: flex-start;
}

.approval-actions button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid #4a4a4a;
  border-radius: 8px;
  padding: 7px 10px;
  color: #f2f2f2;
  background: #242424;
}

.changed-files {
  list-style: none;
  padding: 0;
  margin: 8px 0 0;
}

.changed-files li {
  padding: 6px 0;
  border-bottom: 1px solid #282828;
  color: #cfcfcf;
}
```

- [ ] **Step 4: Run frontend tests**

Run:

```bash
cd apps/desktop
npm test
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src
git commit -m "feat: render desktop runtime events"
```

**Acceptance Criteria:**

- Assistant messages render inline.
- Tool and exec output render as expandable details.
- Approval cards render Approve and Deny controls.
- Runtime errors are visible.

## Task 13: End-To-End Desktop Flow

**Files:**
- Modify: `src/app_server/desktop_facade.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src/api/exagentClient.ts`
- Modify: `apps/desktop/src/components/Sidebar.tsx`
- Modify: `apps/desktop/src/components/Composer.tsx`
- Modify: `apps/desktop/src/components/ChatView.tsx`
- Test: `tests/desktop_facade.rs`
- Test: `apps/desktop/src/App.test.tsx`

- [ ] **Step 1: Add facade tests for listing threads and starting turns**

Append to `tests/desktop_facade.rs`:

```rust
#[tokio::test]
async fn desktop_facade_lists_threads_after_start() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![])),
        ToolRegistry::new,
    );
    let facade = DesktopFacade::new(service, db);
    let project_record = facade
        .add_project(NewProjectRequest {
            name: "Project".into(),
            path: project,
        })
        .await
        .unwrap();

    facade.start_thread(&project_record.id).await.unwrap();
    let threads = facade.list_threads(&project_record.id, false, None).await.unwrap();

    assert_eq!(threads.len(), 1);
}
```

- [ ] **Step 2: Implement facade list/resume/read/turn methods**

Add methods to `src/app_server/desktop_facade.rs`:

```rust
pub async fn list_threads(
    &self,
    project_id: &str,
    include_archived: bool,
    search: Option<String>,
) -> Result<Vec<crate::index_db::ThreadRecord>> {
    self.index
        .list_threads(crate::index_db::ThreadListFilter {
            project_id: project_id.to_string(),
            include_archived,
            search,
        })
        .await
}
```

Add `thread_read`, `thread_resume`, `turn_start`, `rename_thread`, `archive_thread`, `pin_thread` wrappers that call `AppServerService` and `IndexDb`.

- [ ] **Step 3: Wire Tauri commands to real facade methods**

Replace deliberate error bodies in `apps/desktop/src-tauri/src/commands.rs` with calls to `desktop.facade()`.

Add commands:

```rust
thread_list
thread_start
thread_resume
thread_rename
thread_pin
thread_archive
thread_unarchive
approval_decision
```

- [ ] **Step 4: Extend frontend client**

Add to `apps/desktop/src/api/exagentClient.ts`:

```ts
export async function listThreads(input: {
  projectId: string;
  includeArchived?: boolean;
  search?: string | null;
}) {
  return invoke("thread_list", { request: input });
}

export async function startThread(projectId: string) {
  return invoke("thread_start", { projectId });
}

export async function startTurn(input: {
  thread_id: string;
  prompt: string;
  workspace_root?: string | null;
}) {
  return invoke("turn_start", { params: input });
}
```

- [ ] **Step 5: Connect Sidebar and Composer**

`Sidebar` should:

- Load projects through `listProjects`.
- Load threads for selected project through `listThreads`.
- Call `startThread` on New Chat.
- Filter sessions with the search box.

`Composer` should:

- Keep prompt draft locally.
- Disable send when no selected thread or prompt is empty.
- Call `startTurn`.
- Clear draft after accepted turn response.

- [ ] **Step 6: Add app integration test**

Create `apps/desktop/src/App.test.tsx` with mocked client functions:

```tsx
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import App from "./App";

describe("App", () => {
  it("renders the workbench shell", () => {
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <App />
      </QueryClientProvider>
    );
    expect(screen.getByRole("button", { name: /new chat/i })).toBeTruthy();
  });
});
```

- [ ] **Step 7: Run tests**

Run:

```bash
cargo test --test desktop_facade
cd apps/desktop && npm test && npm run build
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src/app_server/desktop_facade.rs apps/desktop/src-tauri apps/desktop/src tests/desktop_facade.rs
git commit -m "feat: connect desktop workbench flow"
```

**Acceptance Criteria:**

- Desktop can add/list projects.
- Desktop can list sessions for selected project.
- Desktop can start a new thread.
- Desktop can submit a turn.
- Rename, pin, archive, and unarchive are exposed to the frontend.

## Task 14: Runtime Event Subscription In Desktop

**Files:**
- Modify: `src/app_server/desktop_facade.rs`
- Modify: `apps/desktop/src-tauri/src/events.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src/api/exagentClient.ts`
- Modify: `apps/desktop/src/components/ChatView.tsx`
- Test: `tests/desktop_facade.rs`

- [ ] **Step 1: Add facade event subscription test**

Append to `tests/desktop_facade.rs`:

```rust
#[tokio::test]
async fn desktop_facade_subscribes_to_runtime_events() {
    let dir = tempdir().unwrap();
    let project = dir.path().join("project");
    tokio::fs::create_dir_all(&project).await.unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let service = AppServerService::with_llm(
        AgentConfig::default(),
        Box::new(MockLlm::new(vec![exagent::types::AssistantTurn {
            text: Some("hello desktop".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );
    let facade = DesktopFacade::new(service, db);
    let project_record = facade
        .add_project(NewProjectRequest {
            name: "Project".into(),
            path: project,
        })
        .await
        .unwrap();
    let started = facade.start_thread(&project_record.id).await.unwrap();
    let mut rx = facade.subscribe_events(&started.thread.id, None).await.unwrap();

    facade
        .turn_start(exagent::app_server::protocol::TurnStartParams {
            thread_id: started.thread.id.clone(),
            prompt: "say hi".into(),
            workspace_root: None,
            turn_context: None,
        })
        .await
        .unwrap();

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.thread_id, started.thread.id);
}
```

- [ ] **Step 2: Implement facade subscription wrapper**

Add to `DesktopFacade`:

```rust
pub async fn subscribe_events(
    &self,
    thread_id: &crate::types::ThreadId,
    after_event_id: Option<crate::types::EventId>,
) -> Result<tokio::sync::broadcast::Receiver<crate::events::RuntimeEvent>> {
    self.service.events_subscribe(crate::app_server::protocol::EventsSubscribeParams {
        thread_id: thread_id.clone(),
        workspace_root: None,
        after_event_id,
    })
}
```

- [ ] **Step 3: Add Tauri channel command**

Use Tauri's `tauri::ipc::Channel`:

```rust
#[tauri::command]
pub async fn events_subscribe(
    state: State<'_, ManagedDesktopState>,
    thread_id: String,
    on_event: tauri::ipc::Channel<crate::events::DesktopEvent>,
) -> Result<(), CommandError> {
    let facade = {
        let guard = state.lock().await;
        guard
            .as_ref()
            .ok_or_else(|| CommandError {
                message: "desktop state is not initialized".into(),
            })?
            .facade()
    };
    let mut rx = facade
        .subscribe_events(&ThreadId::new(thread_id), None)
        .await
        .map_err(CommandError::from)?;
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = on_event.send(crate::events::DesktopEvent::Runtime { event });
                }
                Err(err) => {
                    let _ = on_event.send(crate::events::DesktopEvent::Error {
                        message: err.to_string(),
                    });
                    break;
                }
            }
        }
    });
    Ok(())
}
```

- [ ] **Step 4: Add frontend channel wrapper**

Add to `apps/desktop/src/api/exagentClient.ts`:

```ts
import { Channel } from "@tauri-apps/api/core";
import type { RuntimeEvent } from "../types";

export async function subscribeEvents(
  threadId: string,
  onEvent: (event: RuntimeEvent) => void,
  onError: (message: string) => void
) {
  const channel = new Channel<{ type: string; event?: RuntimeEvent; message?: string }>();
  channel.onmessage = (message) => {
    if (message.type === "runtime" && message.event) {
      onEvent(message.event);
    }
    if (message.type === "error" && message.message) {
      onError(message.message);
    }
  };
  await invoke("events_subscribe", { threadId, onEvent: channel });
}
```

- [ ] **Step 5: Update ChatView to render event stream**

Keep a local `RuntimeEvent[]` for the selected thread. Render:

```tsx
{events.map((event) => (
  <TurnItem key={event.event_id} event={event.kind} />
))}
```

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test --test desktop_facade desktop_facade_subscribes_to_runtime_events
cd apps/desktop && npm test && npm run build
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/app_server/desktop_facade.rs apps/desktop/src-tauri apps/desktop/src tests/desktop_facade.rs
git commit -m "feat: stream runtime events to desktop"
```

**Acceptance Criteria:**

- Runtime events stream into the desktop frontend through a Tauri channel.
- Chat view renders incoming event items.
- Event errors are surfaced as reconnectable UI state.

## Task 15: Inspector And Changed Files

**Files:**
- Modify: `src/state/index_db/schema.rs`
- Modify: `src/state/index_db/store.rs`
- Modify: `src/state/index_db/reindex.rs`
- Modify: `apps/desktop/src/components/Inspector.tsx`
- Modify: `apps/desktop/src/components/ChangedFiles.tsx`
- Test: `tests/index_db.rs`
- Test: `apps/desktop/src/components/ChangedFiles.test.tsx`

- [ ] **Step 1: Add changed files DB test**

Append to `tests/index_db.rs`:

```rust
#[tokio::test]
async fn changed_files_are_upserted_and_listed() {
    let dir = tempdir().unwrap();
    let db = IndexDb::open(dir.path().join("exagent.sqlite")).await.unwrap();
    let thread_id = ThreadId::new("thread_changed_files");

    db.upsert_changed_file(&thread_id, "src/main.rs").await.unwrap();
    db.upsert_changed_file(&thread_id, "src/lib.rs").await.unwrap();
    let files = db.list_changed_files(&thread_id).await.unwrap();

    assert_eq!(files, vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]);
}
```

- [ ] **Step 2: Implement changed file methods**

Add to `src/state/index_db/store.rs`:

```rust
impl IndexDb {
    pub async fn upsert_changed_file(
        &self,
        thread_id: &crate::types::ThreadId,
        path: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
INSERT INTO thread_changed_files (thread_id, path, last_seen_at)
VALUES (?, ?, ?)
ON CONFLICT(thread_id, path) DO UPDATE SET
  last_seen_at = excluded.last_seen_at
            "#,
        )
        .bind(thread_id.as_str())
        .bind(path)
        .bind(time::now_unix_seconds())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn list_changed_files(
        &self,
        thread_id: &crate::types::ThreadId,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT path FROM thread_changed_files WHERE thread_id = ? ORDER BY path ASC",
        )
        .bind(thread_id.as_str())
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| Ok(row.try_get::<String, _>("path")?))
            .collect()
    }
}
```

- [ ] **Step 3: Extract changed files from write events**

In event handling, detect `tool_result` events whose `result.tool_name` is `write_file`. Extract path from metadata if available. If current `write_file` metadata lacks path, add a small follow-up patch to `src/tools/write_file.rs` so tool result metadata includes:

```json
{
  "path": "relative/or/absolute/path"
}
```

Add a test in `tests/file_tools.rs` proving the metadata path exists.

- [ ] **Step 4: Add changed files component test**

Create `apps/desktop/src/components/ChangedFiles.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ChangedFiles } from "./ChangedFiles";

describe("ChangedFiles", () => {
  it("renders changed file paths", () => {
    render(<ChangedFiles files={["src/main.rs"]} />);
    expect(screen.getByText("src/main.rs")).toBeTruthy();
  });
});
```

- [ ] **Step 5: Wire Inspector**

Update `Inspector` to accept:

```tsx
type InspectorProps = {
  status: string;
  workspacePath?: string;
  changedFiles: string[];
};
```

Render `ChangedFiles` in the inspector.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test --test index_db changed_files_are_upserted_and_listed
cd apps/desktop && npm test
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add src/state/index_db src/tools/write_file.rs tests/index_db.rs tests/file_tools.rs apps/desktop/src
git commit -m "feat: show changed files in desktop inspector"
```

**Acceptance Criteria:**

- Changed files are stored per thread in SQLite.
- Inspector shows changed files as a list.
- No diff viewer is included.

## Task 16: Verification, Polish, And Documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/protocol/app-server-boundary-v2.md` if approval decision protocol is added.
- Modify: `docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md` if implementation differs.
- Create: `docs/demo/exagent-desktop-walkthrough.md`

- [ ] **Step 1: Add desktop walkthrough**

Create `docs/demo/exagent-desktop-walkthrough.md`:

````markdown
# ExAgent Desktop Walkthrough

## Start In Development

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

## First Run

1. Add a project folder.
2. Start a new chat.
3. Send a prompt.
4. Watch assistant and tool events stream into the transcript.
5. Rename the session.
6. Archive and unarchive the session.
```
````

- [ ] **Step 2: Update README**

Add a short desktop section to `README.md`:

````markdown
## Desktop Workbench

ExAgent Desktop is a Tauri workbench for local projects. It runs in one process and calls the Rust app-server boundary in-process; it does not require the HTTP API server.

```bash
cd apps/desktop
npm install
npm run tauri:dev
```
````

- [ ] **Step 3: Run full Rust verification**

Run:

```bash
cargo fmt --check
cargo test
git diff --check
```

Expected: all pass.

- [ ] **Step 4: Run full desktop verification**

Run:

```bash
cd apps/desktop
npm test
npm run build
npm run tauri:build
```

Expected: all pass.

- [ ] **Step 5: Manual acceptance smoke**

Run:

```bash
cd apps/desktop
npm run tauri:dev
```

Verify manually:

- Add `/Volumes/EXEXEX/ExAgent` as a project.
- Existing `.exagent/threads` sessions appear after reindex.
- New Chat creates a new thread and session row.
- Sending a prompt starts a turn.
- Runtime events appear in the transcript.
- Tool output appears as expandable summary.
- Inspector shows workspace and changed files.
- Rename changes session title without changing rollout.
- Archive hides the session from default list.
- Unarchive restores it.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/demo/exagent-desktop-walkthrough.md docs/protocol/app-server-boundary-v2.md docs/superpowers/specs/2026-06-01-exagent-desktop-gui-design.md
git commit -m "docs: add exagent desktop usage guide"
```

**Acceptance Criteria:**

- Full Rust and desktop test suites pass.
- Desktop app builds.
- Manual smoke covers the complete first-version workflow.
- Documentation tells a new contributor how to run the app.

## Implementation Order Summary

1. Add root module and dependency baseline.
2. Add SQLite schema and `IndexDb::open`.
3. Add project registry.
4. Add rollout reindex and thread list.
5. Add rename, pin, soft archive, search.
6. Add root-crate desktop facade.
7. Add approval decision boundary.
8. Scaffold Tauri app.
9. Wire Tauri commands and channels.
10. Build frontend client/store.
11. Build layout.
12. Render runtime events.
13. Connect end-to-end desktop flow.
14. Stream runtime events.
15. Add changed files inspector.
16. Verify and document.

## Risk Controls

- Keep root index code independent from Tauri so it can be tested with `cargo test`.
- Keep desktop commands as a facade over `AppServerService`; do not call runtime internals from UI code.
- Treat rollout as source of truth; SQLite is index plus desktop metadata.
- Reindex must tolerate missing `.exagent/threads`.
- Approval decision behavior must be explicit. Do not claim Codex-style paused tool-loop resumption unless the runtime actually supports it.
- Do not add Monaco, diff viewer, remote server mode, plugin UI, or multi-window synchronization in the first version.
