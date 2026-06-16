#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/export-public-runtime.sh [options]

Exports the private ExAgent monorepo runtime snapshot into the public runtime
repository working tree. The private monorepo is the source of truth; the
public repository is a generated release surface.

Options:
  --public-dir PATH   Public runtime repo path. Defaults to ../ExAgent-public-runtime.
  --source-ref REF    Source git ref to export. Defaults to HEAD.
  --allow-dirty      Allow a dirty private source checkout. Export still uses source-ref.
  --skip-verify      Skip cargo fmt/clippy/test/deny verification in the generated snapshot.
  --commit           Commit generated public changes in the public runtime repo.
  --push             Push the current public runtime branch after committing.
  -h, --help         Show this help.

Recommended protected-branch flow:
  cd ../ExAgent-public-runtime
  git switch main
  git pull --ff-only
  git switch -c runtime-sync/<short-sha>
  ../ExAgent/scripts/export-public-runtime.sh --commit
  git push -u origin HEAD
  gh pr create --base main --fill
USAGE
}

log() {
  printf '==> %s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
SOURCE_REF="HEAD"
PUBLIC_DIR="$(cd "$SOURCE_ROOT/.." && pwd -P)/ExAgent-public-runtime"
ALLOW_DIRTY=0
RUN_VERIFY=1
DO_COMMIT=0
DO_PUSH=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --public-dir)
      [[ $# -ge 2 ]] || die "--public-dir requires a path"
      PUBLIC_DIR="$2"
      shift 2
      ;;
    --source-ref)
      [[ $# -ge 2 ]] || die "--source-ref requires a git ref"
      SOURCE_REF="$2"
      shift 2
      ;;
    --allow-dirty)
      ALLOW_DIRTY=1
      shift
      ;;
    --skip-verify)
      RUN_VERIFY=0
      shift
      ;;
    --commit)
      DO_COMMIT=1
      shift
      ;;
    --push)
      DO_PUSH=1
      DO_COMMIT=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

require_cmd cargo
require_cmd find
require_cmd git
require_cmd perl
require_cmd rg
require_cmd rsync
require_cmd tar

[[ -d "$PUBLIC_DIR/.git" ]] || die "public runtime repo is not a git checkout: $PUBLIC_DIR"

SOURCE_ORIGIN="$(git -C "$SOURCE_ROOT" remote get-url origin 2>/dev/null || true)"
case "$SOURCE_ORIGIN" in
  *ExAgent-Desktop.git*) ;;
  *) die "source origin must point at the private ExAgent-Desktop repo, got: ${SOURCE_ORIGIN:-<none>}" ;;
esac

PUBLIC_ORIGIN="$(git -C "$PUBLIC_DIR" remote get-url origin 2>/dev/null || true)"
case "$PUBLIC_ORIGIN" in
  *github.com/exqqstar/ExAgent.git*) ;;
  *) die "public repo origin must point at exqqstar/ExAgent, got: ${PUBLIC_ORIGIN:-<none>}" ;;
esac

if [[ "$ALLOW_DIRTY" -ne 1 && -n "$(git -C "$SOURCE_ROOT" status --porcelain)" ]]; then
  die "private source checkout is dirty; commit/stash first or pass --allow-dirty"
fi

if [[ -n "$(git -C "$PUBLIC_DIR" status --porcelain)" ]]; then
  die "public runtime checkout is dirty; commit/stash/reset before exporting"
fi

git -C "$SOURCE_ROOT" rev-parse --verify "$SOURCE_REF^{commit}" >/dev/null
SOURCE_SHA="$(git -C "$SOURCE_ROOT" rev-parse "$SOURCE_REF^{commit}")"
SOURCE_SHORT="$(git -C "$SOURCE_ROOT" rev-parse --short "$SOURCE_REF^{commit}")"

STAGING="$(mktemp -d)"
cleanup() {
  rm -rf "$STAGING"
}
trap cleanup EXIT

