# ExAgent App-Server Boundary V2

This document describes the current client-facing HTTP boundary for ExAgent.
It is a protocol note for CLI, UI, and SDK clients. Historical design docs may
mention older `fork`, `inspect`, `collect`, or `thread/spawn_child` routes; those
routes are not part of the current public boundary.

## Protocol Version

Clients can discover the protocol surface with:

```bash
curl -s http://127.0.0.1:3000/initialize \
  -H 'content-type: application/json' \
  -d '{}'
```

The current response advertises:

```json
{
  "type": "initialized",
  "protocol_version": "appserver-runtime-boundary-v2",
  "supported_ops": [
    "initialize",
    "thread_start",
    "thread_resume",
    "thread_read",
    "turn_start",
    "turn_interrupt",
    "events_replay"
  ],
  "supported_streams": ["events_subscribe"]
}
```

## Routes

| Route | Method | Purpose |
| --- | --- | --- |
| `/health` | `GET` | Process health check. |
| `/initialize` | `POST` | Return protocol version and capability lists. |
| `/run` | `POST` | Compatibility convenience: create/resume a thread, run one turn, wait for completion. |
| `/thread/start` | `POST` | Create durable thread state and load a runtime actor. |
| `/thread/read` | `POST` | Read a renderable thread view. Uses live state when loaded. |
| `/thread/resume` | `POST` | Load an existing thread from persisted state. |
| `/turn/start` | `POST` | Submit one user turn. Returns after the turn is accepted, not after completion. |
| `/turn/interrupt` | `POST` | Interrupt an active turn or a pending approval wait. |
| `/thread/op` | `POST` | Generic tagged `BoundaryOp` dispatch route. |
| `/events/replay` | `POST` | Return persisted runtime events, optionally with a snapshot view. |
| `/events/subscribe` | `POST` | Return an SSE stream of replayed gap events followed by live events. |

Removed legacy routes return `404 Not Found`: `/fork`, `/inspect`, `/collect`,
and `/thread/spawn_child`.

## Thread Lifecycle

`POST /thread/start` creates a new thread:

```json
{
  "workspace_root": ".",
  "cwd": "."
}
```

The response contains a `thread` view:

```json
{
  "thread": {
    "id": "session_...",
    "status": "idle",
    "active_turn": null,
    "turns": [],
    "snapshot_path": ".exagent/sessions/session_.../snapshot.json",
    "events_path": ".exagent/sessions/session_.../events.jsonl"
  }
}
```

`snapshot_path` and `events_path` are compatibility fields in the public
protocol. For rollout-backed new sessions they are not durable state files and
may not exist on disk. The durable thread record is
`.exagent/threads/<thread_id>/rollout.jsonl`.

`workspace_root` and `cwd` are durable context for a newly created thread.
`cwd` must resolve inside `workspace_root`.

`POST /thread/resume` loads existing state:

```json
{
  "thread_id": "session_...",
  "workspace_root": "."
}
```

Persisted thread context wins on resume. Unsupported request overrides are
reported in `ignored_overrides`.

## Turn Lifecycle

`POST /turn/start` submits user input to a loaded runtime:

```json
{
  "thread_id": "session_...",
  "prompt": "Summarize this runtime.",
  "workspace_root": ".",
  "turn_context": {
    "cwd": "src"
  }
}
```

`turn_context.cwd` is optional. When present, it applies only to that turn and
does not rewrite the thread snapshot's durable `cwd`.

The response shape is:

```json
{
  "thread_id": "session_...",
  "turn": {
    "id": "turn_1",
    "status": "in_progress",
    "items": []
  }
}
```

Only one active turn is allowed per loaded thread in one app-server process. A
second `turn/start` while a turn is active returns `409 Conflict`.

Final output is delivered as runtime events. Clients should subscribe before
starting a turn, or call `events/replay` after the turn.

## Event Subscription

`POST /events/subscribe` returns Server-Sent Events:

```bash
curl -N -s http://127.0.0.1:3000/events/subscribe \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "session_...",
    "workspace_root": ".",
    "after_event_id": "evt_3"
  }'
```

The stream behavior is replay-first:

