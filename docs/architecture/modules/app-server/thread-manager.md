# Thread Manager

## Responsibility

`src/app_server/thread_manager.rs` is the app-server orchestration hub.

It is a large file because it currently groups several related responsibilities:

- thread lifecycle
- turn lifecycle
- runtime loading
- event replay and subscription
- thread view construction
- runtime error mapping

## State

- `base_config`: default runtime config.
- `loaded_threads`: loaded runtime actors keyed by thread id.
- `exec_sessions`: shared persistent exec session manager.
- `policy`: shared command policy manager.
- `llm_factory`: builds LLM clients for runtime agents.
- `registry_factory`: builds tool registries.

## Responsibility Blocks

### Thread Lifecycle

- `thread_start`
- `thread_resume`
- `thread_read`
- `start_thread_with_options`

### Turn Lifecycle

- `turn_start`
- `turn_start_direct`
- `start_turn_in_background`
- `run_turn_through_runtime`

### Runtime Loading

- `ensure_runtime_loaded`
- `runtime_for`
- `resolve_loaded_runtime`

### Events

- `events_replay`
- `events_subscribe`
- `filter_replay_events`

### View Building

- `build_thread_view`
- `build_turn_views`
- `thread_item_from_event`

## Key Rule

`ThreadManager` decides which runtime should handle work. It should not contain the LLM/tool loop itself.

## Extension Points

- New boundary operation: add DTOs in `protocol.rs`, service method, manager method, and route.
- New event view item: update `ThreadItem` and `thread_item_from_event`.
- New runtime load policy: update `ensure_runtime_loaded` and `resolve_loaded_runtime`.


6个struct ThreadManager持有 创建runtime/agent 把他们注入 执行逻辑 在runtime agent tool里发生
一 base_config
AgentConfig 来自config.rs 但是ThreadManager会持有一份
AgentConfig::default()
  -> ThreadManager.base_config
config.rs 定义AgentConfig和default生成规则
ThreadManager 保存base_config
override_policy基于 base_config生成request/thread config
runtime/agent/tools 消费config

二llm_factory
抽象借口 定义在model::llm.rs
runtime创建agent时 给agent一个可调用模型的client

三registry_factory
决定agent有哪些工具schema 能给模型看 模型能call哪些工具
每个agent拿到一个registry 实例 默认一样 用factory创建 未来可按场景注入不同工具集合

四exec session
rollout管durable history 事件 对话 snapshot tool result等可以回放的记录
ExecSessionManager 管理活的进程会话  persistent shell command/long-running process
工具启动了 npm run dev 进程活着 后续工具调用 可以 poll output write stdin terminate
真正的child process stdin handle在ExecSessionManager里面 （后续child spawn？）
rollout = 可持久化历史
exec_sessions = 当前内存中的活 subprocess 管理

五 policy
ThreadManager创建 并持有PolicyManager 然后传给runtime agent tools
真正用到的地方主要是 run_command tool runtime/session也会使用来取消 Pending approval 之类的
ThreadManager 持有 policy
ToolCallRuntime / run_command 使用 policy
ThreadSession interrupt 时清理 policy 状态

6 loaded_threads
thread_id 对应一个 ThreadRuntime 防止一个thread 被重复spawn多个runtime
重复spawn 一个thread 同时写一个Rollout 广播两份event 同时跑两个turn 那就出问题了
Mutex支持并发 是因为 api server 多个http请求可以同时进来 访问同一个 所以需要保护一下

ThreadManager 是依赖和 runtime 索引的持有者；
runtime/agent/tools 才是真正消费这些依赖、执行模型调用、工具调用、命令审批和活进程管理的地方。


4条主流程

thread_start/thread_resume

ThreadStartParams {
    workspace_root: Option<String>,
    cwd: Option<String>,
}

ThreadStartParams
  -> OverridePolicy 合并 workspace_root/cwd
  -> 生成 thread_id
  -> 创建 SessionSnapshot
  -> 写一条 SessionMeta 到 rollout
  -> ensure_runtime_loaded
  -> 返回 ThreadView(status=Idle)

新 thread 会先写 durable 的 SessionMeta。
然后加载一个 ThreadRuntime 到内存。
返回的是 protocol 层的 ThreadView，不是 runtime 内部对象。

