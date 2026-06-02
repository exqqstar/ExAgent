# ADR-0001: Use Rollout JSONL As Thread Source Of Truth

## Status

Accepted

## Context

ExAgent needs durable thread records for recovery, replay, and auditability. Earlier compatibility paths include snapshot and event files, but runtime behavior needs one append-oriented record that captures thread metadata, conversation items, context, compaction checkpoints, and important events.

## Decision

Use `.exagent/threads/<thread_id>/rollout.jsonl` as the durable source of truth for thread recovery and event replay.

## Consequences

- Cold recovery can rebuild snapshots and events from one ordered log.
- Append-only records make audit and replay easier.
- Live-only state, such as subprocess handles and pending approval waiters, must not be recreated from rollout.
- Compatibility paths can remain in protocol responses without being runtime inputs.

## Affected Modules

- `src/state/rollout.rs`
- `src/runtime/thread_session/mod.rs`
- `src/runtime/thread_session/events.rs`
- `src/app_server/thread_manager.rs`

## Related Docs

- `docs/architecture/modules/state/rollout.md`
- `docs/architecture/flows/event-replay.md`
