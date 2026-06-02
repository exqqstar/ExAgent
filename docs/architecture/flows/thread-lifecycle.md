# Thread Lifecycle

Thread lifecycle is coordinated by `ThreadManager`.

```mermaid
sequenceDiagram
    participant Client
    participant Boundary as AppServerService
    participant Manager as ThreadManager
    participant Runtime as ThreadRuntime
    participant Rollout as rollout.jsonl

    Client->>Boundary: thread_start / thread_resume
    Boundary->>Manager: thread_start / thread_resume
    Manager->>Rollout: write/read SessionMeta
    Manager->>Manager: ensure_runtime_loaded
    Manager->>Runtime: ThreadRuntime::spawn if not loaded
    Runtime->>Rollout: restore ThreadSession from rollout
    Manager-->>Client: ThreadView
```

## Key State Changes

- New threads create a `SessionMeta` rollout item.
- Resumed threads must already have a non-empty rollout.
- Loaded runtime instances are cached in `ThreadManager.loaded_threads`.
- Cold replay restores durable snapshot/events but does not recreate open subprocesses or pending approval waiters.

## Main Files

- `src/app_server/thread_manager.rs`
- `src/runtime/thread_runtime.rs`
- `src/runtime/thread_session/mod.rs`
- `src/state/rollout.rs`
