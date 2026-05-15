# 03. Key Types and Ops

## Thread / Turn / Item

This is the most useful conceptual model to copy.

```text
Thread
  Long-lived conversation or task context.

Turn
  One user request inside a thread.

Item
  A fine-grained event or artifact inside a turn.
```

Examples of items:

```text
user message
assistant message delta
reasoning delta
tool call started
tool call completed
shell command output
file change
approval request
error
turn completed
```

This is stronger than a plain `messages[]` array because an agent does work,
not just chat.

## TurnContext

A turn is not only user input. A turn also needs execution context.

Useful shape:

```ts
type TurnContext = {
  model: string
  provider?: string
  reasoningEffort?: string
  cwd?: string
  environment?: string
  tools: ToolSpec[]
  permissions: PermissionProfile
  approvalPolicy: ApprovalPolicy
  instructions: InstructionBundle
  outputSchema?: unknown
  clientMetadata?: Record<string, string>
}
```

The abstract categories are:

```text
model context
execution context
tool context
permission context
instruction context
output context
trace/client metadata
```

## Op

An `Op` is the internal command protocol consumed by the runtime.

External requests should not call the model loop directly. They should become
Ops first.

Useful minimal shape:

```ts
type Op =
  | { type: "user_input"; items: UserInput[]; context?: TurnContext }
  | { type: "interrupt"; turnId?: string }
  | { type: "compact" }
  | { type: "set_thread_name"; name: string }
  | { type: "shutdown" }
```

In Codex terms:

```text
Op::UserInput
Op::UserInputWithTurnContext
Op::Interrupt
Op::Compact
Op::Shutdown
Op::SetThreadName
```

Important correction:

`Op` is not "compact input." `Compact` is only one kind of Op.

The main role of Op is:

```text
external intent
  -> normalized internal command
  -> queued into core runtime
```

## Event

Events are the output of the runtime.

Useful minimal shape:

```ts
type AgentEvent =
  | { type: "turn_started"; threadId: string; turnId: string }
  | { type: "assistant_delta"; turnId: string; text: string }
  | { type: "tool_started"; turnId: string; callId: string; name: string; args: unknown }
  | { type: "tool_completed"; turnId: string; callId: string; result: unknown }
  | { type: "approval_requested"; turnId: string; requestId: string; action: unknown }
  | { type: "error"; turnId?: string; message: string }
  | { type: "turn_completed"; turnId: string; usage?: Usage }
```

Design lesson:

```text
request/response = control plane
event stream      = user experience and observability plane
```

## Tool Runtime

Tools should not be directly executed just because the model requested them.

Recommended tool path:

```text
model tool call
  -> tool registry lookup
  -> argument schema validation
  -> permission check
  -> approval flow if needed
  -> sandbox/environment selection
  -> execute
  -> normalize result
  -> emit event
  -> send result back to model
```

app-server usually only receives and translates permission/tool configuration.
The actual enforcement belongs in core/tool runtime.
