# Context Manager

## Responsibility

`src/runtime/context.rs` manages model-visible conversation history.

It decides what messages are sent to the LLM, injects runtime/environment context, tracks token usage, and estimates active context size.

## State

- `items`: conversation messages visible to the model.
- `reference_turn_context`: previous runtime context used to compute context diffs.
- `token_info`: token usage summary for compaction decisions.
- `history_version`: internal change counter.

## Key Flow

1. Build `PromptContext` from config and turn paths.
2. Inject full context for first turn or diff messages for changed context.
3. Record user, assistant, and tool messages.
4. Sync `SessionSnapshot.conversation`.
5. Replace history when compaction writes a summary.

## Extension Points

- Add new context fields to `TurnContextItem`.
- Change token estimation or compaction thresholds.
- Change how runtime/environment context is injected.

ThreadSession 里面的 LLM 上下文管理器，负责决定“这一轮发给 LLM 的 messages 到底是什么”。

核心在 src/runtime/context.rs (line 8)：
items: 当前模型可见的完整 conversation messages。
reference_turn_context: 上一次注入给模型的运行环境，用来判断本轮需不需要补充 diff。
token_info: token 使用信息，给 compaction 判断上下文是否快满。
history_version: 内部版本号，记录上下文是否变过。
它的主流程是：
每轮 turn 开始时，用 AgentConfig + workspace_root + cwd 生成 PromptContext。
也就是把 model、policy、timeout、max_output、workspace、cwd、UTC date 这些变成 TurnContextItem。见 for_turn (line 27)。

apply_context_updates 判断要不要注入 context message。
第一轮没有 reference_turn_context，所以注入完整 runtime/environment context。
后续轮次只注入变化，比如 cwd 变了、policy 变了、model 变了。见 apply_context_updates (line 105)。

真正发给 LLM 的 prompt 来自 for_prompt()。
它就是把 items clone 出去。也就是说 LLM 的上下文主要以 ContextManager.items 为准，不是直接以 SessionSnapshot 为准。见 for_prompt (line 130)。

它会从 rollout 恢复。
from_rollout_items 会读回历史消息、turn context、compaction 后的 replacement history、token info。见 from_rollout_items (line 47)。

它同步 snapshot，但自己不是持久化层。
sync_snapshot 只是把当前 conversation 和 reference context 拷贝给 SessionSnapshot。真正写 rollout 还是 turn/session 那边做。见 sync_snapshot (line 73)。

所以一句话：context.rs 是 runtime 内部的“模型可见上下文状态机”。它不负责调用 LLM，也不负责写文件；它负责维护 LLM 应该看到的消息、环境注入、token 估算，以及 compaction 后的历史替换。