# Tool Call Runtime

## Responsibility

`src/runtime/tool_call_runtime.rs` is the bridge between model tool calls and runtime side effects.

It executes tool calls through `ToolRegistry` and extracts effects from `ToolResult.meta`.

## State

No long-lived state. Each `ToolCallRuntime` is scoped to a session and turn.

## Effects

- `ExecSessionUpdate`: update live open exec session refs.
- `ApprovalUpdate`: add, approve, or deny pending approvals.

## Key Rule

Tool implementations return metadata; runtime interprets selected metadata as state changes. This keeps tools simple while preserving runtime ownership of live state.

职责 三行话
execute tool call through ToolRegistry
extract runtime effects from ToolResult.meta
return ToolExecutionOutcome { result, effects }

---
状态
ToolCallRuntime 是 turn-scoped 的，不是长期状态管理器。它持有这次 turn 执行 tool 需要的上下文：

config
registry
exec_sessions
policy
session_id
turn_id
cwd
这些字段让 tool 执行时知道：

当前 workspace/cwd 是什么
当前 session/turn 是什么
有哪些 tools 可用
persistent exec sessions 存哪里
command policy/approval 走哪个 manager

execute 做什么

let ctx = ToolContext { ... };
let result = self.registry.execute(call, Some(&ctx)).await;
let effects = tool_effects_from_result(&result, self.cwd.clone());
ToolExecutionOutcome { result, effects }
也就是说：

ToolCallRuntime
  -> 构造 ToolContext
  -> 调 ToolRegistry 执行具体 tool
  -> 拿 ToolResult
  -> 从 ToolResult.meta 解析 live side effects
ToolRegistry 做什么

ToolRegistry 是按 tool name 找具体 tool：

ToolCall.name
  -> registry.tools[name]
  -> tool.execute(call, ctx)
如果找不到 tool，就返回 Unknown tool 的 error result。

Effect 有哪两类

目前只有两类：

ToolEffect::ExecSessionUpdate
ToolEffect::ApprovalUpdate
ExecSessionUpdate：

Running { exec_session_id, command, cwd }
NotRunning { exec_session_id }
用于更新：

overlay.open_exec_sessions
ApprovalUpdate：

Requested { approval_id, tool_name, reason }
Approved { approval_id }
Denied { approval_id }
用于更新：

overlay.pending_approvals
并记录 approval 相关 events。

Effect 从哪里来

不是 tool 直接改 overlay。tool 只是返回 ToolResult.meta，比如：

exec_session_id
lifecycle = running / exited / terminated
command
cwd
或者：

approval_id
approval_status = pending / approved / denied
approval_reason
ToolCallRuntime 从这些 meta 里解析出 effect。

这样设计的好处是：

tools 负责执行和返回结果
runtime 负责解释哪些结果会改变 live state
所以 overlay 仍然由 runtime/session 管理，不让 tool 自己直接改 runtime state。

一句话

ToolCallRuntime 是 tool 调用和 runtime live state 之间的桥：
执行 tool，保留 ToolResult 作为对话历史，再把 meta 翻译成 ExecSession/Approval 这类 live effect。





-------
是异步等待，但不是开新 turn。

在 run_session_turn 里，一个用户 turn 内部可能有多轮 LLM/tool loop：

LLM call
  -> assistant 返回 tool_calls
  -> 执行 tool calls
  -> 把 tool results 写回 ContextManager
  -> 再次 LLM call
  -> 直到 assistant 不再 tool_calls
这些都属于同一个：

turn_id
不是新的 turn。

执行方式是顺序 async await：

let completion = agent.sample_assistant_turn(...).await;
...
let outcome = tool_runtime.execute(call).await;
...
下一轮 loop
所以：

LLM 请求会 await
tool 执行也会 await
tool result 记录好后，才进入下一轮 LLM 请求
会不会每一轮都“加载”很慢？不会每轮重新加载 runtime/session/agent。它们都是已经在内存里的：

ThreadRuntime 已 loaded
ThreadSession 已存在
Agent 已存在
ToolRegistry 已存在
ContextManager 已存在
每一轮主要成本是：

1. clone/build prompt messages
2. 发送一次 LLM API 请求
3. 如果有 tool call，执行 tool
4. 写 rollout / 更新 live_state / broadcast event
真正慢的一般是：

LLM API 请求
tool 本身，比如 shell command / 文件 IO / 网络
不是 runtime 重新加载。

但是确实每轮 LLM 都会重新发送完整当前 prompt 给模型，这是 chat completion 的正常模式：

context_manager.for_prompt()
  -> messages
  -> LLM complete(messages, tools)
所以如果历史很长，会变慢、变贵、触发 context window。这就是为什么有：

token tracking
auto compaction
context-window error fallback
一句话：

一个 user turn 内，LLM/tool 是异步顺序循环；
每轮不会重建 runtime，只是用更新后的 ContextManager 再请求一次 LLM。