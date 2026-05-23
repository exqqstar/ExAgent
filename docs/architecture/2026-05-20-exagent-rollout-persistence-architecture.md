# ExAgent Rollout Persistence Architecture

**Date:** 2026-05-20
**Status:** Implemented in current working tree
**Reference:** Codex rollout + session ContextManager design

## Executive Summary

ExAgent has moved from the previous `snapshot.json + events.jsonl` dual-file persistence model to a Codex-style single-authority rollout model for new sessions.

Target invariant:

```text
rollout.jsonl is the only durable source of truth.
ThreadSession is the runtime owner.
ContextManager is a stateful in-memory history manager owned by ThreadSession.
RuntimeOverlay owns live-only state that rollout replay must not reconstruct.
```

This replaces the current split:

```text
snapshot.json restores state
events.jsonl explains what happened
ThreadEventRecorder keeps both loosely aligned
```

with:

```text
rollout.jsonl records durable facts
ThreadSession replays those facts into runtime memory
ContextManager owns prompt-visible history and context baseline
RuntimeOverlay owns pending approvals and open exec session refs
```

The goal is not to copy every Codex field immediately. The goal is to adopt the same ownership model:

```text
persistence authority -> rollout
runtime authority     -> ThreadSession
prompt history owner  -> ContextManager
live-only owner       -> RuntimeOverlay
```

## Previous ExAgent Architecture

Before this migration, the runtime shape was already session-centered:

```text
CLI / HTTP
  -> AppServerService
  -> ThreadManager
  -> ThreadRuntime
  -> ThreadRuntimeLoop
  -> ThreadSession
      -> Agent sampling
      -> ToolCallRuntime
      -> ThreadEventRecorder
      -> live_state.snapshot
```

The previous persistence shape was snapshot/event based:

```text
ThreadSession
  -> ThreadEventRecorder
      -> writes snapshot.json
      -> appends events.jsonl
      -> updates live_state
      -> broadcasts RuntimeEvent
```

Previous module responsibilities:

```text
src/state/session.rs
  - SessionSnapshot schema
  - conversation
  - workspace_root / cwd
  - reference_turn_context
  - open_exec_sessions
  - pending_approvals
  - latest_compaction

src/state/events.rs
  - RuntimeEvent schema
  - event ids
  - lifecycle/tool/approval/error/exec-output event payloads

src/state/transcript.rs
  - session path layout
  - snapshot.json read/write
  - events.jsonl append/replay

src/runtime/thread_session/events.rs
  - ThreadEventRecorder
  - event id assignment
  - snapshot checkpoint writes
  - event log writes
  - live event buffer and broadcasts

src/runtime/context.rs
  - currently a projection helper
  - ContextManager is a zero-sized namespace
  - state still lives in SessionSnapshot

src/runtime/thread_session/turn.rs
  - turn loop
  - mutates snapshot.conversation
  - calls ContextManager::for_prompt(&snapshot)
```

That architecture fixed the earlier `Agent` ownership problem, but it still had two durable views of a thread.

## Implemented Shape

The current implementation uses this ownership model:

```text
ThreadSession
  owns RolloutStore
  owns ContextManager
  owns ThreadEventRecorder
  owns live runtime state
```

New threads write `.exagent/threads/<thread_id>/rollout.jsonl` and do not create `snapshot.json` or `events.jsonl`. Legacy `snapshot.json + events.jsonl` files are no longer runtime or migration inputs; v2 may still return their paths as compatibility-only response fields.

`ContextManager` is now a real stateful object:

```rust
pub(crate) struct ContextManager {
    items: Vec<ConversationMessage>,
    history_version: u64,
    reference_turn_context: Option<TurnContextItem>,
}
```

The turn loop records context messages, user messages, assistant messages, and tool results into `ContextManager` and persists the same prompt-visible items as `RolloutItem::ResponseItem`. Sampling uses `context_manager.for_prompt()` and does not derive prompts from `SessionSnapshot`.

`ThreadEventRecorder` now assigns event ids, updates the live event buffer, broadcasts events, and persists selected `RuntimeEvent`s as `RolloutItem::EventMsg`. It no longer checkpoints `snapshot.json` or appends `events.jsonl` for new sessions.

