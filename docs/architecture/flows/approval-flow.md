# Approval Flow

Approval is used when command policy requires review before running a risky command.

```mermaid
sequenceDiagram
    participant Tool as run_command
    participant Policy as PolicyManager
    participant Session as ThreadSession
    participant Overlay as RuntimeOverlay
    participant Events as ThreadEventRecorder
    participant Client

    Tool->>Policy: classify_command
    alt review required
        Policy-->>Tool: approval_id + reason
        Tool-->>Session: ToolResult(review_required)
        Session->>Overlay: pending approval
        Session->>Events: ApprovalRequested
        Client->>Tool: run_command approval decision
        Tool->>Policy: take_pending_command
        Tool-->>Session: approved/denied ToolResult
        Session->>Overlay: clear approval
        Session->>Events: ApprovalDecision
    else allowed
        Tool-->>Session: command result
    end
```

## Key State Changes

- `PolicyManager.pending` stores the in-memory command approval waiter.
- `RuntimeOverlay.pending_approvals` exposes pending approval state to live thread views.
- `ApprovalRequested` and `ApprovalDecision` are persisted events.
- Interrupting a waiting approval clears overlay approvals and policy-side waiters.

## Main Files

- `src/runtime/policy.rs`
- `src/tools/run_command.rs`
- `src/runtime/tool_call_runtime.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/thread_session/overlay.rs`
