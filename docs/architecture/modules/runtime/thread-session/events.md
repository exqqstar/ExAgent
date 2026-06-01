# Thread Session Events

## Responsibility

`src/runtime/thread_session/events.rs` centralizes runtime event recording.

It ensures every recorded event can update all required outputs consistently.

## Outputs

For each recorded event:

1. Allocate or use a reserved event id.
2. Append persisted event item to rollout when allowed.
3. Update `ThreadSession.live_state.snapshot`.
4. Push the event into bounded live event buffer.
5. Broadcast the event to subscribers.

## Related State

- `ThreadEventRecorder.next_event_index`
- `ThreadSessionLiveState.events`
- `rollout.jsonl`
- broadcast channel subscribers

## Extension Point

When adding a new event type, update:

- `src/state/events.rs`
- event recording call site
- rollout persistence policy if durable
- app-server view mapping if client-visible




----
可以先记一句：
ThreadEventRecorder 负责把 RuntimeEvent 同时送到 rollout、live_state、event_tx。

1. RuntimeEvent 长什么样
事件类型定义不在 events.rs，而是在 state/events.rs (line 13)：
rust
RuntimeEvent {
    event_id,
    session_id,
    turn_id,
    kind,
}

kind 包括：
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

2. LiveEventSink 是什么
LiveEventSink 是一个 trait，定义了“记录事件”的接口：
rust
reserve_event_id()
record_reserved(...)
record(...)

record(...) 默认逻辑是：
先 reserve_event_id
再 record_reserved

为什么要有 reserve_event_id？因为某些场景要先拿到 event id，再把这个 event id 写进 overlay 里的 pending approval。比如 approval requested 需要：
requested_event_id

所以它要先预留 id，再记录事件。

3. ThreadEventRecorder 存什么
ThreadEventRecorder 里面有：
thread_id
rollout_store
next_event_index
event_tx
live_state
live_event_buffer_cap

它知道：
当前 thread 是谁
rollout 写到哪里
下一个 event id 是多少
live subscribers 往哪个 event_tx 推
live_state 怎么更新
live events buffer 最多保留多少条

4. record_snapshot 做什么
这是核心函数：
1. 构造 RuntimeEvent
2. append RolloutItem::EventMsg(event) 到 rollout
3. 拿 live_state 写锁
4. 更新 live_state.snapshot
5. 把 event push 到 live_state.events
6. 如果 live events 超出 cap，裁掉旧事件
7. drop live_state lock
8. event_tx.broadcast(event)
9. 返回 event

所以一个事件进入 recorder 后，会影响三处：
rollout.jsonl       durable replay
live_state.events   live replay/thread_read
event_tx            subscribe/SSE 实时推送

5. rollout 不是所有事件都持久化
这里有个细节：record_snapshot 总是 append RolloutItem::EventMsg(event)，但 RolloutStore.append_items_blocking 里面会过滤。
当前持久化的 event 是：
TurnStarted
TurnCompleted
TurnInterrupted
RuntimeError
ApprovalRequested
ApprovalDecision
TokenCount

不持久化的主要是：
AssistantTurn
ToolResult
ExecOutput
CompactionWritten
其中 assistant/tool/conversation 相关内容通常通过其他 RolloutItem::ResponseItem 等方式保存；不是所有 live event 都作为 EventMsg 落盘。

6. overlay 更新方法
events.rs 里还有三个方法：
rust
apply_exec_session_update(...)
apply_approval_requested(...)
clear_approval(...)

这三个不直接 broadcast event，它们更新的是：

live_state.overlay
也就是 live-only 状态：
open exec sessions
pending approvals
真正的 ApprovalRequested / ApprovalDecision 事件仍然会通过 record / record_reserved 记录。

7. append_and_broadcast_snapshot
这是给 ThreadSession 用的便捷方法：
ThreadSession::append_and_broadcast_snapshot(snapshot, turn_id, kind)

内部就是：

reserve event id
recorder.record_snapshot(...)

turn 生命周期里常用它记录：

TurnStarted
TurnCompleted
TurnInterrupted
RuntimeError

一句话总结：

events.rs 不决定业务什么时候发生；
它只保证一旦发生 RuntimeEvent，就用同一条管线写 durable history、更新 live mirror、推送 live subscribers。