log "Exporting private $SOURCE_SHORT into staging"
git -C "$SOURCE_ROOT" archive "$SOURCE_REF" -- \
  src \
  tests \
  Cargo.toml \
  .env.example \
  deny.toml \
  LICENSE-APACHE \
  LICENSE-MIT \
  NOTICE \
  THIRD_PARTY_NOTICES.md \
  AUTHORS.md \
  | tar -x -C "$STAGING"

log "Applying public runtime sanitization"
perl -0pi -e 's/description = ".*?"/description = "A Rust agent runtime for durable local coding sessions, replayable events, and approval-gated tools."/s' "$STAGING/Cargo.toml"
perl -0pi -e 's/members = \["\.", "apps\/desktop\/src-tauri"\]/members = ["."]/g' "$STAGING/Cargo.toml"

cat > "$STAGING/.env.example" <<'EOF_ENV'
# Optional local CLI/API configuration.
#
# Use these variables for local runtime experiments such as:
# - cargo run -- "Summarize this workspace"
# - cargo run -- api
# - integration tests and protocol experiments.

OPENAI_BASE_URL=https://api.openai.com/v1
OPENAI_API_KEY=your-api-key
OPENAI_MODEL=gpt-5.2

# Optional provider-specific fallback used by resolver tests and CLI flows.
DEEPSEEK_BASE_URL=https://api.deepseek.com
DEEPSEEK_API_KEY=your-deepseek-api-key
DEEPSEEK_MODEL=deepseek-v4-flash

# Optional. Supported values: off, advisory, enforced.
EXAGENT_POLICY_MODE=off

# Optional. Only full_access is supported today.
EXAGENT_PERMISSION_PROFILE=full_access
EOF_ENV

cat > "$STAGING/THIRD_PARTY_NOTICES.md" <<'EOF_NOTICES'
# Third-Party Notices

ExAgent is licensed under `MIT OR Apache-2.0`. Some documentation in this
repository summarizes or references third-party open-source projects.

The root [NOTICE](NOTICE) file contains attribution notices for ExAgent itself.

## External Reference Material

Private design notes and local-only reference checkouts are not distributed in
the public repository. Do not add vendored third-party source, reference packs,
or copied documentation unless the upstream license allows it and the required
license and attribution notices are included.

## Dependency Licenses

Rust dependencies keep their own licenses. Use `cargo deny check licenses` for
the dependency policy configured in `deny.toml`.
EOF_NOTICES

if [[ -f "$STAGING/src/app_server/desktop_facade.rs" ]]; then
  mv "$STAGING/src/app_server/desktop_facade.rs" "$STAGING/src/app_server/client_facade.rs"
fi
if [[ -f "$STAGING/tests/desktop_facade.rs" ]]; then
  mv "$STAGING/tests/desktop_facade.rs" "$STAGING/tests/client_facade.rs"
fi

mapfile -t RUST_FILES < <(find "$STAGING/src" "$STAGING/tests" -type f -name '*.rs' | sort)
if [[ "${#RUST_FILES[@]}" -gt 0 ]]; then
  perl -pi -e 's/desktop_facade/client_facade/g; s/DesktopFacade/ClientFacade/g; s/DesktopInspect/ClientInspect/g; s/desktop_inspect/client_inspect/g; s/DESKTOP/CLIENT/g; s/Desktop/Client/g; s/desktop/client/g; s/\bGUI\b/client interface/g' "${RUST_FILES[@]}"
  perl -pi -e 's/ExAgent Client/ExAgent Runtime/g; s/exagent-client/exagent-runtime/g; s/apps\/client\/src\/components\/TranscriptList\.tsx/src\/app_server\/thread_projection.rs/g; s/runtime and client updates/runtime and projection updates/g; s/Design the client client interface/Design the runtime API/g' "${RUST_FILES[@]}"
fi

mkdir -p "$STAGING/.github/workflows"

