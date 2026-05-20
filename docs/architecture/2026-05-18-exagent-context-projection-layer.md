# ExAgent Context Projection Layer

**Date:** 2026-05-18
**Status:** Implemented in current working tree
**Scope:** Historical stage 1 context projection. The later rollout migration is now implemented separately in `2026-05-20-exagent-rollout-persistence-architecture.md`.

## Goal

This stage added a Codex-style context projection layer before the rollout migration. At the time, it deliberately avoided changing the existing `snapshot.json + events.jsonl` storage architecture; that storage model has since been replaced for new sessions.

The target invariant is:

```text
Committed thread state lives in ThreadSession.
Runtime/environment context is committed into that state before the user message.
Every model sampling request is derived through ContextManager::for_prompt().
```

This removes the most dangerous form of "working conversation" drift: local prompt construction that evolves separately from committed session history.

## Before

Previously, prompt construction was effectively:

```text
ThreadSession live snapshot
  -> clone snapshot.conversation
  -> push user message
  -> sampling from cloned conversation
```

That was acceptable for simple chat history, but weak for runtime background. Workspace, cwd, model, policy mode, command limits, and UTC date were not represented as first-class turn context. If later code added local prompt-only background, the model could see context that was not committed into replayable session state.

The risk was not that cloning is always wrong. A short-lived prompt payload is fine. The risk is a second mutable conversation that is updated independently from `ThreadSession`.

## After

The first stage had three explicit phases:

```text
Persistent state:
  SessionSnapshot.conversation
  SessionSnapshot.reference_turn_context
  events.jsonl remained unchanged

Runtime state:
  ThreadSession live_state.snapshot
  ThreadEventRecorder writes snapshot/events

Sampling:
  ContextManager::for_prompt(&snapshot)
```

After the rollout migration, the same projection rule still holds, but the live
history owner is a stateful `ContextManager` owned by `ThreadSession`, and
sampling uses `context_manager.for_prompt()`.

On each user turn:

```text
ThreadSession::handle_user_input_inner
  -> load live snapshot clone
  -> build PromptContext from AgentConfig + TurnPaths
  -> ContextManager::apply_context_updates(snapshot, prompt_context)
      - initial turn injects full runtime/environment context
      - later turns inject only diffs
      - snapshot.reference_turn_context becomes the new baseline
  -> append user message
  -> record TurnStarted with updated snapshot
  -> run_session_turn
      -> ContextManager::for_prompt(snapshot)
      -> Agent samples model
      -> ThreadSession applies assistant/tool effects
```

The model now sees the same context that replay, live view, and persisted snapshot see.

## Codex Mapping

Codex uses a single rollout log as the durable source, and has runtime state plus context projection on top of it. The important design idea is not the exact file shape. The important idea is that model prompt construction is a projection from committed session state.

```text
Codex:
  rollout.jsonl
    -> SessionState.history / ContextManager
    -> ContextManager.for_prompt()
    -> sampling

ExAgent stage 1:
  snapshot.json + events.jsonl
    -> ThreadSession live_state.snapshot
    -> ContextManager::for_prompt()
    -> sampling

ExAgent after rollout migration:
  rollout.jsonl
    -> ThreadSession-owned ContextManager
    -> context_manager.for_prompt()
    -> sampling
```

`TurnContextItem` is ExAgent's baseline context item. It is not a second conversation. It is the reference used to know whether runtime/environment context changed between turns.

## Module Responsibilities

### `src/state/session.rs`

Owns persisted session schema.

New responsibility:

```text
SessionSnapshot.reference_turn_context: Option<TurnContextItem>
```

`TurnContextItem` stores the last committed context baseline:

```text
workspace_root
cwd
model
policy_mode
command_timeout_secs
max_output_bytes
current_utc_date
```

Old snapshots deserialize with `reference_turn_context = None`, so existing sessions remain compatible.

`current_utc_date` is optional for schema compatibility. New turns populate it with UTC date from `OffsetDateTime::now_utc()`. Snapshots written by the earlier stage-1 draft that used `current_date` are accepted as an alias.

### `src/runtime/context.rs`

Owns context projection.

Responsibilities:

```text
PromptContext::for_turn(config, TurnPaths { workspace_root, cwd })
  - gathers runtime values from AgentConfig
  - takes per-turn workspace/cwd from explicit validated paths

ContextManager::apply_context_updates(...)
  - compares current TurnContextItem with snapshot.reference_turn_context
  - appends initial context messages or diff messages
  - updates snapshot.reference_turn_context

ContextManager::for_prompt(...)
  - derives the model prompt from committed snapshot conversation
```

This is the only place that should know how runtime/environment context becomes prompt-visible conversation.

### `src/runtime/thread_session/turn.rs`

Owns turn ordering and state writes.

New responsibility:

```text
context projection happens before appending the user message
sampling goes through ContextManager::for_prompt()
```

That ordering matters. A user turn is interpreted under the context that was current at the start of that turn.

### `src/runtime/agent.rs`

Remains an execution component.

It now exposes read-only config access so `ThreadSession` can build `PromptContext`. It does not become the owner of context history.

### `src/model/types.rs`

Adds `ConversationMessage::system(...)` so runtime context can be injected as a system message without ad hoc construction.

### `src/runtime/policy.rs`

Makes `PolicyMode` serializable and gives it a stable string form for `TurnContextItem`.

## Message Shape

Initial turn context writes two system messages:

```text
system: Runtime context:
  model
  policy_mode
  command_timeout_secs
  max_output_bytes

system: Environment context:
  workspace_root
  cwd
  current_utc_date
```

Later changes write only diffs:

```text
system: Runtime context updated:
  model: old -> new

system: Environment context updated:
  cwd: old -> new
```

This keeps context visible to the model and replayable from committed history, while avoiding repeated full context spam on unchanged turns.

## Why Not Add `Developer` Role Now

Codex has richer internal context roles and item types. ExAgent's current `ConversationMessage` boundary only needs system, user, assistant, and tool behavior for this stage.

Adding `Developer` now would expand the public model role surface before the rest of the runtime needs it. The conservative choice is:

```text
runtime configuration -> system message
environment/cwd/UTC date -> system message
```

Both messages are system-authored because neither one is user input. If later compaction or model-specific prompt adapters need a developer role, that should be introduced as part of a model role normalization pass, not as incidental context work.

## Why Store A Reference Context

`reference_turn_context` answers one question:

```text
What runtime/environment context did we last commit into conversation?
```

It is not an alternative source of prompt history. The prompt still comes from `snapshot.conversation`.

Without this baseline, every turn would need to either:

```text
append full context every time
```

or:

```text
infer prior context by parsing old message text
```

Both are worse. A typed baseline makes diffing deterministic and avoids parsing prompt text back into state.

## Tradeoffs

Benefits:

- Model prompt is derived from committed session state.
- Runtime/environment context is replayable and visible in snapshots.
- First-stage compatibility is preserved for old snapshots.
- Context update logic is isolated in one module.
- The existing storage architecture remains stable while the runtime design improves.

Costs:

- Conversation length increases by two messages on the first turn.
- Runtime context is represented as messages, not a richer internal prompt item enum.
- `reference_turn_context` is an extra snapshot field that must be kept in sync with context messages.

These costs are acceptable for stage 1 because they establish the correct ownership boundary without forcing storage compaction or rollout-log migration at the same time.

## Acceptance Criteria

Implemented criteria:

- Old snapshots without `reference_turn_context` still deserialize.
- First turn injects runtime and environment context before the user message.
- Unchanged context does not inject duplicate context messages.
- Changed context injects only runtime/environment diffs.
- `ThreadSession` sampling uses `ContextManager::for_prompt()`.
- Live runtime view shows committed context messages and ignores disk mutation after runtime load.
- Existing app-server, runtime, tool, policy, and architecture guard tests pass.

Explicit non-goals:

- No storage migration from dual files to single rollout log.
- No compaction or `replacement_history` implementation.
- No `Developer` role addition.
- No change to approval, exec session, or tool effect ownership beyond prompt context routing.

## Next Stage

The next architecture stage should be compaction-aware prompt projection:

```text
TurnContextItem
  -> compact/replacement history checkpoints
  -> ContextManager::for_prompt() token budgeting
  -> optional model-role normalization
  -> reject workspace_root drift as an invariant violation instead of projecting it as a normal context diff
```

Storage can be revisited separately. A Codex-like rollout log may still be cleaner long term, but this stage deliberately keeps `snapshot.json + events.jsonl` stable so context ownership can be proven first.