创建 thread 后不是写 DB。当前是写本地文件：

<workspace_root>/.exagent/threads/<thread_id>/rollout.jsonl

SessionSnapshot::new_thread 创建的是这个 thread 的初始状态，定义在 session.rs (line 148)。初始里面有：

session_id
root_session_id
parent_session_id = None
spawned_by_turn_id = None
agent_role = Primary
workspace_root
cwd
reference_turn_context = None
conversation = []
open_exec_sessions = []
latest_compaction = None
pending_approvals = []


它就是 thread 的初始 durable/live state。刚创建时还没有用户消息和 assistant 消息，所以 conversation 是空的。
写到 rollout 的 SessionMeta 是从 snapshot 提取出来的元信息，定义在 rollout.rs (line 21)：

thread_id
root_thread_id
parent_thread_id
spawned_by_turn_id
agent_role
workspace_root
initial_cwd
created_at

ensure_runtime_loaded 做的检查/动作是：
1. 先看 loaded_threads 里有没有这个 thread_id 的 runtime
2. 如果有，直接复用
3. 如果请求指定了 workspace_root，还会检查 loaded runtime 的 workspace 是否匹配
4. 如果没有 runtime，就 ThreadRuntime::spawn(...)
5. spawn 后放进 loaded_threads

ThreadView 是返回给外部看的 thread 状态，定义在 protocol.rs (line 91)：
ThreadView {
    id: SessionId,
    status: ThreadStatus,
    active_turn: Option<TurnView>,
    turns: Vec<TurnView>,
    snapshot_path: PathBuf,
    events_path: PathBuf,
}

ThreadView 的作用是：给外部入口一个可展示、可继续操作的 thread 摘要。

thread_start 不能只返回 “OK”，因为调用方马上需要知道：
新 thread 的 id 是什么
现在状态是什么
有没有 active turn
后续读状态/events 的路径是什么
比如 CLI adapter 里创建完 thread 后，下一步要：
events_subscribe(thread.thread.id)
turn_start(thread.thread.id, prompt)


TurnStartParams
  -> merge_turn_start
  -> ensure_runtime_loaded
  -> runtime.next_turn_id
  -> live_view
  -> apply turn_context.cwd
  -> runtime.submit_user_input
  -> 返回 TurnStartResponse
1. runtime.next_turn_id() 是做什么

它给这次 turn 分配一个 TurnId，比如： turn1 turn2 turn3 所以如果 conversation 里已经有 2 个 assistant message，下一次就是 turn_3。
这个 ID 后面会用于：

标记 runtime events 属于哪个 turn
TurnView.id
interrupt 某个 turn
CLI/API 等待某个 turn 的完成事件

turn_context.cwd 是什么

这里不是“以前有上下文跑过了”的意思。

TurnStartParams 里有一个可选字段： turn_context: Option<TurnContextOverrides>  这一次 turn 临时用哪个 cwd 执行
目前里面只有 cwd: Option<String>

AppServer
OverridePolicy::apply_turn_context(&live_view.snapshot, turn_context)
把 raw "src/runtime" 解析成 workspace 内的真实 PathBuf。

如果没有传 turn_context，runtime 就用 snapshot 当前的 cwd。

注意这里的 turn_context 和 runtime 里的 reference_turn_context 不是一回事：

TurnStartParams.turn_context：外部请求这次 turn 的临时 override。
snapshot.reference_turn_context：runtime 记录上一次模型可见的环境上下文，用来判断这次是否需要告诉模型 cwd/model/policy 等变化。

runtime.submit_user_input 是不是就进入 runtime 了
是的。准确说，是把这次用户输入封装成一个 runtime op，放进这个 thread runtime 的队列。
在 thread_runtime.rs (line 53) 里，runtime op 是：
ThreadOp::UserInput {
    turn_id,
    prompt,
    turn_context,
}

submit_user_input 里面会：
1. 创建 interrupt channel
2. reserve_active_turn，防止同一 thread 同时跑两个 turn
3. 把 ThreadSubmission 发进 op_tx 队列
4. 立即返回
然后 runtime loop 在后台收这个队列，见 thread_runtime.rs (line 388)：
op_rx.recv()
  -> ThreadOp::UserInput
  -> session.handle_user_input(turn_id, prompt, turn_context, interrupt)
