# ADR-0007: Allow Workspace-Scoped Absolute Paths

## Status

Accepted

## Context

ExAgent currently rejects all absolute paths in workspace file tools. This is a
simple guardrail, but it creates poor ergonomics for benchmark and container
tasks where prompts naturally reference files as `/app/main.cpp` or
`/workspace/service.proto`.

The runtime already has a `workspace_root`. Rejecting every absolute path makes
file tools stricter than shell commands, which can use absolute paths inside the
same workspace. The model often recovers by retrying with a relative path, but
that wastes tool calls and creates avoidable context noise.

Future sandboxing and approval policy should be built on normalized paths and
permission decisions, not on a blanket absolute-path ban.

## Decision

File tools should accept absolute paths only when they resolve inside
`workspace_root`.

The shared path resolver should:

- Accept relative paths that stay inside `workspace_root`.
- Accept absolute paths that canonicalize under `workspace_root`.
- Reject absolute paths outside `workspace_root`.
- Reject parent traversal that resolves outside `workspace_root` and reject
  symlink escapes.
- Return a normalized path plus metadata for audit: requested path, normalized
  path, and whether the request was absolute.

For the early implementation, the resolver should return a structured result
rather than a bare `PathBuf`, for example:

- `requested_path`: the exact model/user input.
- `normalized_path`: the lexical absolute candidate path.
- `canonical_path`: the path used for workspace containment checks.
- `was_absolute`: whether the request was absolute.

For existing files, containment should be checked against the canonical file
path. For writes to missing files, containment should be checked by
canonicalizing the nearest existing parent and appending the missing tail.
Symlink escapes are rejected in this early implementation.

This is not sandboxing. It is path normalization and workspace containment.
Sandbox policy and approval policy can later consume the same normalized path
metadata to decide read/write permissions.

Once sandbox and permission profiles are mature, this resolver can become closer
to Codex's split: path normalization produces typed absolute paths and audit
metadata, while permission profile and sandbox enforcement decide whether the
operation is allowed. At that point, symlink handling can move from a resolver
ban to an explicit policy decision.

## Consequences

- Benchmark prompts using `/app/...` or `/workspace/...` become first-class
  inputs instead of errors.
- File tools and shell command behavior become more consistent.
- Path resolution becomes slightly more expensive because absolute paths need
  canonicalization.
- Tests must cover existing and missing paths, workspace-internal paths,
  out-of-workspace paths, symlink escapes, and parent traversal.

## Affected Modules

- `src/workspace.rs`
- `src/tools/read_file.rs`
- `src/tools/write_file.rs`
- `tests/file_tools.rs`
- `docs/architecture/modules/project-shell.md`
- `docs/architecture/modules/tools/read-write-file.md`

## Related Docs

- `docs/architecture/benchmarks/terminal-bench-2-1-followups.md`
- `docs/plans/2026-06-01-exagent-runtime-hardening-followups.md`
