# ADR-0002: Use A Thread Runtime Actor For Serialized Thread Operations

## Status

Accepted

## Context

Each thread can receive user input, interruption, event subscriptions, and reads. Running two turns against the same thread concurrently would corrupt conversation order and live state.

## Decision

Represent each loaded thread with a `ThreadRuntime` actor facade. Operations are sent through an internal queue and handled by a single `ThreadSession`.

## Consequences

- Per-thread operations are serialized.
- `active_turn` can reject overlapping turns before they enter the runtime loop.
- Event subscription and live reads can observe shared state without owning execution.
- Cross-process active-turn locking is still out of scope.

## Affected Modules

- `src/runtime/thread_runtime.rs`
- `src/runtime/thread_session/mod.rs`
- `src/app_server/thread_manager.rs`

## Related Docs

- `docs/architecture/modules/runtime/thread-runtime.md`
- `docs/architecture/flows/turn-lifecycle.md`
