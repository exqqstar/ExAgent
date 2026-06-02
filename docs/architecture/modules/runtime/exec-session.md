# Exec Session Manager

## Responsibility

`src/runtime/exec_session.rs` manages persistent subprocess sessions.

It supports:

- starting a persistent command
- writing stdin
- polling output/status
- terminating a session

## State

- `sessions`: nested map from session id to exec session id to active subprocess handle.
- Each active session keeps command, cwd, child handle, stdin, stdout/stderr buffers, status, and exit code.

## Durability Rule

Subprocess handles are live-only. Cold rollout replay does not recreate running processes.

The current Rust parameter/type name uses `session_id`, but in the app-server/runtime architecture this value is semantically the thread id. This matches the broader `SessionId` versus `thread_id` naming debt and should be cleaned up with the protocol id rename.

Stdout/stderr buffers are also live-only today. They are returned through `run_command` tool results when a persistent command is started, polled, written to, or terminated, but output chunks are not currently durably persisted as a separate command log stream.

Future cleanup should define two separate policies:

- command log durability: persist stdout/stderr chunks or snapshots so command history survives process restart;
- model context shape: avoid repeatedly appending full accumulated stdout/stderr to LLM context, and prefer deltas, tail windows, or summaries for repeated polls.

Persistent process handles should still remain live-only. After a cold restart, the runtime should not pretend that an old subprocess is still controllable. What should survive is the command log/history, not the process handle itself.

For repeated polling, add a cursor or byte offset so `poll` can return output since the last observed position. The LLM-facing `ToolResult` can then include incremental stdout/stderr plus status, while UI/debug surfaces can still read the durable full log when needed.

## Connections

- Called by `tools/run_command.rs`.
- Open exec session refs are mirrored in `RuntimeOverlay`.



所以当前是：

ExecSessionState.stdout/stderr
  持续 append，live-only

run_command poll/start/terminate
  返回完整 stdout/stderr snapshot

ToolResult
  写进 ContextManager.items
  写进 rollout


delta 的意思是：不要每次 poll 都把完整 stdout 从头返回，而是只返回“上次 poll 之后新增的输出”。比如：

poll 1 -> new stdout: line 1-10
poll 2 -> new stdout: line 11-20
poll 3 -> new stdout: line 21-25
这样 LLM context 不会反复塞入 line 1-10、line 1-20、line 1-25 这种重复内容。

关键设计应该拆成两层：

Durable command log
完整 stdout/stderr 要保存，给审计、debug、UI 回放用。可以是 rollout event，也可以是单独的 exec log 文件。

LLM context
不一定要长期拥有完整日志。它主要需要“最近新增了什么、当前状态是什么、有没有错误、最后几行是什么”。

比较合理的策略是：

完整日志：持久保存到 command log
LLM ToolResult：返回 delta + tail + status + log reference
必要时：对长日志做摘要
摘要不是每次都必须。小输出直接进 context 最简单；长输出才需要摘要或 tail window。否则每次 tool 调用都先摘要，会慢，也会引入模型总结错误。

所以后续比较好的方向是：

persistent command output
 -> durable log stream 保存完整内容
 -> ToolResult 只给 LLM 新增输出 / 最后 N 行 / 状态
 -> compaction 或超长输出时再生成摘要
这样既不丢日志，也不把 LLM context 搞爆。



所以更准确的目标是：

进程 handle：live-only，冷启动不恢复
命令日志：durable，可以回放/debug
poll 输出：cursor/offset 增量返回
LLM context：只吃增量输出 + 状态，必要时 tail/summary