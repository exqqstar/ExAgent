# Policy Manager

## Responsibility

`src/runtime/policy.rs` classifies shell commands and stores pending command approvals.

## State

- `pending`: in-memory approval id to pending command approval.

## Modes

- `off`: allow commands unless hard-denied.
- `advisory`: currently behaves like allow except hard-deny.
- `enforced`: risky commands require review.

## Key Rules

- Hard-deny patterns are rejected without approval.
- Review-required patterns produce a pending approval.
- Pending approvals are live-only waiters and can be cancelled for a session.

## Extension Points

- Make `advisory` mode meaningful by allowing execution while emitting warnings/events for risky commands.
- Replace string `contains` checks with structured, configurable command rules that can consider command, args, cwd, workspace root, tool name, persistence, and environment.
- Add approval scopes and lifetimes such as once, this turn, this thread, this workspace, or TTL-based approval.
- Move policy rules into config layers such as `AgentConfig`, workspace policy files, user settings, or organization defaults.
- Keep policy as the decision layer and pair it with stronger sandbox/workspace enforcement at execution time.
- Define pending approval lifecycle for timeout, cold restore, cancellation, and audit history.



----
app_server/override_policy.rs
  请求边界策略：workspace_root / cwd 怎么合并进 AgentConfig

runtime/policy.rs
  工具执行策略：run_command 这种危险操作 allow / deny / approval

Responsibility
它负责两件事：
分类命令风险：
classify_command(mode, command) -> Allow / Deny / ReviewRequired

保存 pending approval：
approval_id -> PendingCommandApproval

----
PolicyMode
有三个模式：
Off: 默认允许，但 hard-deny 还是拒绝。
Advisory: 目前和 off 差不多，也是除了 hard-deny 都放行。
Enforced: risky command 需要 approval。
现在规则非常简单：
hard deny:
rm -rf /
mkfs

enforced 下需要 review:
rm -rf
git reset --hard
git checkout --
shutdown
reboot

这些在 hard_deny_reason / review_required_reason (line 161)。

----
State
它唯一长期持有的状态是：

pending: HashMap<approval_id, PendingCommandApproval>

PendingCommandApproval 里存：
approval_id
session_id，这里语义上还是 thread id
tool_name
command
cwd
timeout_secs
persistent
reason
这也是 live-only，不是 rollout 恢复出来的 waiter。
----


run_command 怎么用它

在 run_command.rs (line 257)：

run_command
 -> maybe_require_approval
 -> ctx.policy.classify_command

如果是：
Allow: 继续执行命令。
Deny: 返回 ToolResult error。
ReviewRequired: 创建 pending approval，返回 ToolStatus::ReviewRequired，并带上 approval_id。
然后 ToolCallRuntime 看到 approval_id + approval_status = pending，会转成 ApprovalUpdate::Requested，最后进 overlay.pending_approvals 和 ApprovalRequested event。
interrupt 关系
如果用户在等待 approval 的时候 interrupt，ThreadSession.handle_interrupt 会：
清掉 overlay 里的 pending approvals。
调 policy.cancel_pending_for_session(&self.thread_id)。
记录 TurnInterrupted。
