# ExAgent App-Server Boundary V2

This document describes the advanced HTTP boundary for ExAgent. The primary
user entry point is the Tauri desktop app, which calls the Rust runtime through
in-process Tauri commands and does not require starting this HTTP server.

Use this protocol when building tests, SDK experiments, or external clients that
need machine-readable thread state and event streams. Historical design docs may
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
    "thread_fork",
    "thread_compact",
    "thread_goal",
    "agent_tree",
    "approvals_list",
    "checkpoint_restore",
    "open_question_resolve",
    "turn_start",
    "turn_interrupt",
    "approval_decision",
    "submit_user_input",
    "events_replay"
  ],
  "supported_streams": ["events_subscribe"],
  "supported_permission_profiles": ["full_access"]
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
| `/agent/tree` | `POST` | Read the root-thread agent roster and nested subagent activity. |
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

`ThreadView` is a projection, not the source of truth. For a loaded runtime it
is derived from the `ThreadSession` durable materialized state plus its
in-memory `RuntimeOverlay` and live event buffer. For cold storage reads it is
derived from `rollout.jsonl` plus an empty `RuntimeOverlay`, so historical
approval requests or old persistent-command tool results do not become current
actionable UI state.

`ThreadView.goal` is the active goal projection when a goal exists.
`ThreadView.goal_mode` is always present and defaults to `standard` when there
is no active goal or no explicit sidecar row.

When rollout `response_item` records contain `turn_id`, `ThreadView` uses that
field to place user and assistant messages under the matching runtime turn.
Older rollout files without `turn_id` remain readable and are projected by
message order.

`POST /thread/resume` loads existing state:

```json
{
  "thread_id": "session_...",
  "workspace_root": "."
}
```

Persisted thread context wins on resume. Unsupported request overrides are
reported in `ignored_overrides`.

Thread forks are created through the generic `POST /thread/op` route:

```json
{
  "type": "thread_fork",
  "thread_id": "session_parent",
  "at_turn_id": "turn_1",
  "workspace_root": "."
}
```

`workspace_root` is optional and uses the same canonicalization policy as
`thread/read`. If the parent runtime is already loaded, the loaded runtime's
workspace is used after normal workspace mismatch checks. The parent rollout is
read only; fork creation does not rewrite or append to the parent.

The response names the new cold thread:

```json
{
  "type": "thread_fork",
  "new_thread_id": "session_child",
  "parent_thread_id": "session_parent",
  "fork_point_turn_id": "turn_1"
}
```

The new thread has `ThreadMeta.thread_source = "fork"` and a transcript prefix
built from the parent's rollout through `at_turn_id`. Original turn IDs are
preserved in the forked transcript. The app-server records a fork edge in the
workspace fork edge store, but does not load a runtime for the child; clients
should open it through normal `thread/read` or `thread/resume` flows.

Fork errors:

- `thread not found`: no rollout exists for `thread_id` in the resolved
  workspace.
- `invalid request: cannot fork while a turn is in progress`: the parent has an
  active turn in the current app-server process.
- `fork point turn id <id> was not found in parent history`: `at_turn_id` is not
  present in the parent's response history.
- Persistence errors: rollout or fork-edge storage could not be read or written.

## Thread Goals And Modes

Goals can be created, updated, read, and cleared through the generic
`POST /thread/op` route with `type = thread_goal_set`,
`thread_goal_get`, or `thread_goal_clear`.

Goal mode is a per-goal sidecar value. It is not stored on `ThreadGoal` and it
does not add goal status variants. The supported wire values are:

- `standard`: previous goal behavior.
- `reviewed`: Forge reviewer-gated completion when Forge gates are enabled.
- `intensive`: reviewer-gated completion plus the intensive goal prompt mode
  when Forge gates are enabled.

Creating or updating a goal can include `mode`:

```json
{
  "type": "thread_goal_set",
  "thread_id": "session_...",
  "workspace_root": ".",
  "objective": "Ship the goal mode UI",
  "status": "active",
  "token_budget": 1200,
  "mode": "reviewed"
}
```

The response returns both the goal and the resolved mode:

