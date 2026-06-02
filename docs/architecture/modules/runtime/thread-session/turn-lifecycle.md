# Thread Session Turn Lifecycle

## Responsibility

`src/runtime/thread_session/turn.rs` executes one user turn inside a loaded thread.

## Key Flow

1. Set status to `Running`.
2. Clone current snapshot from live state.
3. Compact before turn if token budget requires it.
4. Apply runtime/environment context updates.
5. Record the user message in `ContextManager`.
6. Append turn context and conversation items to rollout.
7. Record `TurnStarted`.
8. Run the LLM/tool loop.
9. Record `TurnCompleted`, `TurnInterrupted`, or `RuntimeError`.
10. Set status back to `Idle`.

## LLM/Tool Loop

1. Build prompt from `ContextManager`.
2. Ask `Agent` for `LlmCompletion`.
3. Record assistant message and `AssistantTurn` event.
4. Record token count if usage exists.
5. If tool calls exist, execute each tool call.
6. Record tool messages and `ToolResult` events.
7. Repeat until no tool calls or `max_turns` is reached.

## State Writes

- `ContextManager.items`
- `SessionSnapshot.conversation`
- `rollout.jsonl`
- `ThreadSession.live_state`
- broadcast runtime events

## Follow-Up Reading

Return to this file for details on:

- compaction summary generation and replacement history.
- token count estimation and context-window fallback behavior.
- approval-specific tool effects and pending approval cleanup.
- turn lifecycle tests that cover interruption, compaction, and tool loops.



---

handle_user_input = 一个 turn 真正开始执行
record_user_turn_start = 写入上下文、用户消息、rollout，并发布 TurnStarted
run_session_turn = LLM/tool loop
ContextManager = LLM prompt 权威
SessionSnapshot = 当前 thread 状态镜像
rollout = durable source of truth
live_state = 对外 live projection
ThreadEventRecorder = rollout + live_state + event_tx 的统一输出管线
RuntimeOverlay = live-only side effects



-----
turn 开始执行，就是从 ThreadSession::handle_user_input 开始
ThreadRuntimeLoop 收到 ThreadOp::UserInput
  -> ThreadSession::handle_user_input(...)

从这里开始，runtime/session 进入真实执行阶段。

handle_user_input 做的第一件事就是：
self.set_status(ThreadRuntimeStatus::Running);
然后进入 handle_user_input_inner，开始 compaction、记录 user turn start、LLM/tool loop、事件记录。最后无论成功或失败，回到：
self.set_status(ThreadRuntimeStatus::Idle);

所以可以记：
ThreadManager.turn_start = 提交任务
ThreadRuntime.submit_user_input = 排队
ThreadRuntimeLoop = 取出任务
ThreadSession.handle_user_input = turn 真正开始执行

----


turn.rs 里 stateful 的东西主要有这些：

ThreadSession.live_state
ContextManager
SessionSnapshot
RolloutStore / rollout.jsonl
ThreadEventRecorder
RuntimeOverlay
Agent / ToolCallRuntime
PolicyManager
但它们作用不同：

ContextManager：内存中的 prompt history，会不断加入 user/assistant/tool message。
SessionSnapshot：当前 turn 操作时的 session 状态副本，更新后同步进 live_state。
RolloutStore：durable append log，记录 user/assistant/tool/context/compaction/events。
ThreadEventRecorder：统一记录 event，更新 live_state，广播 event_tx。
live_state：对外可读的当前状态镜像。
RuntimeOverlay：live-only 状态，比如 open exec sessions / pending approvals。
Agent / ToolCallRuntime：执行 LLM 和 tools 时携带 config、registry、exec_sessions、policy。
PolicyManager：pending approval 的内存状态。
一次 turn 的 state 变化大概是：

status: Idle -> Running

ContextManager:
  + context messages if needed
  + user message
  + assistant messages
  + tool messages

SessionSnapshot:
  sync conversation/context/compaction