到这里才进入 ThreadSession，开始真正的 turn 执行。

ThreadManager 做到这里就结束了：
protocol TurnStartParams
  -> runtime ThreadOp::UserInput

ThreadManager 在 turn_start 里已经开始调用 runtime 的两个能力：
runtime.next_turn_id()
runtime.submit_user_input(...)
但它还没有进入 agent loop。它只是
1. 找到/加载这个 thread 对应的 ThreadRuntime
2. 给这次 turn 分配 id
3. 把 prompt 包装成 ThreadOp::UserInput
4. 发进 runtime queue

真正转交发生在：
runtime.submit_user_input
  -> op_tx.send(ThreadSubmission { op: ThreadOp::UserInput, ... })

之后后台的 ThreadRuntimeLoop 收到这个 op：
ThreadRuntimeLoop
  -> ThreadSession.handle_user_input
  -> runtime/thread_session/turn.rs
从这里开始，app_server 基本退出执行细节，runtime 接管：
context update
TurnStarted event
LLM call
tool call
ToolResult event
TurnCompleted event
rollout persistence
live event broadcast

ThreadManager 的 turn_start 是“提交 runtime 工作”的边界；
ThreadRuntime 是“排队和串行化执行”的入口；
ThreadSession 才是真正处理 turn 的执行体。


3. events_replay / events_subscribe

events_replay：补历史事件。
EventsReplayParams
  -> OverridePolicy 合并 workspace_root
  -> 如果 runtime loaded：从 live_view.events 读
  -> 否则：从 rollout storage 读
  -> 按 after_event_id / event_kinds / limit 过滤
  -> 可选附带 snapshot view
  -> 返回 EventsReplayResponse

events_subscribe：订阅新事件。
EventsSubscribeParams
  -> 如果 runtime loaded：runtime.subscribe_events()
  -> 如果没 loaded 但 storage 有 thread：ensure_runtime_loaded 后 subscribe
  -> 否则 ThreadNotFound

API SSE 里会组合它们：
先 replay 已有 events
再 subscribe live events

所以客户端不会只看到“从现在开始”的事件，也能补上之前错过的事件。




thread_read / view building

thread_read 是读当前 thread 对外展示状态。
ThreadReadParams
  -> 如果 runtime loaded：读 runtime.live_view()
  -> 否则：从 rollout storage 重建 snapshot/events
  -> thread_read_from_state_view
  -> build_thread_view
  -> 返回 ThreadReadResponse

3. thread_read 是做什么

thread_read 是“读当前 thread 的外部视图”。

它不是直接返回原始 SessionSnapshot，也不是直接返回一堆 RuntimeEvent。它返回的是：
ThreadReadResponse {
    thread: ThreadView
}
这个 thread 现在是什么状态？
有没有 active turn？
有哪些 turns？
每个 turn 里有哪些展示 item？
snapshot/events path 是什么？


build thread view
原始事件大概是线性的：
TurnStarted
AssistantTurn
ToolResult
ExecOutput
TurnCompleted

build_thread_view 会整理成：

ThreadView
  turns:
    TurnView
      id: turn_1
      status: Completed
      items:
        AssistantMessage
        ToolResult
        ExecOutput
也就是从：

RuntimeEvent[]
变成：
ThreadView / TurnView / ThreadItem

events_replay = 给客户端补原始历史事件
events_subscribe = 给客户端推新的实时事件
thread_read = 给客户端一个当前 thread 的整理后视图
view building = 把 RuntimeEvent 流转换成 ThreadView/TurnView/ThreadItem

AssistantTurn       -> ThreadItem::AssistantMessage
ToolResult          -> ThreadItem::ToolResult
ExecOutput          -> ThreadItem::ExecOutput
ApprovalRequested   -> ThreadItem::ApprovalRequested
ApprovalDecision    -> ThreadItem::ApprovalDecision
RuntimeError        -> ThreadItem::RuntimeError
CompactionWritten   -> ThreadItem::CompactionWritten



ThreadItem、ThreadView、TurnView 都定义在 protocol.rs (line 91)，属于 app_server protocol/view 类型。
thread_manager.rs 只是负责把 runtime/state 的数据转换成这些 protocol view 类型：

