# 05. Adaptation Guide

This guide translates Codex app-server ideas into a smaller agent you can build.

## Phase 1: Define Runtime Objects

Start with data shapes. Do not start with prompts.

Minimum objects:

```ts
type Thread = {
  id: string
  status: "idle" | "running" | "archived"
  defaultContext: Partial<TurnContext>
  createdAt: number
  updatedAt: number
}

type Turn = {
  id: string
  threadId: string
  status: "queued" | "running" | "completed" | "failed" | "interrupted"
  input: UserInput[]
  context: TurnContext
}

type Item = {
  id: string
  threadId: string
  turnId: string
  type: string
  payload: unknown
  createdAt: number
}
```

## Phase 2: Define API Methods

Start with:

```text
initialize
thread/start
thread/read
turn/start
turn/interrupt
events/subscribe
```

If using HTTP:

```text
POST /initialize
POST /threads
GET  /threads/:id
POST /threads/:id/turns
POST /turns/:id/interrupt
GET  /threads/:id/events
```

If using JSON-RPC:

```text
initialize
thread/start
thread/read
turn/start
turn/interrupt
```

## Phase 3: Build TurnContext

Treat context as part of the task.

```ts
type TurnContext = {
  model: string
  cwd?: string
  instructions: string[]
  tools: ToolSpec[]
  permissions: PermissionProfile
  environment?: string
  outputSchema?: unknown
}
```

Build it from:

```text
global config
thread defaults
request overrides
policy constraints
available tools
```

Validate it before accepting the turn.

## Phase 4: Use an Op Queue

External requests should become internal Ops:

```ts
type Op =
  | { type: "user_input"; threadId: string; turnId: string; input: UserInput[]; context: TurnContext }
  | { type: "interrupt"; threadId: string; turnId?: string }
  | { type: "shutdown"; threadId: string }
```

Queue gives you:

```text
ordering
interrupt handling
retries
state transitions
decoupled API/core
```

## Phase 5: Emit Events

Do not wait for final answer.

Emit:

```text
turn_started
assistant_delta
tool_started
tool_completed
approval_requested
error
turn_completed
```

Your UI should be built around events.

## Phase 6: Tool Runtime

A tool call should pass through:

```text
registry lookup
schema validation
permission check
approval if needed
execution environment
result normalization
event emission
```

Minimal tool interface:

```ts
type Tool = {
  name: string
  description: string
  inputSchema: unknown
  requiresApproval?: boolean
  run(input: unknown, context: ToolContext): Promise<ToolResult>
}
```

## Phase 7: Persistence

Persist:

```text
Thread metadata
Turn metadata
Items/events
Tool calls/results
Final assistant messages
Errors
```

This enables:

```text
resume
fork
debugging
auditing
UI replay
context reconstruction
```

## A Good Minimal Architecture

```text
AgentServer
  - public API

ThreadManager
  - live thread registry
  - start/resume/fork/archive

RuntimeEngine
  - Op queue
  - one session loop per thread

AgentCore
  - build prompt
  - call model
  - route tool calls
  - decide completion

ToolRuntime
  - validate and execute tools

EventBus
  - publish events to subscribers

Store
  - persist thread/turn/item state
```

## Common Mistake

Avoid this:

```text
POST /chat -> call model -> maybe call tool -> return final answer
```

Prefer this:

```text
POST /turn/start -> return turnId
event stream -> progress and final answer
```