`RuntimeOverlay` is the in-memory holder for state that only has meaning while a
runtime process is alive:

```text
src/runtime/thread_session/overlay.rs
  - pending_approvals
  - open_exec_sessions
  - helpers for applying ToolEffect lifecycle updates
```

Cold rollout replay intentionally creates an empty `RuntimeOverlay`. Historical
`ApprovalRequested` events and old persistent-command tool results remain
history, not current waiters or running processes.

## Current Pain

### 1. Dual Durable Authority

`snapshot.json` and `events.jsonl` are not equivalent:

```text
snapshot.json
  - restores latest materialized state
  - loses detailed ordering unless also reading events

events.jsonl
  - shows what happened
  - cannot fully reconstruct all snapshot fields without special logic
```

That makes every new state field ambiguous:

```text
Should it go into snapshot?
Should it go into events?
Should it go into both?
Which one wins on resume?
```

### 2. ContextManager Is Not Yet The Codex Layer

Current ExAgent:

```rust
pub(crate) struct ContextManager;
```

It has associated functions and no fields. It does not own history.

Codex:

```rust
pub(crate) struct ContextManager {
    items: Vec<ResponseItem>,
    history_version: u64,
    token_info: Option<TokenUsageInfo>,
    reference_context_item: Option<TurnContextItem>,
}
```

Codex's `ContextManager` is a session-lifetime memory object. ExAgent's current `ContextManager` is a projection namespace.

### 3. Resume Cannot Become Codex-Style While Snapshot Is Authority

Codex can resume by scanning rollout, finding the latest compaction checkpoint, and replaying the surviving tail.

With snapshot authority, ExAgent resume is fast but structurally brittle:

```text
read snapshot
read events if UI wants timeline
trust that snapshot and events agree
```

That does not scale cleanly to compaction, rollback, fork, or history reconstruction.

## Codex Reference Architecture

Codex has three separate responsibilities:

```text
Durable:
  rollout.jsonl

Runtime:
  Session
    -> state: Mutex<SessionState>

Prompt/history:
  SessionState.history: ContextManager
```

Codex rollout schema:

```rust
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMetaLine),
    ResponseItem(ResponseItem),
    Compacted(CompactedItem),
    TurnContext(TurnContextItem),
    EventMsg(EventMsg),
}
```

Important Codex behavior:

```text
record_conversation_items(...)
  -> record_into_history(...)
  -> persist RolloutItem::ResponseItem
  -> emit RawResponseItem event

record_context_updates_and_set_reference_context_item(...)
  -> build full context or context diff
  -> record context messages into history
  -> persist RolloutItem::TurnContext
  -> update ContextManager.reference_context_item

resume
  -> read rollout items
  -> reverse scan for Compacted.replacement_history and latest TurnContext
  -> replay surviving tail into ContextManager
```

`EventMsg` exists in rollout, but Codex filters it with an event persistence policy. It does not persist every streaming delta or begin/end noise by default.

## Target ExAgent Architecture

ExAgent should not introduce a new user-facing `SessionState` concept. The target should be:

```text
ThreadSession
  owns RolloutStore
  owns ContextManager
  owns runtime-only fields
  owns turn loop
```

Target shape:

```text
CLI / HTTP
  -> AppServerService
  -> ThreadManager
  -> ThreadRuntime
  -> ThreadRuntimeLoop
  -> ThreadSession
      -> RolloutStore
      -> ContextManager
          -> conversation items
          -> reference_turn_context
          -> token_info later
      -> RuntimeOverlay
          -> pending approvals
          -> open exec session refs
      -> runtime view/status
      -> Agent sampling
      -> ToolCallRuntime
```

The persistence flow becomes:

```text
append RolloutItem
  -> apply same item to ThreadSession-owned runtime objects
  -> broadcast selected RuntimeEvent
```

The sampling flow becomes:

```text
ThreadSession.context.for_prompt()
  -> Agent.sample_assistant_turn(...)
  -> ThreadSession records assistant/tool rollout items
  -> ContextManager updates history
```

