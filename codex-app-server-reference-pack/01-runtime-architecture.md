# 01. Runtime Architecture

## What "Runtime" Means

An agent runtime is the system that turns model reasoning into controlled,
observable work.

The model decides what to say or which tool to call. The runtime decides how
that work is represented, queued, executed, streamed, checked, persisted, and
resumed.

## Main Runtime Shape

```text
Client / UI / SDK
  |
  v
API Gateway
  - parse requests
  - authenticate client
  - validate method shape
  - translate public API types into internal runtime types
  |
  v
Thread Manager
  - create/resume/fork/archive threads
  - load persisted state
  - own live thread registry
  |
  v
TurnContext Builder
  - model/provider
  - cwd/environment
  - tool availability
  - permissions/sandbox/approval policy
  - instructions
  - output schema
  |
  v
Op Queue
  - UserInput
  - UserInputWithTurnContext
  - Interrupt
  - Compact
  - Shutdown
  |
  v
Core Agent Loop
  - build prompt/context
  - call model
  - parse response
  - dispatch tools
  - feed tool results back to model
  - decide completion
  |
  v
Event Bus
  - turn started
  - message deltas
  - tool started/completed
  - command output
  - file changes
  - approval requests
  - errors
  - turn completed
  |
  v
Client UI
```

## Design Principle

The runtime input is not just a prompt.

It is:

```text
UserInput + TurnContext
```

That combined payload becomes an internal operation:

```text
Op::UserInput(...)
Op::UserInputWithTurnContext(...)
```

Then the core runtime consumes it.

## Why This Matters

A simple chatbot can block until it has an answer. A real agent cannot.

A real agent may:

- run for minutes
- call many tools
- need approval before executing a command
- change files
- emit partial output
- be interrupted
- be resumed later
- be inspected after failure

That is why the runtime needs:

- a state model
- an operation queue
- an event stream
- a tool runtime
- persistence
- permission boundaries

## Minimal Version For Your Own Agent

Start with these components:

```text
AgentServer
ThreadStore
ThreadManager
TurnContextBuilder
OpQueue
RuntimeLoop
ModelClient
ToolRouter
EventBus
Persistence
```

Everything else can be added later.
