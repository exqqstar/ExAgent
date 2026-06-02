# ADR-0008: Project Tool Output Before Model Context

## Status

Accepted

## Context

`run_command` currently returns model-visible content shaped as raw stdout and
stderr text, truncated by a byte cap. This is transparent, but it is expensive
and noisy for long debugging and data-analysis tasks. Repeated command output
enters conversation history immediately and can dominate the prompt inside a
single user turn.

Codex separates captured exec output from model-visible output. It tracks
stdout, stderr, aggregated output, duration, timeout state, and exit code, then
formats a bounded representation for the model using a truncation policy.
Output capture remains useful for events and UI, while the model receives a
projection sized for reasoning.

## Decision

ExAgent should introduce an explicit tool-output projection boundary before
tool results enter model-visible context.

For shell commands, the first implementation should:

- Capture stdout and stderr with byte counts and truncation flags.
- Preserve exit code, timeout state, cwd, duration, and command metadata.
- Return a compact model-visible projection that includes metadata plus bounded
  output, preferably head/tail rather than only prefix truncation.
- Keep raw or fuller output out of `ToolResult.content` and `ToolResult.meta`
  until there is a separate artifact/event channel for data that should not be
  serialized into model-visible tool messages.
- Make truncation policy explicit so future models and modes can choose bytes or
  approximate token budgets.

The early implementation scope is intentionally narrow:

- Apply projection to one-shot `run_command` output first.
- Leave persistent exec session poll output unchanged until a separate delta or
  live-buffer projection design exists.
- Use byte-based head/tail projection and preserve the failure tail.
- Store projected stdout and stderr in both `content` and `meta.stdout` /
  `meta.stderr`, because the current runtime serializes the whole `ToolResult`
  JSON into model context.
- Add structured metadata such as `stdout_bytes`, `stderr_bytes`,
  `stdout_truncated`, `stderr_truncated`, and `output_projection`.

Repeated command detection, semantic log summarization, data profiling, and
in-turn compaction are follow-up layers on the same projection boundary.

The longer-term direction should move closer to Codex:

- The context manager, not only individual tools, should own model-visible
  tool/function output truncation.
- Projection policy should support token budgets as well as byte budgets.
- Exec capture should distinguish raw/full output artifacts or event-store data
  from model-visible projections.
- Long-running and persistent exec should use a head/tail buffer or delta
  projection so repeated polls do not reinsert the full accumulated transcript.

## Consequences

- Long command output consumes less prompt budget.
- Runtime and UI can still inspect richer output through metadata/events.
- Tool result shape becomes more structured and needs migration-safe tests.
- Some debugging detail may be hidden from the model by default, so projections
  must preserve the most useful failure information: exit code, final error
  lines, traceback tail, and truncation metadata.

## Affected Modules

First implementation:

- `src/tools/output_projection.rs`
- `src/tools/run_command.rs`
- `tests/run_command.rs`

Future Codex-like output pipeline:

- `src/runtime/context.rs`
- `src/runtime/thread_session/turn.rs`
- `src/state/events.rs`
- `src/model/types.rs`
- `tests/thread_runtime.rs`

## Related Docs

- `docs/architecture/benchmarks/terminal-bench-2-1-followups.md`
- `docs/plans/2026-06-01-exagent-runtime-hardening-followups.md`
