# Thread Session

## Responsibility

`runtime/thread_session` is the loaded thread state machine.

It restores from rollout, owns live session state, executes user turns, records runtime events, and handles approval interruption.

## State

- `ThreadSession.live_state`: snapshot, overlay, live event buffer, status.
- `ContextManager`: model-visible conversation history.
- `RolloutStore`: durable append target.
- `ThreadEventRecorder`: event id allocation, persistence, broadcast.

## File Map

- `mod.rs`: session construction, rollout restore, live view, status publication, approval interrupt handling.
- `turn.rs`: user input handling, LLM/tool loop, compaction, token count events.
- `events.rs`: event recording pipeline.
- `overlay.rs`: live-only approval and exec session refs.

## Main Flows

- [turn-lifecycle.md](turn-lifecycle.md)
- [events.md](events.md)
- [overlay.md](overlay.md)