## Target RolloutItem Schema

Stage 1 should define a minimal schema that maps to current ExAgent facts:

```rust
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMeta),
    ResponseItem(ConversationMessage),
    TurnContext(TurnContextItem),
    Compacted(CompactedItem),
    EventMsg(RuntimeEvent),
}
```

Minimal `SessionMeta`:

```text
thread_id
root_thread_id
parent_thread_id
spawned_by_turn_id
agent_role
workspace_root
initial_cwd
created_at
```

Minimal `ResponseItem`:

```text
ConversationMessage
  - role
  - content
  - tool_call_id
  - tool_calls
  - injected
```

Minimal `TurnContext`:

```text
turn_id
workspace_root
cwd
model
policy_mode
command_timeout_secs
max_output_bytes
current_utc_date
```

Minimal `Compacted`:

```text
message
replacement_history: Option<Vec<ConversationMessage>>
```

Minimal `EventMsg`:

```text
selected RuntimeEvent only
```

Do not aim for Codex's full `TurnContextItem` field set in the first pass. Add fields only when ExAgent runtime has a real producer and consumer for them.

## Event Persistence Policy

`RolloutItem::EventMsg` must be filtered. Otherwise rollout becomes a streaming dump.

Initial policy:

```text
Always persist:
  SessionMeta
  ResponseItem
  TurnContext
  Compacted

Persist selected events:
  TurnStarted
  TurnCompleted
  TurnInterrupted
  RuntimeError
  ApprovalRequested
  ApprovalResolved

Do not persist initially:
  ExecOutput streaming chunks
  transient progress events
  purely broadcast-only events
```

This policy can expand later if UI replay needs more durable event classes.

## Module Responsibility Changes

### `src/state/rollout.rs` New

Owns rollout persistence.

Responsibilities:

```text
RolloutItem schema
RolloutStore
rollout path layout
append items
read items
flush
selected EventMsg persistence policy
```

### `src/runtime/context.rs` Changes

Becomes the real in-memory history owner.

From:

```text
ContextManager as ZST helper
SessionSnapshot owns conversation and reference_turn_context
```

To:

```text
ContextManager {
  items: Vec<ConversationMessage>,
  reference_turn_context: Option<TurnContextItem>,
  history_version: u64,
  token_info: later
}
```

Responsibilities:

```text
record_items(...)
for_prompt(...)
raw_items(...)
replace_history(...)
apply_context_updates(...)
set_reference_turn_context(...)
reference_turn_context(...)
```

### `src/runtime/thread_session/mod.rs` Changes

Becomes the direct runtime owner.

Responsibilities:

```text
owns RolloutStore
owns ContextManager
owns RuntimeOverlay
owns runtime live view
loads thread from rollout
hydrates ContextManager from rollout
routes operations to turn loop
```

No new public `SessionState` concept is needed. If an internal grouping type becomes useful, prefer a name like `ThreadSessionRuntime` and keep it private.

### `src/runtime/thread_session/turn.rs` Changes

Stops mutating `SessionSnapshot`.

New flow:

```text
build TurnContextItem
record context updates into ContextManager
append RolloutItem::TurnContext
append RolloutItem::ResponseItem(user)
sample from ContextManager::for_prompt()
append assistant/tool ResponseItems
apply tool effects to RuntimeOverlay
persist selected EventMsg
```

RuntimeOverlay is never included in prompt history. It may affect UI projection
and interrupt behavior, but not model-visible conversation.

### `src/runtime/thread_session/overlay.rs` New

Owns live-only runtime state.

Responsibilities:

```text
track open persistent exec sessions while process handles exist
track pending approvals while policy waiters exist
clear approvals on interrupt or decision
drop open exec refs when lifecycle reports not-running
stay empty during cold rollout replay
```

### `src/runtime/thread_session/events.rs` Changes

Stops owning snapshot/event file writes.

New responsibility:

```text
event id assignment
live event buffer
broadcast
selected EventMsg -> RolloutStore
```

The type may be renamed later because `ThreadEventRecorder` currently implies persistence ownership.

