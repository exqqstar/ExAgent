# App Server Module

## Responsibility

`app_server` is the protocol boundary between clients and runtime.

It owns:

- protocol DTOs
- boundary facade
- thread runtime lookup
- thread and turn orchestration
- replay and subscription entry points

It does not execute LLM loops directly.

## State

Primary state:

- `ThreadManager.loaded_threads`: in-memory map from `thread_id` to loaded `ThreadRuntime`.

Shared runtime resources:

- `ExecSessionManager`
- `PolicyManager`
- LLM factory
- tool registry factory

## File Map

- [protocol.md](protocol.md): protocol request/response types.
- [service.md](service.md): boundary trait and facade.
- [thread-manager.md](thread-manager.md): orchestration and runtime loading.
- [override-policy.md](override-policy.md): workspace/cwd override rules.

## Key Flows

- `thread_start`: validate overrides, create rollout metadata, load runtime, return `ThreadView`.
- `thread_resume`: validate storage, load or reuse runtime, return current `ThreadView`.
- `turn_start`: load runtime, submit user input, return in-progress turn.
- `events_replay`: read stored or live events and apply filters.
- `events_subscribe`: ensure runtime is loaded and return broadcast receiver.

## Connections

- Upstream: `entrypoints`.
- Downstream: `runtime`, `state`, `model`, `tools`.

## Extension Points

- Add an API operation by updating protocol, service boundary, thread manager, and HTTP route.
- Add a new `ThreadView` item by updating protocol view types and event-to-view conversion.


一层层解耦
entrypoints
  只关心外部输入形态：CLI args / HTTP JSON / SSE

AppServerBoundary
  定义 app_server 对外承诺的能力

AppServerService
  提供这个能力清单的具体实现

ThreadManager
  真正处理 thread/turn/events/runtime loading

为什么这么做：

入口层不绑定具体实现
cli_adapter.rs 只接收：

boundary: &dyn AppServerBoundary
它不关心后面是 ThreadManager、mock service，还是未来别的实现。这样 CLI/API 都可以复用同一个 app_server 能力。

协议边界稳定
entrypoints 只知道 ThreadStartParams、TurnStartParams 这些 protocol struct。
至于 thread 怎么存、runtime 怎么 spawn、events 怎么 replay，都藏在 app_server 内部。

测试更容易
如果测试 CLI adapter，不需要真的启动 runtime，可以塞一个假的 AppServerBoundary。
如果测试 app_server，则直接测 ThreadManager 或 AppServerService。

未来可以换入口或换实现
以后如果加 Tauri、gRPC、WebSocket、MCP server，入口层只要把输入转成 protocol params，然后调用 AppServerBoundary。
不用每个入口都重新实现 thread 管理逻辑。

ThreadManager 保持为核心编排器
复杂逻辑集中在一个地方：thread start/resume/read、turn start/interrupt、runtime loading、events replay/subscribe。
不分散到 CLI/API 里。