RuntimeEvent[]
SessionSnapshot
RuntimeOverlay
    -> ThreadView
       -> TurnView[]
          -> ThreadItem[]

所以分层是：

runtime 层：
  RuntimeEvent
  ThreadRuntime
  ThreadSession
  SessionSnapshot
  RuntimeOverlay

app_server protocol 层：
  ThreadView
  TurnView
  ThreadItem

ThreadManager 做的是转换：


runtime/state 内部事实
  -> app_server 对外响应形态



生命周期大概是：

ThreadManager.ensure_runtime_loaded
  -> ThreadRuntime::spawn
      -> 创建 channels
      -> 创建 ThreadSession
      -> 启动后台 runtime loop
      -> 返回 Arc<ThreadRuntime>
  -> 存到 loaded_threads

后续 turn_start
  -> runtime.submit_user_input
  -> runtime loop 收到 ThreadOp::UserInput
  -> ThreadSession.handle_user_input

ensure_runtime_loaded = runtime 诞生/复用点
submit_user_input = 已存在 runtime 的任务投递点


什么时候会触发 ensure_runtime_loaded？

主要这些场景：

thread_start
thread_resume
turn_start
events_subscribe
/run
对于新 thread：

thread_start
  -> start_thread_with_options
  -> ensure_runtime_loaded
  -> ThreadRuntime::spawn

thread_resume / turn_start / events_subscribe
  -> ensure_runtime_loaded
  -> ThreadRuntime::spawn


runtime.submit_user_input 做的是：

把一次用户输入提交给已经存在的 ThreadRuntime
它不会创建 runtime。它假设 runtime 已经存在。

runtime 本身当然是 stateful 的。ThreadRuntime 里面有：

op_tx          // 操作队列入口
event_tx       // 事件广播通道
status_rx      // runtime 状态
active_turn    // 当前正在跑的 turn
live_state     // 当前 thread 的 live snapshot/events/overlay
可以记成：

ThreadRuntime = 每个 thread 的内存 actor
它负责：

串行接收 ThreadOp
防止同一个 thread 同时跑多个 active turn
广播 runtime events
暴露 live view
把操作交给 ThreadSession 执行



Event 为什么触发 runtime 思考 可能后续需要更改什么的
events_subscribe 本身不是创建 thread，也不是启动 turn，但它现在需要一个 loaded runtime，原因是：

live event broadcast channel 是 ThreadRuntime 持有的
也就是说，想订阅“未来发生的新事件”，必须拿到：

runtime.subscribe_events()
而这个 runtime 如果还不在内存里，就只能先：

ensure_runtime_loaded
  -> ThreadRuntime::spawn
  -> 从 rollout 恢复 ThreadSession/live state
  -> 创建 event broadcast channel
  -> 返回 receiver
所以 events_subscribe 触发 ensure_runtime_loaded 的含义不是“开始执行任务”，而是：

为了能订阅 live events，把这个 thread 的 runtime actor 加载到内存里。
它不会自动跑 LLM，也不会自动 start turn。它只是让这个 thread 有一个 runtime event channel 可以订阅。

如果不这么做，就只能走另一种设计：订阅 rollout 文件变化，或者等 turn_start 时 runtime 被加载后再挂上 subscriber。但当前架构选择的是：

live subscribe = 订阅 ThreadRuntime 的 broadcast channel
所以需要 runtime。




这里也需要思考 是为了后续的拓展 仅仅cwd 会不会有些太过于小了
第二个问题，ThreadTurnContext 现在确实名字有点大，但当前结构很小，定义在 thread_runtime.rs (line 48)：

pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
}

所以 目前 ThreadTurnContext 只承载 turn-scoped cwd override。
但后面进入 ThreadSession 后，会基于这个 cwd、AgentConfig、workspace 信息，生成更完整的模型可见 context。也就是：

TurnStartParams.turn_context.cwd
  -> ThreadTurnContext.cwd
  -> ThreadSession 里变成 PromptContext / TurnContextItem

这里要区分两个 context：

ThreadTurnContext
  runtime op 上携带的临时执行上下文
  当前只有 cwd

TurnContextItem / PromptContext
  runtime 内部生成的模型可见上下文
  包含 workspace_root、cwd、model、policy_mode、timeout、max_output 等