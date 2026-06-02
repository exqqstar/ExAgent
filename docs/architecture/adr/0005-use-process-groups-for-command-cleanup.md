# ADR-0005: Use Process Groups For Command Cleanup

## Status

Accepted

## Context

`run_command` starts commands through a shell. A command can then spawn children
or grandchildren. Killing or dropping only the direct shell process is not
enough to guarantee cleanup on timeout, interrupt, explicit termination, or
parent shutdown.

This matters for benchmark and runtime reliability. Password crackers, test
runners, servers, package managers, and backgrounded shell jobs can keep running
after the model-visible tool result says the command timed out. Descendants can
also keep stdout or stderr pipes open, causing output collection to hang after
the direct child exits.

Codex handles this class of problem by treating shell execution as a process
lifecycle, not just a `wait_with_output` call: spawned shell commands run in a
separate process group/session where possible, timeout and cancellation target
the group, and output drain is bounded.

## Decision

Command execution must use a shared process-tree cleanup primitive:

- Add a small runtime helper module that can configure spawned commands into an
  isolated process group or session and clean up the process tree for a child.
- Start each ordinary shell command and each persistent exec command in an
  isolated process group or session on Unix platforms.
- On one-shot timeout, turn interrupt, persistent exec termination, or runtime
  shutdown, terminate the whole process group rather than only the direct child.
- Prefer graceful termination first when practical, then force-kill after a
  short bounded grace period.
- Bound stdout/stderr drain after cancellation so inherited pipes cannot hang
  the runtime.
- Report cleanup metadata in tool output metadata: timed out, root pid, process
  group id when available, cleanup action, cleanup success, and drain timeout.

Persistent exec sessions remain a separate lifecycle. Their normal behavior is
unchanged: they can stay alive across turns, accept stdin, be polled, and remain
live-only across cold replay. The cleanup primitive only changes how they are
stopped when explicitly terminated or when the runtime is shutting them down.

## Consequences

- Timeouts and interrupts become closer to what users expect: the work stops,
  not just the shell wrapper.
- Benchmark trials become less likely to leak CPU-heavy descendants across
  later commands.
- Command execution needs platform-specific helpers and tests. Unix can use
  process groups. Windows is explicitly best-effort for now and may need a
  later job-object/process-tree strategy.
- `run_command` metadata becomes more useful for debugging long-running tasks
  and timeout failures.

## Affected Modules

- `src/tools/run_command.rs`
- `src/runtime/process_cleanup.rs`
- `src/runtime/exec_session.rs`
- `src/runtime/thread_session/turn.rs`
- `tests/run_command.rs`
- `tests/exec_session.rs`

## Related Docs

- `docs/architecture/benchmarks/terminal-bench-2-1-followups.md`
- `docs/plans/2026-06-01-exagent-runtime-hardening-followups.md`
