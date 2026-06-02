# Compaction

## Responsibility

`src/runtime/compaction.rs` summarizes conversation history so the active prompt can be shortened.

## State

No long-lived state in this file. It returns:

- summary text
- replacement conversation history

`ContextManager` and `SessionSnapshot.latest_compaction` store the result.

## Key Flow

1. Build a compaction prompt from existing conversation history.
2. Ask the agent's LLM for a summary.
3. Reject empty summaries.
4. Return replacement history with an injected summary system message.

## Connections

- Called from `thread_session/turn.rs`.
- Persisted as `RolloutItem::Compacted`.
- Emitted as `RuntimeEventKind::CompactionWritten`.


真正触发 compact 的地方不在这个文件，而是在 thread_session/turn.rs (line 195)：
turn 前主动 compact：如果 active_context_tokens >= auto_compact_token_limit，先 compact，再记录新的 user prompt。
context window error 后兜底 compact：LLM 已经报上下文爆了，就 compact，然后重新注入当前 context，再把最后一条 user message 加回去，再 retry 一次。
compact 结果真正落地是在 record_compaction_checkpoint (line 384)：
context_manager.replace_history(...)
context_manager.sync_snapshot(snapshot)
snapshot.latest_compaction = Some(...)
写 RolloutItem::Compacted
发 RuntimeEventKind::CompactionWritten
再发一次 TokenCount


所以分层是：

compaction.rs
  只负责：旧 history -> summary + replacement_history

thread_session/turn.rs
  负责：什么时候 compact、compact 后怎么写回 context/snapshot/rollout/events

ThreadOp? 这里可以收敛一下吗 毕竟有很多 文件都是stateless 更像是一个任务的形式 你说呢