```json
{
  "type": "thread_goal_set",
  "goal": {
    "thread_id": "session_...",
    "goal_id": "goal_...",
    "objective": "Ship the goal mode UI",
    "status": "active",
    "token_budget": 1200,
    "tokens_used": 0,
    "time_used_seconds": 0,
    "continuation_suppressed": false,
    "continuation_suppressed_after_turn_id": null,
    "created_at_ms": 1710000000000,
    "updated_at_ms": 1710000000000
  },
  "mode": "reviewed"
}
```

Omitting `mode` on a status-only update preserves the existing sidecar value.
Creating a new goal without `mode` uses `standard`. Clearing a goal clears the
sidecar row, so the next `thread_goal_get` response returns
`{ "goal": null, "mode": "standard" }`.

When a loaded runtime observes a goal mode change, clients may receive:

```json
{
  "type": "thread_goal_mode_updated",
  "thread_id": "session_...",
  "goal_id": "goal_...",
  "mode": "reviewed"
}
```

Clients should update the current thread's visible goal mode from this event and
reset it to `standard` on `thread_goal_cleared`.

## Agent Tree

`POST /agent/tree` reads the root thread for a conversation and returns the
nested subagent roster:

```json
{
  "thread_id": "session_...",
  "workspace_root": "."
}
```

The response is rooted at the conversation root thread, even when `thread_id`
names a descendant:

```json
{
  "root": {
    "thread_id": "session_...",
    "root_thread_id": "session_...",
    "depth": 0,
    "agent_path": "root",
    "status": "running",
    "current_tool": "read_file",
    "tokens_used": 52340,
    "children": [
      {
        "thread_id": "session_child",
        "parent_thread_id": "session_...",
        "root_thread_id": "session_...",
        "depth": 1,
        "agent_path": "root/researcher",
        "status": "idle",
        "agent_type": "explorer",
        "agent_role": "research role",
        "agent_nickname": "Rhea",
        "last_task_message": "map the inspector state",
        "last_activity": "also check activeSessionId consumers"
      }
    ]
  }
}
```

`current_tool` and `tokens_used` are additive optional fields. `current_tool`
is the most recent tool invocation start without a matching completed, failed,
or cancelled event for that thread. For approval-waiting tools, a matching
`approval_decision` also resolves that in-flight tool. `tokens_used` is the
thread's latest total token count when token usage has been reported. Older
payloads omit both fields when the values are unavailable.

## Approval Inbox Listing

Clients can list pending approvals across loaded threads with the generic
boundary op route:

```json
{
  "type": "approvals_list",
  "workspace_root": "."
}
```

`workspace_root` is optional. When present, the list is scoped to loaded
runtimes whose live snapshot belongs to that workspace.

The response contains the actionable inbox projection:

```json
{
  "type": "approvals_list",
  "approvals": [
    {
      "thread_id": "session_...",
      "approval_id": "approval_1",
      "kind": "command",
      "summary": "run_command: rm -rf scratch",
      "detail": "rm -rf scratch",
      "goal_id": "goal_...",
      "requested_at_ms": 1710000000000,
      "checkpoint_id": "8f3..."
    },
    {
      "thread_id": "session_...",
      "approval_id": "oq_...",
      "kind": "open_question",
      "summary": "Which customer segment ships first?",
      "detail": "Blocks: rollout targeting",
      "goal_id": "goal_...",
      "requested_at_ms": 1710000001000
    }
  ]
}
```

`kind` is currently `command` or `open_question`; patch approvals can use the
same shape later. For command approvals, `summary` is a compact command line and
`detail` is the full command. For open questions, `approval_id` carries the
question id, `summary` is the question, and `detail` names what it blocks.
`goal_id` is populated when the item belongs to a goal. Command approvals use
the active goal for the loaded runtime; open questions use their persisted goal.
`checkpoint_id` is populated for approval-gated mutating commands when a git
workspace checkpoint was created before the approval request. It is omitted
when the workspace is not a git repository or checkpoint creation failed.

Loaded-runtime invariant: the Approval Inbox lists pending approvals from
loaded runtimes because the in-memory `PolicyManager` is the source of truth
for actionable approvals. A thread waiting for approval is loaded by
definition. Cold historical `approval_requested` events are transcript history,
not inbox items.

