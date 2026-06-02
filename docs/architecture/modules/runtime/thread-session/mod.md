Responsibility
ThreadSession是loaded thread的状态机
负责 从Rollout.jsonl 恢复thread状态  持有当前thread的live state
执行用户turn 管理模型可见context
记录runtime events 处理pending approval interrupt
把事件同步到rollout live_state event broadcast

不负责 http/cli 输入解析
app_server protocol view构建
thread runtime actore queue
LLM provider细节
tool 具体实现




mod.rs 负责 ThreadSession 的“装配和恢复”。

它主要做：

定义 ThreadSession 这个结构体
定义 ThreadSessionOptions
定义 ThreadSessionLiveState / LiveView
从 rollout 恢复 session 状态
创建 Agent / RolloutStore / EventRecorder / ContextManager
提供 runtime 需要的状态访问 helper
处理 waiting approval interrupt


Outputs

mod.rs 本身主要输出这些对象/能力：
ThreadSession
ThreadSessionLiveState handle
ThreadSessionLiveView
ThreadSessionStoppedGuard
next_turn_id
status update
waiting approval interrupt result

也就是说它提供给 ThreadRuntime 用的接口：
ThreadSession::new(...)
session.live_state_handle()
session.stopped_guard()
session.set_status(...)
ThreadSession::live_view_from_state(...)
ThreadSession::next_turn_id_from_state(...)
session.handle_interrupt(...)


State
mod.rs 定义并初始化 ThreadSession 的核心状态：
thread_id
agent
recorder
rollout_store
context_manager
status_tx
live_state
policy

还定义了 ThreadSessionLiveState：
snapshot
overlay
events
status

它会在 ThreadSession::new 里从 rollout 恢复：
SessionSnapshot
RuntimeEvent[]
ContextManager
然后构建 live_state。

Extension Point
如果你要改这些东西，看 mod.rs：
session 启动/恢复流程
loaded thread 初始化时要创建哪些组件
live_state 里暴露哪些 live 信息
runtime status 怎么发布
waiting approval interrupt 怎么清理
cold restore 怎么从 rollout 重建 snapshot/events/context

如果你要改这些，不主要看 mod.rs：
LLM loop：看 turn.rs
事件写入管线：看 events.rs
live-only overlay 状态：看 overlay.rs
context 文本如何注入模型：看 context.rs











ThreadSessionOptions
thread_id
config
agent_factory
event_tx
status_tx
policy
live_event_buffer_cap

它由 ThreadRuntime::spawn 创建并传入。默认会自己建 event_tx/status_tx/policy，但 runtime spawn 时会覆盖成 runtime 自己创建的通道和共享 policy。

作用是：

告诉 ThreadSession：
你是哪条 thread
你用什么 config
你怎么创建 Agent
你的事件往哪个 event_tx 发
你的状态往哪个 status_tx 发
你的 policy manager 是哪个
live events buffer 最多留多少条




ThreadSession存什么
thread_id
agent
recorder
rollout_store
context_manager
status_tx
live_state
policy

agent：真正调用 LLM、创建 tool runtime 的对象。
recorder：事件记录器，负责写 rollout、更新 live_state、broadcast event。
rollout_store：当前 thread 的 .exagent/threads/<thread_id>/rollout.jsonl。
context_manager：维护模型可见的 conversation/context。
status_tx：发布 Idle/Running/Stopped。
live_state：当前 loaded thread 的 live snapshot/overlay/events/status。
policy：处理 pending approval / interrupt 清理。


这里确实创建/持有了很多组件，但它们来自不同文件：

Agent：来自 runtime/agent.rs
ThreadEventRecorder：来自 thread_session/events.rs
RolloutStore：来自 state/rollout.rs
ContextManager：来自 runtime/context.rs
PolicyManager：来自 runtime/policy.rs
ThreadSessionLiveState：在本文件里定义
所以 mod.rs 是装配点，把这些能力组装成一个可执行的 session。





ThreadSessionLiveState
ThreadSessionLiveState {
    snapshot: SessionSnapshot
    overlay: RuntimeOverlay
    events: Vec<RuntimeEvent>
    status: ThreadRuntimeStatus
}
snapshot：从 rollout 恢复并随 turn 更新的 session 状态。
overlay：live-only 状态，比如 open exec sessions、pending approvals。
events：内存中的 bounded live event buffer。
status：runtime 状态。
这就是 ThreadRuntime.live_view() 读出来给 ThreadManager.thread_read/events_replay 用的东西。
在session这里做 因为session 第一手了解相关的信息



