# Project Shell Module

## Responsibility

The project shell is the crate and binary boundary around the main architecture modules.

It owns process startup wiring, public crate exports, default runtime configuration, and shared workspace path helpers.

It does not own thread lifecycle, active agent execution, durable rollout state, or individual tool behavior.

## State

No long-lived runtime state is owned here.

Values defined or prepared here are passed into the owning modules:

- `AgentConfig`: runtime settings consumed by `app_server`, `runtime`, and `tools`.
- environment defaults: model, policy mode, context window, and compaction limit.
- workspace paths: `workspace_root` and `cwd` carried through runtime config.

## File Map

- `src/main.rs`: binary startup, tracing initialization, CLI parsing, and CLI/API branching.
- `src/lib.rs`: public crate modules, re-exports, and `default_tool_registry`.
- `src/config.rs`: `AgentConfig`, environment defaults, and auto-compaction token limit resolution.
- `src/workspace.rs`: workspace path canonicalization and workspace-bounded path resolution helpers.

## Key Flows

Binary startup:

1. `main.rs` initializes tracing.
2. It parses the command through `entrypoints::cli`.
3. `Api` commands call `entrypoints::api::serve`.
4. Other commands create `AppServerService` and call `entrypoints::cli_adapter`.

Default service construction:

1. `AppServerService::new` starts from `AgentConfig::default`.
2. `ThreadManager` uses `crate::default_tool_registry` as the default tool registry factory.
3. Runtime components receive cloned `AgentConfig` values when threads and turns are loaded.

Workspace path handling:

1. Boundary overrides canonicalize `workspace_root` and `cwd` before runtime use.
2. Tools resolve model-supplied file paths against `AgentConfig.workspace_root`.
3. Absolute paths and parent traversal are rejected by shared workspace helpers.

## Connections

- Upstream: process execution, library consumers, environment variables.
- Downstream: `entrypoints`, `app_server`, `runtime`, `tools`, and `policy`.
- Related module pages: [entrypoints.md](entrypoints.md), [app-server/override-policy.md](app-server/override-policy.md), [tools/read-write-file.md](tools/read-write-file.md).

## Extension Points

- Add a public module or re-export in `src/lib.rs` when another crate boundary needs it.
- Register a default LLM-callable tool in `default_tool_registry` after implementing it under `tools`.
- Add an `AgentConfig` field when a setting must travel through app-server, runtime, or tool execution.
- Add workspace helper behavior in `src/workspace.rs` when multiple modules need the same path safety rule.


main.rs为程序入口 初始化tracing 解析cli 决定走api server还是 cli adapter 然后app server是内部边界服务 是不同入口共同的调用边界

Config.rs 定义Agentconfig  被后续的AppserverService ThreadManager 或者各个地方复用 override
加参数 如果后面很多地方都会复用

default/env config
    -> base_config
        -> per thread / per request config
            -> runtime consumes
            -> tools consume

workspace目前只是路径层面的 guardrail 而不是sandbox