cat > "$STAGING/.gitignore" <<'EOF_GITIGNORE'
/.DS_Store
**/.DS_Store

# Rust build output.
/target

# Local runtime state and generated benchmark artifacts.
/.exagent
/.exagent-bench
/.superpowers
/.worktrees

# Local-only reference checkouts and private notes.
/external-references
/AGENTS.md
/AGENT.md
/Agent.md
/docs/private/
/docs/.private-archive/
/.claude

# Local environment files. Keep examples and templates commit-able.
/.env
/.env.*
!/.env.example
!/.env.*.template
EOF_GITIGNORE

cat > "$STAGING/README.md" <<'EOF_README'
# ExAgent

ExAgent is an open-source Rust runtime for local coding agents. It provides durable thread execution, replayable runtime events, approval-gated tools, model-provider adapters, MCP support, subagents, goals, and project memory primitives.

The client application is distributed separately while the product interface is still evolving. This repository contains the runtime and integration surface intended for public development.

## Highlights

- Durable agent sessions with event replay
- Approval-gated local tools for command execution and file edits
- Model adapters for OpenAI-compatible, Anthropic, Gemini, GitHub Copilot, and related providers
- MCP client/runtime support
- Subagents, goals, project memory, and checkpoint primitives
- HTTP/API and CLI entrypoints for local experiments

## Quickstart

Install a current Rust toolchain, then run:

```bash
cargo test --package exagent --locked
```

For a local CLI experiment:

```bash
cp .env.example .env
cargo run -- "Summarize this workspace"
```

For the local API server:

```bash
cargo run -- api
```

## Repository Layout

- `src/runtime`: execution kernel, thread/session turn loop, tool runtime, policy, goals, memory, and subagents
- `src/tools`: built-in tool trait, registry, command/file tools, memory tools, and web tools
- `src/model`: model provider adapters and conversation types
- `src/mcp`: MCP client and tool integration
- `src/app_server`: local app-server boundary and request processors
- `src/state`: durable event, rollout, index, memory, and transcript state
- `tests`: integration coverage for runtime behavior, policy, tools, persistence, and protocol boundaries

## Development

Useful checks:

```bash
cargo fmt --all -- --check
cargo clippy --package exagent --all-targets
cargo test --package exagent --locked
cargo deny check licenses sources bans
```

## Status

ExAgent is pre-1.0. Runtime APIs and storage formats may change while the project is being stabilized.

## License

Licensed under either of:

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT License (`LICENSE-MIT`)

at your option.
EOF_README

cat > "$STAGING/CONTRIBUTING.md" <<'EOF_CONTRIBUTING'
# Contributing

Thanks for taking the time to improve ExAgent. This project is still early, so small, focused changes are easier to review than broad rewrites.

## Development Setup

Install the Rust toolchain, then run:

```bash
cargo test --package exagent --locked
```

Use `.env.example` only for local CLI/API runtime experiments. Do not commit real API keys, OAuth tokens, rollout files, benchmark outputs, or local workspace state.

## Common Commands

```bash
cargo fmt --all -- --check
cargo clippy --package exagent --all-targets
cargo test --package exagent --locked
cargo deny check licenses sources bans
```

## Pull Requests

Before opening a pull request:

- Keep the scope narrow and describe the behavior change.
- Add or update tests when runtime behavior changes.
- Update README or protocol documentation when commands, API contracts, or setup steps change.
- Include known limitations when a change intentionally does not cover a case.
- Confirm the commands above were run, or explain why a command could not run.

## Architecture Changes

For changes to runtime persistence, thread lifecycle, app-server protocol, tool execution, provider adapters, or memory storage, describe the rationale and trade-offs in the pull request.

## Security

Please do not report vulnerabilities in public issues. Follow `SECURITY.md`.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project is licensed as `MIT OR Apache-2.0`, without any additional terms or conditions.
EOF_CONTRIBUTING

cat > "$STAGING/SECURITY.md" <<'EOF_SECURITY'
# Security Policy