Open questions are resolved with:

```json
{
  "type": "open_question_resolve",
  "thread_id": "session_...",
  "question_id": "oq_...",
  "answer": "Ship beta users first.",
  "workspace_root": "."
}
```

The response is:

```json
{
  "type": "open_question_resolved",
  "thread_id": "session_...",
  "question_id": "oq_...",
  "goal_id": "goal_...",
  "status": "resolved"
}
```

Resolving an open question updates the persisted Forge question store and
records an `open_question_resolved` event in the thread rollout.

## Goal Reports

`thread_goal_report` events include the base unattended-goal summary fields
(`goal_id`, `objective`, `final_status`, `turns_run`, `tokens_used`,
`token_budget`, `time_used_seconds`, `changed_files`,
`pending_approvals_count`, `summary`). Forge-enabled reports may also include:

- `open_questions`: unresolved questions still blocking completion, each with
  `question_id`, `question`, and `blocks_what`.
- `review_summary`: the latest Forge review ticket, with `ticket_id`, `status`,
  optional `reviewed_hash`, optional `reject_category`, and optional `findings`.

## Checkpoint Restore

Clients can restore a git workspace checkpoint with the generic boundary op
route:

```json
{
  "type": "checkpoint_restore",
  "workspace_root": ".",
  "checkpoint_id": "8f3..."
}
```

`workspace_root` is required and scopes the restore. The app-server
canonicalizes it with the same workspace override policy used by thread
operations, then restores only that workspace from the named checkpoint.
`checkpoint_id` must come from an approval-derived checkpoint: the id must
match an `approval_requested` event with `checkpoint_id` in a loaded runtime's
persisted rollout for the same workspace. Raw checkpoint refs created outside
the approval flow are rejected.

The response summarizes the restore:

```json
{
  "type": "checkpoint_restored",
  "workspace_root": "/absolute/path/to/workspace",
  "checkpoint_id": "8f3...",
  "status": "restored",
  "message": "workspace restored from checkpoint"
}
```

Restore is rejected as an invalid request if any loaded runtime in the target
workspace has an active turn (`active_turn_id` is present). The check is
conservative across all loaded runtimes for the workspace and runs while a
short-lived workspace restore guard is held. While that guard is active, new
turn starts and approval decisions in the same workspace are rejected before
they can mutate the workspace. Missing checkpoint refs and other restore
failures are returned as invalid requests. Clients should treat restore as
destructive to uncommitted changes made after the checkpoint and present their
own confirmation before calling this op.

## Permission Profiles

`thread/start` accepts an optional `permission_profile`. The only supported
value today is `full_access`.

```json
{
  "workspace_root": ".",
  "cwd": ".",
  "permission_profile": "full_access"
}
```

`full_access` means command execution is not protected by an OS sandbox. It has
no filesystem sandbox, no network sandbox, and no environment isolation. Policy
and approval checks may still require user approval, but they do not create a
hard platform boundary.

Requests for `external` or `managed` currently return an invalid request error:

```text
unsupported permission profile: managed
```

## Turn Lifecycle

`POST /turn/start` submits user input to a loaded runtime:

```json
{
  "thread_id": "session_...",
  "prompt": "Summarize this runtime.",
  "workspace_root": ".",
  "turn_context": {
    "cwd": "src",
    "model": {
      "provider_id": "openai",
      "model_id": "gpt-4.1"
    }
  }
}
```

`prompt` is the legacy text preview. Structured clients may also send `input`;
when `input` is non-empty, it is the authoritative user input for the turn:

```json
{
  "thread_id": "session_...",
  "prompt": "Describe this screenshot.",
  "workspace_root": ".",
  "input": [
    {
      "type": "text",
      "text": "Describe this screenshot."
    },
    {
      "type": "local_image",
      "path": "/Users/me/Desktop/screenshot.png",
      "detail": "high"
    }
  ]
}
```

Supported structured input parts are `text`, `local_image`, and `image_url`.
Local image paths are validated before a new turn is recorded. Historical image
parts are preserved in rollout history, but prompt views strip images to a text
placeholder when the selected model is known to accept text only.