rollout.jsonl:
  + TurnContext
  + ResponseItem(user/assistant/tool)
  + EventMsg(...)
  + Compacted if needed

live_state:
  snapshot updated
  events appended
  overlay updated if tool effects

event_tx:
  RuntimeEvent broadcast

status: Running -> Idle
所以 turn.rs 是目前最“有状态变化”的 runtime 文件。


-----
handle_user_input
  -> set status Running
  -> handle_user_input_inner
      -> compact_before_turn_if_needed
      -> record_user_turn_start    record_user_turn_start 是进入 LLM loop 前的准备和落盘：
      -> run_session_turn          LLM -> tool -> LLM -> tool ... -> final assistant
      -> record TurnCompleted / RuntimeError / TurnInterrupted
  -> set status Idle

------
1.handle_user_input turn总入口
ThreadSession::handle_user_input(turn_id, prompt, turn_context, interrupt)
流程
set_status(Running)
handle_user_input_inner(...)
set_status(Idle)
return result

------
handle_user_input_inner做真正的工作
1. 取 turn_cwd
2. 从 live_state 复制当前 snapshot
3. 如果需要，turn 前 compaction
4. 记录 user turn start
5. 运行 LLM/tool loop
6. 成功则记录 TurnCompleted
7. 出错则记录 RuntimeError
8. 中断则记录 TurnInterrupted



------
record_user_turn_start:记录用户输入和上下文
turn 开始前的durable 准备 3个地方 不可缺一
1. 计算本 turn 的 cwd
2. 生成 PromptContext
3. ContextManager 判断是否需要插入/更新 runtime context message
4. 把 user prompt 记进 ContextManager
5. sync 到 SessionSnapshot
6. 写 rollout:
   - RolloutItem::TurnContext
   - context messages
   - user message
7. 记录 TurnStarted event

为什么 user prompt 要写 ContextManager、SessionSnapshot、rollout

三者用途不同：

ContextManager:
  当前内存里给 LLM 的 prompt history

SessionSnapshot:
  live_state / thread_read / restore 时的当前会话状态镜像

rollout:
  durable history，进程重启后能恢复

如果只写 ContextManager，进程挂了就没了。
如果只写 rollout，当前 turn 内 LLM prompt 不方便直接拿。
如果只写 snapshot，没有 append-only history，也不适合 replay/restore。

所以每次用户输入、assistant、tool result 都会同步：

ContextManager
  -> sync_snapshot
  -> append rollout




3个信息源的作用
可以这样理解：

ContextManager = 当前 turn 内给 LLM 用的工作内存
SessionSnapshot = 当前 thread 状态的结构化镜像
rollout = 可持久恢复的事实日志
live_state = 对外读的内存发布面
你说“三个信息源”，如果主要指 ContextManager / SessionSnapshot / rollout，它们分别解决不同问题。

1. ContextManager：为 LLM 服务

它的目标是：

快速得到下一次 LLM prompt
管理模型可见上下文
插入 context update message
维护 token usage / compaction 后的 history
LLM 每轮直接看它：

context_manager.for_prompt()
所以它偏执行时、偏模型上下文。

2. SessionSnapshot：为状态展示和恢复形态服务

它的目标是：

描述当前 thread 的结构化状态
里面有：

session_id
workspace_root
cwd
conversation
latest_compaction
pending_approvals
open_exec_sessions
reference_turn_context
它比 ContextManager 更像“当前状态对象”，方便：

live_state 发布
thread_read
replay snapshot view
从 rollout 恢复后的状态落点
3. rollout：为 durable source of truth 服务

它是 append-only 历史日志：

SessionMeta
TurnContext
ResponseItem
Compacted
EventMsg
它解决的是：

进程重启后怎么恢复？
历史怎么 replay？
事件顺序怎么保留？
compaction 怎么解释？
它不是为了每次 LLM 调用快速读取，而是为了长期可靠恢复。

为什么不只用一个

如果只用 ContextManager：

