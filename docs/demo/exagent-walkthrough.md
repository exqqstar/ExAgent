# ExAgent Walkthrough

This walkthrough shows the smallest realistic operator flow exposed by the current runtime:

1. start the API server
2. create a root session
3. fork a child session
4. inspect the child topology
5. collect the child output

## Prerequisites

Export the environment variables required by the OpenAI-compatible adapter:

```bash
export OPENAI_BASE_URL="https://api.openai.com/v1"
export OPENAI_API_KEY="your-api-key"
export OPENAI_MODEL="gpt-4.1"
```

## 1. Start the server

```bash
cargo run -- api
```

Expected behavior:

- the process binds to `127.0.0.1:3000` unless `EXAGENT_API_ADDR` is set
- `GET /health` returns `{"status":"ok"}`

## 2. Run a root session

```bash
curl -s http://127.0.0.1:3000/run \
  -H 'content-type: application/json' \
  -d '{
    "prompt": "Read this runtime and summarize the persistence model.",
    "workspace_root": ".",
    "cwd": "."
  }'
```

Expected response shape:

```json
{
  "text": "assistant output",
  "tool_calls": [],
  "session_id": "session-...",
  "snapshot_path": ".exagent/sessions/session-.../snapshot.json",
  "events_path": ".exagent/sessions/session-.../events.jsonl"
}
```

Save the returned `session_id` as the parent session id.

## 3. Fork a child session

```bash
curl -s http://127.0.0.1:3000/fork \
  -H 'content-type: application/json' \
  -d '{
    "parent_session_id": "<root-session-id>",
    "agent_role": "spec",
    "prompt": "Draft goals and acceptance criteria for the next orchestration milestone.",
    "workspace_root": ".",
    "spawned_by_turn_id": "turn_1"
  }'
```

The response returns a new child `session_id` plus the child snapshot and event-log paths.

## 4. Inspect child sessions

```bash
curl -s http://127.0.0.1:3000/inspect \
  -H 'content-type: application/json' \
  -d '{
    "parent_session_id": "<root-session-id>",
    "workspace_root": "."
  }'
```

Expected response shape:

```json
{
  "children": [
    {
      "session_id": "session-...",
      "parent_session_id": "session-...",
      "root_session_id": "session-...",
      "agent_role": "spec",
      "status": "completed",
      "snapshot_path": ".exagent/sessions/session-.../snapshot.json",
      "events_path": ".exagent/sessions/session-.../events.jsonl"
    }
  ]
}
```

## 5. Collect child output

```bash
curl -s http://127.0.0.1:3000/collect \
  -H 'content-type: application/json' \
  -d '{
    "session_id": "<child-session-id>",
    "workspace_root": "."
  }'
```

Expected response shape:

```json
{
  "session": {
    "child": {
      "session_id": "session-...",
      "agent_role": "spec",
      "status": "completed"
    },
    "latest_useful_output": {
      "kind": "assistant_text",
      "content": "..."
    },
    "structured_result": null
  }
}
```

If the child used `record_structured_result`, `structured_result` will contain the typed payload for `spec`, `test`, or `judge`.

## On-Disk Artifacts

Each session persists under:

```text
.exagent/sessions/<session_id>/
```

The two important files are:

- `snapshot.json`: current recoverable state
- `events.jsonl`: replayable runtime history