ThreadSession::new 做什么
这是最关键的构造流程：
1. 根据 config.workspace_root + thread_id 找 rollout path
2. 创建 RolloutStore
3. 读取 rollout.jsonl
4. restore_from_rollout:
   - 重建 SessionSnapshot
   - 重建 ContextManager
   - 提取 RuntimeEvent[]
5. 计算下一个 event id index
6. 裁剪 live events buffer，只保留最近 N 条
7. 用 agent_factory 创建 Agent
8. 初始化 live_state
9. 创建 ThreadEventRecorder
10. 返回 ThreadSession

所以 ThreadSession::new 既支持新 thread，也支持 resume/cold load。新 thread 也会有 rollout，因为 ThreadManager.thread_start 已经先写了 SessionMeta。


流程中4 restore_from_rollout
它不是执行历史 turn，而是从 rollout 的记录里恢复状态：
RolloutItem[]
  -> SessionSnapshot
  -> ContextManager
  -> RuntimeEvent[]

let mut snapshot = snapshot_from_rollout_items(...)
let context_manager = ContextManager::from_rollout_items(...)
context_manager.sync_snapshot(&mut snapshot)
let events = events_from_rollout_items(...)
也就是：
snapshot_from_rollout_items：从 SessionMeta、ResponseItem、TurnContext、Compacted 重建 SessionSnapshot
ContextManager::from_rollout_items：重建模型可见 conversation/context/token info
events_from_rollout_items：提取历史 RuntimeEvent

所以 cold load 的时候，runtime 不会“重跑 LLM/工具”，只是恢复已经记录下来的状态。

handle_interrupt
这里处理的是 pending approval 状态下的 interrupt。它会：
检查 overlay 是否有 pending approval
确定要 interrupt 的 turn id
清空 overlay pending approvals
取消 PolicyManager 里的 pending approval
记录 TurnInterrupted event




----
Agent/RolloutStore/EventRecorder/ContextManager 是怎么创建的

都在 ThreadSession::new 里：

RolloutStore:
  RolloutStore::new(rollout_paths.rollout_path)

ContextManager:
  ContextManager::from_rollout_items(items)

Agent:
  agent_factory(config.clone())

live_state:
  Arc<RwLock<ThreadSessionLiveState { ... }>>

ThreadEventRecorder:
  ThreadEventRecorder::new(thread_id, rollout_store, event_tx, live_state, next_event_index, cap)
这里的关系是：

ThreadManager 提供 agent_factory
ThreadRuntime 提供 event_tx/status_tx
rollout.rs 提供 RolloutStore 和恢复 helpers
context.rs 提供 ContextManager
events.rs 提供 ThreadEventRecorder
mod.rs 把它们装起来



------
这些接口是给谁用的

两类。

给 thread_runtime.rs 用的：

ThreadSession::new
live_state_handle
stopped_guard
live_view_from_state
next_turn_id_from_state
handle_interrupt
handle_user_input
其中 handle_user_input 实现在 turn.rs，但也是 ThreadRuntimeLoop 调用。

给 thread_session 文件夹内部用的：

append_and_broadcast_snapshot
set_status
recorder
rollout_store
context_manager
agent
policy
events.rs 和 turn.rs 都是在扩展 impl ThreadSession，所以它们能使用 mod.rs 定义的字段和内部 helper。

一句话：

mod.rs 定义 ThreadSession 的骨架和恢复流程；
thread_runtime.rs 通过它提交/中断/读取 session；
turn.rs 和 events.rs 通过它的字段真正执行 turn 和记录事件。



------
app_server/thread_manager.rs
  ensure_runtime_loaded
    -> ThreadRuntime::spawn(...)
然后在 ThreadRuntime::spawn 里面：

let session = ThreadSession::new(
    ThreadSessionOptions::new(options.thread_id, options.config, options.agent_factory)
        .with_event_tx(event_tx.clone())
        .with_status_tx(status_tx)
        .with_policy(options.policy),
)?;
所以：

ThreadManager 触发 ensure
ThreadRuntime::spawn 创建 ThreadRuntime + ThreadSession
ThreadSession 被放进后台 runtime loop 里
ThreadRuntime 作为 facade 返回给 ThreadManager
所以你原先以为 “spawn 只创建 ThreadRuntime” 不完全准确。它返回的是 ThreadRuntime，但内部也创建了 ThreadSession，并把 session 移进 runtime loop 里持有。ThreadManager 后面拿不到 ThreadSession，只能通过 ThreadRuntime 间接操作它。
