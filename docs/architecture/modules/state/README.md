# State Module

## Responsibility

`state` defines durable runtime data and helpers for reading/writing it.

## Durable Source Of Truth

The current durable source of truth is:

```text
.exagent/threads/<thread_id>/rollout.jsonl
```

Legacy session paths are still returned for compatibility, but runtime restore is rollout-backed.

## File Map

- [session.md](session.md)
- [events.md](events.md)
- [rollout.md](rollout.md)
- [transcript.md](transcript.md)