`turn_context.cwd` is optional. When present, it applies only to that turn and
does not rewrite the thread snapshot's durable `cwd`.

`turn_context.model` is optional. When present, it is a durable model identity
object with `provider_id` and `model_id`; it never carries resolved provider
endpoints, API keys, OAuth tokens, or other credential material. The app-server
resolves that `ModelRef` before submitting the turn to the runtime actor, and a
running turn keeps the resolved model frozen until completion or interruption.

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
- `token_count`
- `thread_goal_updated`
- `thread_goal_mode_updated`
- `thread_goal_cleared`
- `thread_goal_continuation_started`
- `thread_goal_continuation_suppressed`
- `thread_goal_turn_started`
- `thread_goal_report`
- `runtime_error`

The LLM adapter does not currently stream token deltas. A full assistant message
is emitted as an `assistant_turn` event.

`token_count` carries optional `TokenUsageInfo`:

```json
{
  "type": "token_count",
  "info": {
    "total_token_usage": {
      "input_tokens": 40,
      "cached_input_tokens": 5,
      "output_tokens": 10,
      "reasoning_output_tokens": 2,
      "total_tokens": 52
    },
    "last_token_usage": {
      "input_tokens": 40,
      "cached_input_tokens": 5,
      "output_tokens": 10,
      "reasoning_output_tokens": 2,
      "total_tokens": 52
    },
    "model_context_window": 128000
  }
}
```

`token_count` events are replayable and filterable, but they do not create
visible `ThreadItem` entries in `thread/read`.

## Token Budget And Compaction

The runtime can compact prompt history before a model context window is filled.
The relevant environment variables are:

```text
EXAGENT_MODEL_CONTEXT_WINDOW
EXAGENT_AUTO_COMPACT_TOKEN_LIMIT
```

Both values are positive integer token counts. If only
`EXAGENT_MODEL_CONTEXT_WINDOW` is configured, ExAgent derives the auto-compact
threshold as 90% of that window. If both are configured, the explicit threshold
is clamped to the same 90% headroom.

Compaction is local and logical. It does not rewrite `rollout.jsonl`. A
successful compaction appends a `compacted` rollout checkpoint with
`replacement_history`; replay uses the latest replacement history to rebuild the
model-visible conversation. Earlier rollout lines remain available for audit.

When compaction runs in a loaded runtime, clients may see
`compaction_written`. The durable checkpoint is the `compacted` rollout item;
`events/replay` snapshots expose the latest compaction through
`snapshot.latest_compaction`.

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
  "event_kinds": ["assistant_turn", "tool_result", "token_count"]
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
    "permission_profile": "full_access",
    "latest_compaction": null,
    "open_exec_session_count": 0,
    "conversation_message_count": 2,
    "pending_approval_count": 0
  }
}
```

`events/replay` is the durable replay source for persisted runtime events. It
does not depend on a runtime still being loaded in memory. When a runtime is
loaded, `open_exec_session_count` and `pending_approval_count` are projected
from the runtime's `RuntimeOverlay`. When reading from cold storage, those live
counts are projected from an empty overlay and therefore return `0`.

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
That view includes the durable materialized state, the `RuntimeOverlay`, and a
bounded recent event window. It is the right source for current UI state, active
turn status, pending approvals, and open persistent exec sessions.

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
clears pending approvals from the runtime overlay, cancels policy-side waiters,
and records `turn_interrupted`.

Risky command approvals are exposed as events and inbox items:

- `approval_requested`: command execution is waiting for a decision. It may
  include `checkpoint_id` when a pre-action workspace checkpoint exists.
- `approval_decision`: approval or denial has been recorded.
- `open_question_recorded`: a Forge goal recorded a deferred question.
- `open_question_resolved`: a deferred Forge question was answered or dismissed
  through the inbox.

Approval decisions are submitted with the `approval_decision` boundary op using
the `thread_id`, optional `turn_id`, `approval_id`, and `decision`.

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

Future boundary versions should replace `snapshot_path` and `events_path` with
explicit storage metadata such as `rollout_path`; in v2 they remain
compatibility-only fields.
