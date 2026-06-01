# App Server Protocol

## Responsibility

`src/app_server/protocol.rs` defines the public app-server contract.

It contains request and response shapes for:

- initialization
- thread start/read/resume
- turn start/interrupt
- event replay/subscribe
- generic boundary operations

## State

Protocol types are serialized data. They do not own runtime state.

## Important Types

- `BoundaryCapability`
- `ThreadView`
- `TurnView`
- `ThreadItem`
- `BoundaryOp`
- `BoundaryOpResponse`
- `EventsReplayParams`
- `EventsReplayResponse`

## Design Rule

Keep protocol types stable and explicit. Runtime internals should not leak into public response shapes unless clients need them.

## Naming Debt

`ThreadView.id` and several protocol params use the `SessionId` Rust type, but in the app-server protocol they semantically mean `thread_id`.

This is a historical naming mismatch: lower-level state/runtime code still uses `SessionId`, while the app-server boundary exposes threads as the public concept.

Future cleanup should either rename the underlying ID type or introduce a protocol-level `ThreadId` newtype so `session_id` and `thread_id` do not read as separate concepts.

app_server的schema 确定了 这些types 里面带什么数据 长什么样子

BoundaryCapability：app_server 声明自己支持哪些能力，比如 ThreadStart、TurnStart、EventsReplay
ThreadView：给外部看的 thread 状态   这个也挺关键的
TurnView：给外部看的 turn 状态  这个挺关键的
ThreadItem：turn 里面展示的 item，比如 user message、assistant message、tool result、approval、runtime error
BoundaryOp：统一封装的边界请求 enum   通信 有哪些可以通信
BoundaryOpResponse：统一封装的边界响应 enum
EventsReplayParams：replay events 时客户端能传什么过滤条件
EventsReplayResponse：replay 返回哪些 events，是否带 snapshot

与runtime的关系 并不存储 stateless 只是字符串 记录 进行通信什么的

entrypoints
  用 protocol params 传请求

app_server
  大量使用 protocol.rs 的 Params / Response / View
  把它们翻译成 runtime 操作

runtime
  主要使用自己的内部类型
  比如 ThreadRuntime、ThreadOp、ThreadSubmission、ThreadSession、RuntimeEvent、Agent、ToolCallRuntime

runtime 会间接收到一些 从protocol传来的数据

TurnStartParams.prompt 最后会变成 runtime 的 ThreadOp::UserInput { prompt }
TurnStartParams.turn_context.cwd 会被 app_server 解析后变成 ThreadTurnContext
ThreadStartParams.workspace_root/cwd 会被 override policy 合并进 AgentConfig
runtime 执行后产生的 RuntimeEvent 会被 app_server 转成 ThreadView / TurnView
所以 protocol.rs 更像 边界语言，runtime 更像 内部执行语言。

Protocol                         Runtime/Internal
-------------------------------------------------
ThreadStartParams         ->     AgentConfig + SessionSnapshot + ThreadRuntime spawn
ThreadResumeParams        ->     load existing rollout + ThreadRuntime spawn
TurnStartParams           ->     ThreadOp::UserInput
TurnInterruptParams       ->     ThreadOp::Interrupt
EventsSubscribeParams     ->     broadcast::Receiver<RuntimeEvent>
EventsReplayParams        ->     rollout/live events filtering
ThreadView / TurnView     <-     RuntimeEvent + SessionSnapshot + RuntimeOverlay

protocol.rs 是 app_server 对外的语言；
runtime 不直接“以 protocol 为中心”，而是 app_server 把 protocol 翻译成 runtime 能理解的内部操作。