1. The server reads persisted events after `after_event_id`, if provided.
2. It emits those events as SSE `data:` JSON payloads.
3. It switches to the loaded runtime's live broadcast channel.

Each SSE data payload is a serialized `RuntimeEvent`:

```json
{
  "event_id": "evt_4",
  "session_id": "session_...",
  "turn_id": "turn_1",
  "kind": {
    "type": "assistant_turn",
    "turn": {
      "text": "assistant output",
      "tool_calls": []
    }
  }
}
```

Current event kinds are:

- `turn_started`
- `turn_completed`
- `turn_interrupted`
- `assistant_turn`
- `tool_result`
- `exec_output`
- `approval_requested`
- `approval_decision`
- `compaction_written`
- `runtime_error`

The LLM adapter does not currently stream token deltas. A full assistant message
is emitted as an `assistant_turn` event.

## Event Replay

`POST /events/replay` reads persisted `EventMsg` entries from rollout storage
or the loaded runtime's live event buffer:

```json
{
  "thread_id": "session_...",
  "workspace_root": ".",
  "after_event_id": "evt_3",
  "limit": 50,
  "include_snapshot": true,
  "event_kinds": ["assistant_turn", "tool_result"]
}
```

`after_event_id`, `limit`, `include_snapshot`, and `event_kinds` are optional.
When `include_snapshot` is true, the response includes a stable snapshot view:

```json
{
  "thread_id": "session_...",
  "events": [],
  "snapshot": {
    "thread_id": "session_...",
    "cwd": "/absolute/workspace/path",
    "latest_compaction": null,
    "open_exec_session_count": 0,
    "conversation_message_count": 2,
    "pending_approval_count": 0
  }
}
```

`events/replay` is the durable replay source for persisted runtime events. It does not depend
on a runtime still being loaded in memory.

## `thread/read` Versus `events/replay`

Use `thread/read` when a client needs a compact renderable view:

```json
{
  "thread_id": "session_...",
  "workspace_root": "."
}
```

The response groups events into turns and items:

```json
{
  "thread": {
    "id": "session_...",
    "status": "idle",
    "active_turn": null,
    "turns": [
      {
        "id": "turn_1",
        "status": "completed",
        "items": [
          {
            "type": "assistant_message",
            "text": "assistant output"
          }
        ]
      }
    ],
    "snapshot_path": ".exagent/sessions/session_.../snapshot.json",
    "events_path": ".exagent/sessions/session_.../events.jsonl"
  }
}
```

When a runtime is loaded, `thread/read` prefers the live `ThreadSession` view.
That view includes the live snapshot and a bounded recent event window. It is
the right source for current UI state, active turn status, and pending approval
state.

Use `events/replay` when a client needs the complete historical event log,
cursor pagination, event-kind filtering, or reconstruction after process
restart.

## Interrupts And Approvals

`POST /turn/interrupt` accepts:

```json
{
  "thread_id": "session_...",
  "turn_id": "turn_1",
  "workspace_root": "."
}
```

`turn_id` is optional. For an active turn, the runtime sends an interrupt to the
running turn and records `turn_interrupted`. For a waiting approval state, it
clears pending approvals from the snapshot, cancels policy-side waiters, and
records `turn_interrupted`.

Risky command approvals are exposed as events:

- `approval_requested`: command execution is waiting for a decision.
- `approval_decision`: approval or denial has been recorded.

Approval decisions are currently submitted through the `run_command` tool with
an `approval_id` and `decision`.

## Error Statuses

HTTP adapters map typed runtime errors to stable status classes:

- `400 Bad Request`: invalid workspace, cwd, turn context, or request shape.
- `404 Not Found`: missing thread state.
- `409 Conflict`: busy thread, rejected turn, or interrupted turn.
- `500 Internal Server Error`: unexpected runtime or persistence failure.

Error responses use:

```json
{
  "error": "message"
}
```

## Compatibility Notes

`POST /run` remains as a convenience route for simple clients. Internally it
creates or resumes a thread, starts one turn through the runtime boundary, waits
for completion, and returns the final assistant text. New clients should prefer
the explicit thread, turn, and events routes because those match the live
runtime model.

The current boundary intentionally does not include runtime-native child
threads, transcript forks, list/search/archive operations, multi-client approval
routing, or token-delta streaming.
