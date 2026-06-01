# Read And Write File Tools

## Responsibility

`read_file` and `write_file` provide workspace-bounded text file access.

## Files

- `src/tools/read_file.rs`
- `src/tools/write_file.rs`

## Key Rules

- Absolute paths are rejected.
- Parent traversal that escapes workspace is rejected.
- Read supports optional line ranges.
- Write creates parent directories when needed.

## Connections

Both tools rely on `src/workspace.rs::resolve_workspace_path`.