## Supported Versions

ExAgent is pre-1.0. Security fixes target the current `main` branch unless a release branch is explicitly announced.

## Reporting a Vulnerability

Do not open a public issue with exploit details, credentials, OAuth tokens, private rollout data, or reproduction steps that expose another system.

Preferred reporting path:

1. Use GitHub private vulnerability reporting for this repository if it is enabled.
2. If private reporting is not available, contact the maintainer out of band and share only a minimal summary until a private channel is established.

Please include:

- Affected commit, release, or branch.
- Impacted component, such as runtime tools, app-server API, provider adapters, MCP configuration, or persistence.
- Reproduction steps with test credentials or redacted data only.
- Any known mitigations.

## Scope

Security-sensitive areas include:

- command execution and approval policy
- workspace path validation
- rollout persistence and transcript replay
- provider credentials and OAuth tokens
- MCP and dynamic tool configuration

The current `full_access` permission profile is not an OS sandbox. Treat it as a local development mode with tool-level checks only.
EOF_SECURITY

cat > "$STAGING/.github/workflows/ci.yml" <<'EOF_CI'
name: CI

on:
  push:
    branches: ["main"]
  pull_request:

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  rust-core:
    name: Rust core
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Check formatting
        run: cargo fmt --all -- --check
      - name: Check core lint
        run: cargo clippy --package exagent --all-targets
      - name: Test core
        run: cargo test --package exagent --locked

  dependency-policy:
    name: Dependency policy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install cargo-deny
        run: cargo install cargo-deny --version 0.19.7 --locked
      - name: Check Rust dependency policy
        run: cargo deny check licenses sources bans
EOF_CI

cat > "$STAGING/.github/dependabot.yml" <<'EOF_DEPENDABOT'
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: weekly
    open-pull-requests-limit: 5
EOF_DEPENDABOT

log "Generating runtime-only Cargo.lock"
(cd "$STAGING" && cargo generate-lockfile)

log "Auditing generated public snapshot"
if rg -n 'apps/desktop|apps/client|src-tauri|Tauri|React|GUI|DESIGN\.md|PRODUCT\.md|desktop-chat|exagent-desktop|node_modules|npm|tauri|desktop_facade|DesktopFacade|DesktopInspect|desktop_inspect|desktop' \
  --glob '!target/**' --glob '!LICENSE-APACHE' "$STAGING"; then
  die "public snapshot audit found private desktop/product references"
fi

if rg -n 'gtk|objc2-app-kit|objc2-cloud-kit|webkit2gtk|tauri' "$STAGING/Cargo.lock"; then
  die "public Cargo.lock still contains desktop dependency fingerprints"
fi

if [[ "$RUN_VERIFY" -eq 1 ]]; then
  log "Running public runtime verification"
  (cd "$STAGING" && cargo fmt --all -- --check)
  (cd "$STAGING" && cargo clippy --package exagent --all-targets)
  (cd "$STAGING" && cargo test --package exagent --locked)
  (cd "$STAGING" && cargo deny check licenses sources bans)
fi

log "Syncing generated snapshot to $PUBLIC_DIR"
rsync -a --delete --exclude '.git' "$STAGING"/ "$PUBLIC_DIR"/

log "Public runtime working tree status"
git -C "$PUBLIC_DIR" status --short

if [[ "$DO_COMMIT" -eq 1 ]]; then
  log "Committing public runtime snapshot"
  git -C "$PUBLIC_DIR" add .
  if git -C "$PUBLIC_DIR" diff --cached --quiet; then
    log "No public runtime changes to commit"
  else
    git -C "$PUBLIC_DIR" commit -m "Sync public runtime from private $SOURCE_SHORT"
  fi
fi

if [[ "$DO_PUSH" -eq 1 ]]; then
  log "Pushing public runtime branch"
  git -C "$PUBLIC_DIR" push
fi

log "Export complete from private source $SOURCE_SHORT"
