# Codex app-server Reference Pack

This folder is a compact reference pack for studying the Codex `app-server`
runtime design and adapting its ideas into another agent system.

It is intentionally not a full copy of Codex. The goal is to preserve the
architecture, the key flow, the important type boundaries, and the source-code
entry points that matter when designing your own agent runtime.

## Recommended Reading Order

1. `01-runtime-architecture.md`
   - The high-level runtime model.
   - Why an agent is more than a model call.

2. `02-app-server-flow.md`
   - The concrete Codex app-server flow.
   - Startup, initialize, `thread/start`, `turn/start`, event streaming.

3. `03-key-types-and-ops.md`
   - Thread / Turn / Item.
   - TurnContext.
   - Op queue.
   - Events.

4. `04-code-map.md`
   - Original Codex source locations and what each function does.

5. `05-adaptation-guide.md`
   - How to apply these ideas to your own agent.

6. `PROMPT_FOR_YOUR_AGENT.md`
   - A ready-to-use prompt you can give to your own agent together with this
     folder.

7. `visuals/`
   - `app-server-call-canvas.html`: interactive call canvas.
   - `app-server-learning.html`: diagram-based learning page.

## Core Idea

Codex app-server is useful because it shows an agent as a runtime:

```text
Client
  -> API boundary
  -> Thread / Turn model
  -> Config + TurnContext
  -> Op queue
  -> Core loop
  -> Tool runtime
  -> Event stream
  -> Client UI
```

The most important lesson is that an agent should not be designed as:

```text
handleMessage(prompt) -> finalAnswer
```

It should be designed as:

```text
request
  -> validate
  -> build TurnContext
  -> build Op
  -> enqueue Op
  -> runtime consumes Op
  -> model/tool loop emits Events
  -> client consumes event stream
  -> state persists for resume/fork/debugging
```

## What To Copy Into Your Own Agent Design

- Use `Thread`, `Turn`, and `Item` instead of only `messages[]`.
- Treat `TurnContext` as part of the task input.
- Convert external requests into internal `Op` commands.
- Make event streaming the main output path for long-running agent work.
- Keep API protocol separate from core runtime internals.
- Put tool execution behind validation, permission checks, and normalized
  result handling.
- Persist enough state to inspect, resume, replay, or debug a turn.

## What Not To Copy Initially

Codex has production-scale concerns that are not needed for a first version:

- multiple transports
- enterprise policy layers
- remote control
- plugin marketplaces
- multi-agent thread trees
- platform-specific sandbox backends
- complex analytics and tracing

Start with the runtime skeleton first.
