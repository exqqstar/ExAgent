# Entrypoints Module

## Responsibility

Entrypoints adapt external interaction styles into app-server boundary calls.

They do not own agent execution, durable state, or runtime scheduling.

## State

Entrypoints do not own core runtime state. They hold request-local values only:

- parsed CLI arguments
- Axum request JSON
- SSE stream handles
- temporary event receivers

## File Map

- `src/main.rs`: initializes tracing, parses CLI, branches into API server or CLI adapter.
- `src/entrypoints/cli.rs`: parses `Run`, `Resume`, and `Api` commands.
- `src/entrypoints/cli_adapter.rs`: creates/resumes threads, subscribes to events, starts turns, waits for final assistant text.
- `src/entrypoints/api.rs`: builds Axum routes and maps boundary results to HTTP responses or SSE streams.

## Key Flows

CLI run:

1. `main.rs` parses args.
2. `cli_adapter::execute_cli_command` calls `thread_start`.
3. It subscribes to events.
4. It calls `turn_start`.
5. It waits for `AssistantTurn` and `TurnCompleted`.

HTTP turn start:

1. `api::turn_start` receives JSON.
2. It calls `AppServerBoundary::turn_start`.
3. It returns `TurnStartResponse` immediately.
4. Progress is observed through `events/subscribe`.

## Connections

- Upstream: user shell, HTTP clients.
- Downstream: `app_server::AppServerBoundary`.

## Extension Points

- Add a CLI subcommand in `cli.rs` and route it in `cli_adapter.rs`.
- Add an HTTP endpoint in `api.rs` and a protocol DTO in `app_server/protocol.rs`.

cli_adapter use crate::app_server::AppServerBoundary 依赖app_server暴露出来的边界接口 而不是直接依赖内部实现
trait AppServerBoundary {
    run(...)
    thread_start(...)
    thread_read(...)
    thread_resume(...)
    turn_start(...)
    turn_interrupt(...)
    submit_boundary_op(...)
    events_replay(...)
    events_subscribe(...)
}
AppServerBoundary = entrypoints 能调用的 app-server 能力清单
他规定了入口层可以做什么 但是真正实现是appserverservice 然后往下面再交给threadmanager

CLI/API 处理输入，把输入清洗成 protocol params，
然后通过 AppServerBoundary 交给 app_server。
api.rs：HTTP JSON -> protocol params -> boundary
cli.rs：raw args -> CliCommand
cli_adapter.rs：CliCommand -> protocol params -> boundary

app_server events_subscrbe 订阅runtime event stream
/events/subscribe 把这个event stream 包装成sse 推给http client
