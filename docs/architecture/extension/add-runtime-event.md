# Add A Runtime Event

## Normal Change Set

1. Add the event variant in `src/state/events.rs`.
2. Record the event from the responsible runtime module.
3. Decide whether it should persist in `src/state/rollout.rs`.
4. Add replay/filter support in `src/app_server/protocol.rs` and `thread_manager.rs` if clients can request it.
5. Add `ThreadItem` mapping if it should appear in `ThreadView`.
6. Update the relevant flow document under `docs/architecture/flows/`.

## Decision Checklist

- Is this an event or just state?
- Does a cold replay need to see it?
- Is it user-visible?
- Does it belong to a turn?
- Should it be filtered through `RuntimeEventKindFilter`?
