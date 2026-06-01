# Rollout Persistence

## Responsibility

`src/state/rollout.rs` defines the rollout item format and reads/writes rollout JSONL files.

## Rollout Items

- `SessionMeta`
- `ResponseItem`
- `TurnContext`
- `Compacted`
- `EventMsg`

## Key Flows

- New thread writes `SessionMeta`.
- Turns append context and response items.
- Selected events append as `EventMsg`.
- Restore reads all rollout items and rebuilds snapshot, events, and context.

## Persistence Rule

Rollout should contain enough information to reconstruct durable thread state and audit important lifecycle events. It should not store live handles.

`ExecOutput` chunks are intentionally not persisted as main rollout events today. Streaming command output can be large and high-frequency, so writing every chunk into `rollout.jsonl` would make the thread source log noisy and expensive to replay.

This does not mean command output should remain live-only forever. Future command-log durability should use a separate exec log stream, for example per-exec stdout/stderr files or an exec-log JSONL, while rollout stores command lifecycle records, summaries, offsets, and references to those logs.

LLM-facing tool results should prefer output deltas, tails, or summaries instead of repeatedly appending full accumulated stdout/stderr snapshots to conversation history.


durable source of truth，负责把一个 thread 的历史以 append-only JSONL 存下来，并在 cold resume 时重建状态。
------
它存 5 类 RolloutItem：
SessionMeta
ResponseItem
TurnContext
Compacted
EventMsg
分别是：
SessionMeta: thread 元信息，thread_id、root_thread_id、workspace_root、initial_cwd、created_at。
ResponseItem: conversation message，包含 user/assistant/tool message。
TurnContext: 每轮当时的 cwd、model、policy、timeout 等上下文。
Compacted: compaction summary 和 replacement history。
EventMsg: 被选择持久化的 runtime events。

<workspace_root>/.exagent/threads/<thread_id>/rollout.jsonl

---
thread_start
 -> 写 SessionMeta

turn start
 -> 写 TurnContext
 -> 写 injected context messages
 -> 写 user message

assistant/tool loop
 -> 写 assistant message
 -> 写 tool result message

compaction
 -> 写 Compacted

event recorder
 -> 写 selected EventMsg

----
恢复时怎么做
恢复不是重新执行历史 turn，而是读全部 rollout item，然后投影出当前状态：
snapshot_from_rollout_items(...)
events_from_rollout_items(...)
ContextManager::from_rollout_items(...)

snapshot_from_rollout_items 在 line 48 (line 48)，它会：
找 SessionMeta
校验 requested thread_id 是否匹配
收集 ResponseItem 变成 conversation
用最后的 TurnContext 作为 reference context
遇到 Compacted 就用 replacement_history 替换 conversation
不恢复 open_exec_sessions
不恢复 pending_approvals
这里就是我们前面说的边界：rollout 恢复 durable state，不恢复 live-only state。


---
事件持久化策略
不是所有 event 都写进去。规则在 should_persist_event (line 132)。
会持久化：
TurnStarted
TurnCompleted
TurnInterrupted
RuntimeError
ApprovalRequested
ApprovalDecision
TokenCount

不会作为 EventMsg 持久化：
AssistantTurn
ToolResult
ExecOutput
CompactionWritten

但注意：assistant/tool 的内容不是丢了，它们通常已经以 ResponseItem 写进 rollout。ExecOutput 这种流式 chunk 目前不持久化，这也呼应我们刚才说 command log 后续要补策略。

---
RolloutStore
RolloutStore 在 line 146 (line 146)，就是文件读写封装：
append_items
append_items_blocking
read_items
read_items_blocking
写之前会先过 should_persist_rollout_item，所以调用方可以尝试写 EventMsg，但真正落不落盘由 rollout policy 决定。


---
ContextManager = 当前 LLM 要看的上下文
SessionSnapshot = 当前 thread 对外可读状态
RuntimeOverlay = live-only UI 状态
Rollout = 可持久恢复的事实日志
