# Thread Runtime

## Responsibility

`src/runtime/thread_runtime.rs` is the actor facade for one loaded thread.

It serializes operations for a thread and exposes live event subscription and status.

## State

- `op_tx`: queue for `ThreadOp`.
- `event_tx`: broadcast channel for live runtime events.
- `status_rx`: watch channel for runtime status.
- `active_turn`: tracks a currently running turn and interrupt handle.
- `live_state`: read handle to `ThreadSessionLiveState`.

## Key Flow

1. `ThreadRuntime::spawn` creates channels and a `ThreadSession`.
2. It spawns the runtime loop.
3. `submit_user_input` reserves `active_turn` and sends `ThreadOp::UserInput`.
4. The runtime loop calls `ThreadSession::handle_user_input`.
5. Completion is returned through an optional oneshot channel.

## Key Rule

Only one active turn is allowed per thread runtime.

含义：

op_tx：给 runtime loop 发操作的队列入口，比如 UserInput、Interrupt、Shutdown
event_tx：runtime event 的 broadcast channel，给 subscribe/SSE 用
status_rx：watch 当前 runtime 状态，Idle / Running / Stopped
active_turn：记录当前正在跑的 turn，以及 interrupt handle
live_state：指向 ThreadSessionLiveState 的读句柄，给 live_view() / next_turn_id() / replay/read 使用




ThreadRuntime::spawn 做什么
它是在 ThreadManager.ensure_runtime_loaded 里被调用的。spawn 做的是：
1. 创建 op channel
2. 创建 event broadcast channel
3. 创建 status watch channel
4. 创建 ThreadSession
5. 从 ThreadSession 拿 live_state handle
6. 构造 ThreadRuntime facade
7. 启动后台 runtime loop
8. 返回 Arc<ThreadRuntime>
所以 ThreadRuntime 不是执行体本身，它更像一个对外句柄。真正执行体在后台 loop 里的 ThreadSession。



runtime loop 是什么样
runtime loop 在 thread_runtime.rs (line 388)：

while let Some(submission) = self.op_rx.recv().await {
    match submission.op {
        ThreadOp::Shutdown => { ... break; }
        ThreadOp::UserInput { ... } => {
            self.session.handle_user_input(...).await
        }
        ThreadOp::Interrupt { ... } => {
            self.session.handle_interrupt(...).await
        }
    }
}
终止条件有两个：
    收到 ThreadOp::Shutdown，返回 Ack 后 break
    op_tx 全部 drop，op_rx.recv().await 返回 None，while 结束
loop 开始时创建了一个 stopped_guard。loop 结束时 guard drop，会把状态标成 Stopped。





submit_user_input 做什么

不是“找到 active turn”，而是 reserve active turn。

流程：

submit_user_input
  -> send_user_input
      -> 创建 interrupt channel
      -> reserve_active_turn
          如果已有 active_turn：返回 ThreadBusy
          否则写入 active_turn
      -> op_tx.send(ThreadSubmission {
           op: ThreadOp::UserInput { turn_id, prompt, turn_context },
           interrupt,
           active_turn_guard
         })
关键点：

active_turn 是并发保护：
同一个 thread runtime 同一时间只允许一个 turn 在跑。
ActiveRuntimeTurnGuard 被放进 submission 里。等这次 submission 处理完、guard drop，就会清空 active_turn。

runtime loop call ThreadSession::handle_user_input 后，它的任务完成了吗

对，ThreadRuntime 的核心任务到这里基本完成了：它负责排队、串行化、active turn 管理、interrupt channel、结果回传。

真正的 turn 执行交给：

ThreadSession::handle_user_input
也就是后面的 thread_session/turn.rs。

但 runtime loop 还负责等 handle_user_input 返回后：

complete(submission.completion_tx, result)
如果调用的是 submit_user_input_and_wait，外部会通过 oneshot 等到这个 result。
如果调用的是 submit_user_input，没有 completion channel，runtime 只是后台跑，外部通过 events 看结果。

一句话：

ThreadRuntime 负责让每个 thread 的操作排队、串行、可中断、可订阅；
ThreadSession 负责真正执行一个 turn。



Event_tx
event_tx 会广播什么类型
event_tx 广播的是 RuntimeEvent，里面的 kind 是 RuntimeEventKind (line 22)：

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

它就是 session/agent/tool 执行过程中产生的事件流。作用是让外部不是看一个黑盒，而是能看到：

turn 开始了
assistant 说了什么
工具执行了什么
命令输出了什么
是否需要 approval
有没有错误
turn 完成了


这些事件由 ThreadEventRecorder 负责：

写 rollout
更新 live_state
broadcast 到 event_tx

所以 event_tx 是 live subscribe / SSE 的核心。


status_rx
记录状态 idle running stopped
用途：

runtime.status() 可以快速知道 runtime 是否在跑
live_state.status 也会同步更新
wait_until_terminated() 可以等它变成 Stopped
thread_read/view 可以判断状态，虽然现在更多还是看 active_turn / overlay / events


Interrupt handle

interrupt handle 是做什么

active_turn 里保存了当前 turn 的：

turn_id
interrupt_tx
interrupted notify
当用户调用 turn_interrupt 时，如果当前有 active turn：

ThreadManager
  -> runtime.interrupt_active_turn(...)
  -> 通过 interrupt_tx 发信号
ThreadSession.handle_user_input 里面会用这个 signal 中断正在执行的 turn，比如 compaction/LLM/tool loop 中被打断。打断处理完成后，会 notify，interrupt_active_turn 等到这个 notify 再返回。




live_state 读什么

live_state 是 ThreadSessionLiveState 的共享读视图，里面有：

snapshot
overlay
events
status
含义：

snapshot：当前 thread 的会话状态，比如 conversation、cwd、workspace_root、compaction 等
overlay：live-only 状态，比如 open exec sessions、pending approvals
events：内存里的最近 runtime events buffer
status：Idle / Running / Stopped
ThreadRuntime 自己持有 Arc<RwLock<...>>，对外提供：

live_view()
next_turn_id()

给谁用：

ThreadRuntime.live_view()：给 ThreadManager.thread_read / events_replay
ThreadRuntime.next_turn_id()：根据当前 conversation 推算下一个 turn id
ThreadEventRecorder：每次事件产生时更新它
app_server view building：把它转成 ThreadView
也就是说，live_state 是 runtime/session 写，app_server 读。
