# ExAgent Walkthrough

This walkthrough shows the smallest realistic operator flow exposed by the
current runtime boundary:

1. start the API server
2. check protocol capabilities
3. create a thread
4. subscribe to runtime events
5. start a turn
6. read the thread view
7. replay the durable event log

The current public boundary does not expose `fork`, `inspect`, `collect`, or
`thread/spawn_child`. Child thread orchestration will return only after it is
defined as a runtime-native operation.

## Prerequisites

Export the environment variables required by the OpenAI-compatible adapter:

```bash
export OPENAI_BASE_URL="https://api.openai.com/v1"
export OPENAI_API_KEY="your-api-key"
export OPENAI_MODEL="gpt-4.1"
export EXAGENT_POLICY_MODE="off"
```

## 1. Start The Server

```bash
cargo run -- api
```

Expected behavior:

- the process binds to `127.0.0.1:3000` unless `EXAGENT_API_ADDR` is set
- `GET /health` returns `{"status":"ok"}`

## 2. Check Protocol Capabilities

```bash
curl -s http://127.0.0.1:3000/initialize \
  -H 'content-type: application/json' \
  -d '{}'
```

Expected response shape:

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

## 3. Create A Thread

```bash
curl -s http://127.0.0.1:3000/thread/start \
  -H 'content-type: application/json' \
  -d '{
    "workspace_root": ".",
    "cwd": "."
  }'
```

Expected response shape:

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

Save `thread.id` as `THREAD_ID`.

## 4. Subscribe To Events

Open a second terminal and subscribe before starting a turn:

```bash
curl -N -s http://127.0.0.1:3000/events/subscribe \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "<THREAD_ID>",
    "workspace_root": "."
  }'
```

The endpoint returns Server-Sent Events. Each `data:` payload is a serialized
`RuntimeEvent`. The stream first replays any persisted events after
`after_event_id`, if provided, then switches to live events from the loaded
runtime.

## 5. Start A Turn

In the first terminal, submit work to the thread:

```bash
curl -s http://127.0.0.1:3000/turn/start \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "<THREAD_ID>",
    "prompt": "Read this runtime and summarize the persistence model.",
    "workspace_root": "."
  }'
```

Expected response shape:

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

`turn/start` returns after the turn is accepted. It does not wait for final
assistant output. Watch the `events/subscribe` terminal for:

- `turn_started`
- one or more `assistant_turn` or `tool_result` events
- `turn_completed`, `runtime_error`, or `turn_interrupted`

## 6. Read The Thread View

After the turn completes, ask the boundary for a renderable thread view:

```bash
curl -s http://127.0.0.1:3000/thread/read \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "<THREAD_ID>",
    "workspace_root": "."
  }'
```

Expected response shape:

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
That live view contains the live snapshot and a bounded recent event window, so
it is suitable for UI state.

## 7. Replay Durable Events

Use `events/replay` when you need the complete persisted timeline:

```bash
curl -s http://127.0.0.1:3000/events/replay \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "<THREAD_ID>",
    "workspace_root": ".",
    "include_snapshot": true
  }'
```

Expected response shape:

```json
{
  "thread_id": "session_...",
  "events": [
    {
      "event_id": "evt_1",
      "session_id": "session_...",
      "turn_id": "turn_1",
      "kind": {
        "type": "turn_started"
      }
    }
  ],
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

`events/replay` supports `after_event_id`, `limit`, and `event_kinds` filters.
It reads disk and remains available after process restart.

## Optional: Interrupt A Turn

If a turn is running or waiting on approval, interrupt it with:

```bash
curl -s http://127.0.0.1:3000/turn/interrupt \
  -H 'content-type: application/json' \
  -d '{
    "thread_id": "<THREAD_ID>",
    "workspace_root": "."
  }'
```

The runtime records `turn_interrupted`. If the thread was waiting for command
approval, pending approvals are removed from the snapshot and policy-side
waiters are cancelled.

## On-Disk Artifacts

Each thread persists under:

```text
.exagent/sessions/<thread_id>/
```

The two important files are:

- `snapshot.json`: current recoverable state
- `events.jsonl`: complete replayable runtime history

For the full client-facing protocol contract, see
[docs/protocol/app-server-boundary-v2.md](../protocol/app-server-boundary-v2.md).
