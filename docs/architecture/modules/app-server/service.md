# App Server Service

## Responsibility

`src/app_server/service.rs` defines `AppServerBoundary` and implements it with `AppServerService`.

This is the shared boundary used by both CLI and HTTP.

## State

`AppServerService` owns one `ThreadManager`.

## Key Flow

1. Entrypoint receives request.
2. Entrypoint calls `AppServerBoundary`.
3. `AppServerService` forwards to `ThreadManager`.
4. Result is returned to CLI or HTTP.

## Why This Exists

It prevents CLI and HTTP from each inventing their own runtime behavior. Both go through the same operations and error semantics.

entrypoints
  -> AppServerBoundary
      -> AppServerService
          -> ThreadManager
              -> runtime


AppServerBoundary 是能力清单，也就是 entrypoints 能调用什么：

run
thread_start
thread_read
thread_resume
turn_start
turn_interrupt
submit_boundary_op
events_replay
events_subscribe
AppServerService 是这个能力清单的具体实现，但它本身很薄。它内部只存了一个：
thread_manager: ThreadManager

大部分实现都是转发给 thread_manager进行实现
pub fn thread_start(&self, params: ThreadStartParams) -> Result<ThreadStartResponse> {
    self.thread_manager.thread_start(params)
}