进程重启就丢，外部也不好读结构化状态
如果只用 SessionSnapshot：

缺少 append-only 历史，事件 replay / compaction / audit 不好做
如果只用 rollout：

每次 LLM 调用都要从日志重建 prompt，太重，也不适合运行时频繁变更
所以这个设计是：

rollout 是事实来源
restore 时生成 ContextManager + SessionSnapshot
运行时 ContextManager 驱动 LLM
SessionSnapshot 同步成可展示状态
event recorder 发布 live_state
是不是巧妙？算是一个常见但不错的架构：event log + runtime projection + model context manager。

但它也有成本：

要保证 ContextManager、SessionSnapshot、rollout 三者同步
出错时可能出现状态不一致
代码里需要很多 sync_snapshot / append rollout / record event
所以这类设计的关键不是“少存”，而是明确谁是权威：

durable 权威：rollout
LLM prompt 权威：ContextManager
对外 live view 权威：live_state / SessionSnapshot projection
这个边界清楚，就不乱。




------
run_session_turn LLM/tool主循环

for _ in 0..agent.max_turns() {
    prompt = context_manager.for_prompt()
    completion = agent.sample_assistant_turn(prompt, tool_schemas)
    record_assistant_turn(...)
    record token usage if present

    if no tool calls:
        return assistant turn

    for each tool call:
        outcome = tool_runtime.execute(call)
        record_tool_outcome(...)
}

问 LLM
记录 assistant
如果 assistant 要 call tool，就执行 tool
记录 tool result
再把 tool result 放回上下文
继续问 LLM
直到 assistant 不再 call tool



run_session_turn 里每一轮大概是：
1. prompt = context_manager.for_prompt()
2. LLM completion
3. record_assistant_turn
   -> ContextManager + assistant message
   -> sync SessionSnapshot
   -> rollout ResponseItem
   -> RuntimeEvent::AssistantTurn
   -> live_state snapshot/events 更新
4. 如果有 token_usage
   -> ContextManager token info
   -> RuntimeEvent::TokenCount
   -> live_state events 更新
5. 如果有 tool calls
   -> execute tool
   -> record_tool_outcome
      -> apply ToolEffect to overlay if any
      -> ContextManager + tool message
      -> sync SessionSnapshot
      -> rollout ResponseItem
      -> RuntimeEvent::ToolResult
      -> live_state snapshot/events/overlay 更新
6. 下一轮 loop 用新的 ContextManager prompt



所以每轮 loop 后，通常这些都会被更新：

ContextManager
SessionSnapshot
rollout
live_state
event_tx
但它们的角色不同。

LLM 每一轮的上下文主要以 ContextManager 为准，不是直接以 SessionSnapshot 为准。

代码里是：

let prompt = context_manager.for_prompt();
agent.sample_assistant_turn(&prompt, &tool_runtime.schemas()).await
所以 LLM 看到的是：

ContextManager.items
SessionSnapshot 是被 ContextManager.sync_snapshot(snapshot) 同步出来的状态镜像，主要用于：

live_state.snapshot
rollout restore 的一致性
thread_read / replay snapshot view
后续 cold restore
可以这么记：

ContextManager = LLM prompt 的权威来源
SessionSnapshot = 当前 thread 状态的可恢复/可展示镜像
rollout = durable source of truth
live_state = 已发布状态的内存镜像
SessionSnapshot 会跟着 ContextManager 同步，但 LLM 不直接从 snapshot 取 prompt。


------
Compact类型
有两种 compaction：
turn 前检查 token budget：compact_before_turn_if_needed
LLM 返回 context window error 后：compact_after_context_window_error

调用 compact_history  操作
替换 ContextManager history 更新
更新 snapshot.latest_compaction  记录
写 RolloutItem::Compacted  记录
记录 CompactionWritten event 广播事件
记录 TokenCount event 更新


