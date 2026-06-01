# Runtime Events

## Responsibility

`src/state/events.rs` defines event variants emitted by runtime.

## Important Events

- `TurnStarted`
- `TurnCompleted`
- `TurnInterrupted`
- `AssistantTurn`
- `ToolResult`
- `ExecOutput`
- `ApprovalRequested`
- `ApprovalDecision`
- `CompactionWritten`
- `TokenCount`
- `RuntimeError`

## Rule

Events describe things that happened. Whether an event is persisted is decided by rollout policy.



state/events.rs 是 runtime event 的类型定义。它描述“运行过程中发生了什么”，后面给 replay、subscribe、ThreadView、SSE 用。

核心类型在 src/state/events.rs (line 13)：

pub struct RuntimeEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub kind: RuntimeEventKind,
}
这里 session_id 语义上还是 thread id。turn_id 是可选的，因为有些事件可能不属于某一轮 turn。

RuntimeEventKind
事件种类在 line 24 (line 24)：

TurnStarted
TurnCompleted
TurnInterrupted
AssistantTurn
ToolResult
ExecOutput
ApprovalRequested
ApprovalDecision
CompactionWritten
TokenCount
RuntimeError
可以分成几组：

1. turn lifecycle

TurnStarted
TurnCompleted
TurnInterrupted
RuntimeError
这些用来判断 turn 当前状态。

2. LLM/tool 可见结果

AssistantTurn
ToolResult
这些用于展示模型回复和工具结果。内容本身通常也会以 ResponseItem 写入 rollout。

3. exec streaming

ExecOutput
类型已经有了，但当前 exec_session.rs 没有真正发这个 event。以后如果做实时命令输出流，这个会用上。

4. approval

ApprovalRequested
ApprovalDecision
用于 UI/API 展示审批请求和审批结果。

5. compaction/token

CompactionWritten
TokenCount
用于说明上下文被压缩、token 状态变化。

持久化不是这里决定
events.rs 只定义事件。哪些事件进入 rollout，是 rollout.rs (line 132) 决定的。

当前持久化：

TurnStarted
TurnCompleted
TurnInterrupted
RuntimeError
ApprovalRequested
ApprovalDecision
TokenCount
不作为 EventMsg 持久化：

AssistantTurn
ToolResult
ExecOutput
CompactionWritten
但 Assistant/Tool 通常通过 ResponseItem 另外持久化；Compaction 通过 Compacted 持久化。

和 ThreadView 的关系
app-server 会把 RuntimeEvent 转成 ThreadView/TurnView/ThreadItem。转换逻辑在 thread_manager.rs (line 800)。

例如：

AssistantTurn      -> ThreadItem::AssistantMessage
ToolResult         -> ThreadItem::ToolResult
ExecOutput         -> ThreadItem::ExecOutput
ApprovalRequested  -> ThreadItem::ApprovalRequested
ApprovalDecision   -> ThreadItem::ApprovalDecision
RuntimeError       -> ThreadItem::RuntimeError
CompactionWritten  -> ThreadItem::CompactionWritten
TurnStarted/TurnCompleted/TurnInterrupted/TokenCount 不直接变成 item，主要用来更新状态或内部信息。

一句话总结：

events.rs = runtime 发生了什么的公共事件语言
rollout.rs = 哪些事件需要持久化
thread_manager.rs = 怎么把事件投影成 app-server view
api.rs = 怎么把事件通过 SSE 推给客户端





对，就是这个意思。

RuntimeEvent 是 runtime 内部的事件语言，比较完整、偏底层：

TurnStarted
AssistantTurn
ToolResult
ApprovalRequested
TokenCount
RuntimeError
...
thread_manager 里的投影逻辑，就是把这些 runtime event 转成 app-server boundary 更好消费的 view：

RuntimeEvent[]
 -> ThreadView
 -> TurnView[]
 -> ThreadItem[]
这样 GUI/API/CLI 不需要理解 runtime 内部怎么跑，只需要看：

thread.status
thread.active_turn
turn.status
turn.items
比如：

RuntimeEventKind::AssistantTurn
 -> ThreadItem::AssistantMessage

RuntimeEventKind::ToolResult
 -> ThreadItem::ToolResult

RuntimeEventKind::ApprovalRequested
 -> ThreadItem::ApprovalRequested
同时 app-server 也还可以把原始 RuntimeEvent 用于：

events_replay
events_subscribe
API SSE 推送
CLI adapter 等待最终 assistant turn
所以有两条消费方式：

1. View projection
   RuntimeEvent -> ThreadView/TurnView/ThreadItem
   给 thread_read / GUI 当前状态用

2. Event stream
   RuntimeEvent 原样 replay/subscribe
   给 SSE / CLI / 实时监听用
你可以这样记：

RuntimeEvent = runtime 发生了什么
ThreadView = app/server/client 当前该怎么展示
thread_manager 就是这中间的翻译层。