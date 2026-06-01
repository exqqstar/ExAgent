# Runtime Overlay

## Responsibility

`src/runtime/thread_session/overlay.rs` stores live-only state for loaded threads.

It keeps state that is useful for clients while the runtime is loaded but should not be reconstructed from cold rollout replay.

## State

- `open_exec_sessions`
- `pending_approvals`

## Key Rules

- Running exec refs are replaced by exec session id.
- Non-running exec sessions are removed from the overlay.
- Approval requests replace older entries with the same approval id.
- Interrupting a waiting approval clears pending approvals.

## Why Overlay Exists

It separates durable history from live process state. Rollout can replay what happened, but it should not pretend to resurrect subprocess handles or in-memory approval waiters.



1. open_exec_sessions

这个记录当前还活着的 persistent command，比如：

npm run dev
它里面会保存：

exec_session_id
command
cwd
status = Running
为什么是 live-only？

因为真正的 subprocess handle、stdin/stdout、child process 都只存在内存里。rollout 可以记录“曾经启动过一个 exec session”，但 cold restore 后不能假装那个进程还活着。

所以：

running exec session -> overlay
historical event -> rollout
2. pending_approvals

这个记录当前等待用户审批的命令，比如 risky command 需要确认。

它保存：

approval_id
requested_event_id
tool_name
reason
status = Pending
为什么是 live-only？

因为 pending approval 不是单纯历史，它对应内存里的等待状态和 PolicyManager 里的 pending command。cold restore 只能知道“曾经请求过 approval”，不能安全地恢复一个仍在等待执行的审批 waiter。

3. apply_exec_session_update

逻辑：

先按 exec_session_id 移除旧记录
如果新状态是 Running：
  加入 open_exec_sessions
如果 NotRunning：
  只移除，不再加入
所以 overlay 只保留当前还 running 的 exec sessions。

4. apply_approval_requested / clear_approval

approval requested：

先清掉同 approval_id 的旧记录
再 push 一个 PendingApproval
approval approved/denied 或 interrupt 时：

clear_approval
clear_pending_approvals
5. 为什么它被 events.rs 调用

因为 overlay 更新通常来自 tool execution effects：

ToolResult meta
  -> ToolEffect::ExecSessionUpdate
  -> overlay.open_exec_sessions

ToolResult meta
  -> ToolEffect::ApprovalUpdate
  -> overlay.pending_approvals
这些 effect 是 turn 执行过程中产生的，而且需要和 event recording 一起同步到 live_state，所以由 ThreadEventRecorder 那条 live publication 管线调用 overlay。

一句话：

overlay.rs = 当前 loaded runtime 的临时事实；
rollout = 可持久回放的历史事实。