### `src/state/session.rs` Changes

`SessionSnapshot` stops being durable authority.

Transition options:

```text
Phase 1:
  keep SessionSnapshot for protocol views and compatibility fields

Phase 2:
  stop writing snapshot.json for new sessions

Phase 3:
  narrow SessionSnapshot to derived protocol/runtime projection data
```

### `src/state/transcript.rs` Changes

Keeps JSON helpers and v2 compatibility path construction only.

Target:

```text
runtime state uses rollout paths
snapshot_path/events_path are compatibility-only response fields
no runtime code relies on transcript snapshot/event dual files
```

### `src/app_server/thread_manager.rs` Changes

Thread discovery and resume should use rollout metadata, not snapshot paths.

Responsibilities:

```text
find rollout by thread_id
start ThreadRuntime from rollout history
reject workspace mismatch from rollout SessionMeta / latest TurnContext
```

### `src/app_server/protocol.rs` Changes

Protocol should continue returning stable thread views. It should not expose raw rollout internals as the main API.

Current `snapshot`-shaped responses should become derived views from `ThreadSession`.

## Resume And Reconstruction

Initial implementation can replay forward:

```text
read rollout.jsonl
for item in items:
  apply item to ThreadSession runtime objects
```

But the design must reserve the Codex path:

```text
reverse scan:
  find newest Compacted.replacement_history
  find newest TurnContext baseline
  find latest needed runtime metadata

forward replay:
  start from replacement_history
  replay surviving tail
```

That is why `Compacted.replacement_history` belongs in the schema immediately, even if compaction execution is implemented later.

## Architecture Comparison

```text
Current ExAgent
  durable:
    snapshot.json + events.jsonl
  runtime:
    ThreadSession.live_state.snapshot
  prompt:
    ContextManager::for_prompt(&snapshot)
  context baseline:
    SessionSnapshot.reference_turn_context

Target ExAgent
  durable:
    rollout.jsonl
  runtime:
    ThreadSession owns ContextManager + RuntimeOverlay
  prompt:
    ContextManager::for_prompt()
  context baseline:
    ContextManager.reference_turn_context
    persisted as RolloutItem::TurnContext
  live-only:
    RuntimeOverlay, never rebuilt from cold rollout

Codex
  durable:
    rollout.jsonl
  runtime:
    Session.state: SessionState
  prompt:
    SessionState.history: ContextManager
  context baseline:
    ContextManager.reference_context_item
    persisted as RolloutItem::TurnContext
```

## Migration Strategy

The migration should be direct but staged.

```text
Stage 1:
  introduce rollout schema/store and stateful ContextManager
  keep old snapshot/events only as compatibility fallback

Stage 2:
  switch new session writes to rollout
  replay rollout into ThreadSession on resume
  keep tests proving old snapshot/events can migrate

Stage 3:
  remove snapshot.json/events.jsonl as new-session outputs
  keep one migration path or declare a breaking change
  add compact replacement_history support
```

## Acceptance Criteria

Architecture is complete when:

```text
New sessions can resume from rollout.jsonl alone.
ThreadSession owns ContextManager as a stateful object.
ContextManager owns conversation items and reference_turn_context.
RuntimeOverlay owns pending approvals and open exec session refs.
Sampling prompt is derived only from ContextManager::for_prompt().
RuntimeOverlay is not included in model prompt history.
SessionSnapshot is not written for new sessions.
events.jsonl is not written for new sessions.
Selected RuntimeEvent entries are persisted through RolloutItem::EventMsg.
Cold rollout replay leaves pending approvals and open exec sessions empty.
Compacted replacement_history has schema support.
Legacy snapshot/events can be migrated or the breaking change is explicit.
```

## Non-Goals For First Implementation

Do not implement these in the first rollout migration pass:

```text
full Codex TurnContextItem field parity
token accounting
prompt cache
automatic compaction trigger
reverse-scan optimized resume
SQLite thread index
forked subagent lineage beyond current fields
```

The first pass should establish the correct authority model. Feature parity can follow after the ownership boundary is stable.
