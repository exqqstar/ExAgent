# Runtime Module

## Responsibility

`runtime` executes loaded thread work.

It owns:

- per-thread actor queue
- active turn tracking
- thread session state
- agent sampling
- tool execution wrapper
- live approvals and exec session refs
- persistent subprocess handles
- command policy
- context compaction

## State

- `ThreadRuntime.active_turn`
- `ThreadSession.live_state`
- `ContextManager.items`
- `RuntimeOverlay.open_exec_sessions`
- `RuntimeOverlay.pending_approvals`
- `ExecSessionManager.sessions`
- `PolicyManager.pending`

## File Map

- [agent.md](agent.md)
- [thread-runtime.md](thread-runtime.md)
- [thread-session/](thread-session/README.md)
- [context.md](context.md)
- [tool-call-runtime.md](tool-call-runtime.md)
- [exec-session.md](exec-session.md)
- [policy.md](policy.md)
- [compaction.md](compaction.md)

## Key Flows

- `ThreadRuntime` receives a queued operation.
- `ThreadSession` mutates context and records lifecycle events.
- `Agent` samples the LLM.
- `ToolCallRuntime` executes tool calls.
- `ThreadEventRecorder` writes rollout, updates live state, and broadcasts.
