# ADR-0003: Use Live Overlay For Non-Durable Runtime State

## Status

Accepted

## Context

Some runtime facts are useful to clients while a thread is loaded but cannot be safely restored from disk. Examples include active subprocess handles and pending approval waiters.

## Decision

Store these facts in `RuntimeOverlay`, attached to `ThreadSession.live_state`, instead of encoding them as durable snapshot state.

## Consequences

- Cold rollout replay does not pretend to resurrect live subprocesses or waiters.
- Clients can still see pending approvals and open exec session refs for loaded threads.
- Runtime code must explicitly update overlay state when tool results imply live state changes.

## Affected Modules

- `src/runtime/thread_session/overlay.rs`
- `src/runtime/tool_call_runtime.rs`
- `src/runtime/exec_session.rs`
- `src/runtime/policy.rs`

## Related Docs

- `docs/architecture/modules/runtime/thread-session/overlay.md`
- `docs/architecture/maps/state-map.md`