关于 context window error：对，就是已经真的触碰模型上下文窗口了，LLM API 返回类似“上下文太长”的错误。
这确实是“晚了一步”，但不是不可恢复，因为这次 LLM 请求失败时还没有 assistant 输出，也没有 tool result。系统可以：
发现 context window error
  -> compact 当前 history
  -> 写 Compacted 记录
  -> 恢复本轮必要 context + 最后一条 user message
  -> 重新调用 LLM

所以这里是一个 fallback。前面的 compact_before_turn_if_needed 是主动预防；context window error 分支是兜底恢复。

compaction 记录的东西主要是：
ContextManager 替换后的 history
SessionSnapshot.latest_compaction
RolloutItem::Compacted
CompactionWritten event
TokenCount event

这样后续 cold restore 能知道历史被压缩过，而不是丢了上下文。





------
1. 前几步是在准备 turn 环境
大致是：
拿 turn_cwd
clone 当前 snapshot
必要时 compact
record_user_turn_start
run_session_turn
record_user_turn_start 不是单纯“标记开始”，它确实是在组装这次 LLM prompt 的背景上下文。 很关键 不仅仅是记录开始 也是在组装


----
turn cwd 为什么又算一次
这里有两层 cwd：
ThreadManager:
  把 raw turn_context.cwd 解析成 PathBuf
  -> ThreadTurnContext.cwd

ThreadSession/turn.rs:
  决定本 turn 实际用哪个 cwd
  -> turn_cwd.unwrap_or(snapshot.cwd)
这不算重复，职责不同：

app_server/override_policy：校验和解析 raw cwd
turn.rs：如果这次 turn 没传 cwd，就 fallback 到 snapshot.cwd，并用于 PromptContext / ToolRuntime
所以 turn.rs 这里是在做“最终选择”，不是重新校验 raw input。


------
ContextManager 怎么判断要不要更新 context message

不只是 cwd。PromptContext::for_turn 生成的 TurnContextItem 包括：
workspace_root
cwd
model
policy_mode
command_timeout_secs
max_output_bytes
current_utc_date
ContextManager.apply_context_updates 会比较：

previous reference_turn_context
vs
current turn_context
如果没有 previous，就插入初始 context message：

Runtime context
Environment context
如果有 previous，就只插入变化项，比如：

Model changed
Policy mode changed
Command timeout changed
Max command output changed
Workspace root changed
Current working directory changed
Current UTC date changed
所以不是只有 cwd，也包括模型、policy、timeout、max output、workspace root、日期。



------
 completion 是在做什么

这里：

agent.sample_assistant_turn(&prompt, &tool_runtime.schemas()).await
实际就是调用 LLM：

messages = context_manager.for_prompt()
tools = 当前 ToolRegistry 的 tool schemas
LLM complete(messages, tools)
返回的是 LlmCompletion，里面有：

AssistantTurn {
  text,
  tool_calls
}

token_usage
然后：

记录 assistant message
如果有 token usage，更新 token 统计
如果没有 tool_calls，说明这是最终回答，return
如果有 tool_calls，就执行 tools，把 tool result 写回 context，再继续问 LLM




-------
ToolEffect 不是所有 tool 使用都进 overlay。

所有 tool 调用都会有：

ToolResult
  -> 作为 tool message 写进 ContextManager
  -> 写 rollout ResponseItem
  -> 发 ToolResult event
但只有某些 tool result 产生的 live side effect 会进 overlay：

ExecSessionUpdate
  -> overlay.open_exec_sessions

ApprovalUpdate
  -> overlay.pending_approvals
原因不是“tool 都是 live/stateless”，而是这些 effect 对应的是 live-only 状态：

persistent command 里的子进程句柄只能活在当前进程内存里
pending approval 对应 PolicyManager 里的等待项，也不能靠 rollout 冷恢复
所以：

ToolResult = durable conversation/history
ToolEffect = 从 ToolResult 里提取出的副作用
Overlay = 当前 loaded runtime 才真实存在的 live side effects
普通 read/write file 这类工具不一定会更新 overlay；它们主要就是返回 ToolResult。
