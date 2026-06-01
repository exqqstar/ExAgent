# Override Policy

## Responsibility

`src/app_server/override_policy.rs` applies request-level `workspace_root` and `cwd` overrides.

It protects runtime operations from accidentally targeting a directory outside the intended workspace.

## State

No long-lived state. It transforms `AgentConfig` or `SessionSnapshot` values.

## Key Rules

- `workspace_root` is canonicalized from the current process directory when provided.
- `cwd` is canonicalized relative to `workspace_root` when provided.
- `cwd` must remain inside `workspace_root`.
- Resume currently ignores `cwd` override and reports it as ignored.

## Connections

- Called by `ThreadManager`.
- Mirrors some lower-level path safety logic from `src/workspace.rs`, but at the boundary/request level.

只要请求里可能带 workspace_root / cwd，ThreadManager 进入真正操作前，就会先触发 override_policy。

它做三件事：

从 ThreadManager.base_config clone 一份 config。
根据不同 boundary operation，决定允许覆盖哪些字段。
canonicalize 路径，并保证 cwd 不跑出 workspace_root。
当前主要处理：

thread_start：允许 workspace_root + cwd
thread_read / thread_resume / turn_start / events_replay：主要允许 workspace_root
turn_context：允许单个 turn 临时覆盖 cwd
它不是真正 sandbox，也不是 runtime 执行逻辑。它是 app_server 边界层的 config/path override 策略。


1. 创建 thread 时

如果 API 请求：

POST /thread/start
{
  "workspace_root": "/project-a",
  "cwd": "src"
}
会走：

ThreadManager::thread_start
  -> OverridePolicy::merge_thread_start
  -> 生成新的 AgentConfig
  -> 创建 thread / runtime
结果是这个新 thread 的：

workspace_root = /project-a
cwd = /project-a/src


2. 恢复 thread 时

如果请求：

POST /thread/resume
{
  "thread_id": "...",
  "workspace_root": "/project-a"
}
会走：

ThreadManager::thread_resume
  -> OverridePolicy::merge_thread_resume
这里主要是用 workspace_root 定位/校验这个 thread。cwd override 当前会被忽略。
