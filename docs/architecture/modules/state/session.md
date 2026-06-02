# Session State

## Responsibility

`src/state/session.rs` defines snapshot and session-adjacent state shapes.

## Important Types

- `SessionSnapshot`
- `TurnContextItem`
- `ExecSessionRef`
- `PendingApproval`
- `CompactionSummary`
- `AgentRole`

## Key Rule

`SessionSnapshot` is the durable/current view of thread state, but live-only handles remain outside it in runtime overlay or managers.



SessionSnapshot 的作用可以理解成：当前 thread 状态的结构化投影。

它不是 LLM prompt 的权威，也不是持久化权威：

ContextManager
  LLM prompt 权威：下一轮模型到底看到什么

Rollout
  durable 权威：进程重启后从哪里恢复

SessionSnapshot
  state/view 权威：当前 thread 对外展示和内部检查用的结构化状态
它主要解决几个问题。
------

1. 给 app-server / UI 一个容易读的状态
如果没有 SessionSnapshot，thread_read 想知道当前 cwd、conversation 长度、latest compaction、pending approvals，就得每次去扫 ContextManager、overlay、rollout、events。

有 snapshot 后，可以直接投影成：

ThreadView
TurnView
ReplaySnapshotView
比如：

snapshot.conversation
snapshot.cwd
snapshot.latest_compaction
snapshot.workspace_root

------

2. 给 runtime 一个当前状态副本
每个 turn 开始时，ThreadSession 会从 live_state.snapshot clone 一份出来，在 turn 过程中更新它，最后通过 recorder/live_state 发布。

比如：

record user message
 -> ContextManager.record_items
 -> context_manager.sync_snapshot(snapshot)

record assistant/tool result
 -> ContextManager.record_items
 -> context_manager.sync_snapshot(snapshot)

compaction
 -> context_manager.replace_history
 -> context_manager.sync_snapshot(snapshot)
 -> snapshot.latest_compaction = ...
所以 snapshot 是 runtime 内部“当前状态镜像”。


-----
3. 把多个来源合成一个结构
它把这些东西放在一个结构里：

identity: session_id/root/parent/role
workspace: workspace_root/cwd
conversation: messages
context marker: reference_turn_context
live view refs: open_exec_sessions/pending_approvals
compaction: latest_compaction
这些不完全属于同一个底层来源，但对外看 thread 状态时需要一起出现。


------
4. 恢复后快速拥有当前状态
rollout 是 append-only log，不适合每次直接展示。恢复时先把 log 投影成 snapshot：

rollout items -> SessionSnapshot
然后后续 thread_read 就不用一直理解整份 log。

所以你可以把三者这样记：

Rollout = 事实日志
ContextManager = 模型上下文
SessionSnapshot = 当前状态快照
如果只保留 rollout，系统能恢复，但每次读状态都要重新 replay。
如果只保留 context，LLM 能继续，但 app-server/UI 不好展示状态。
如果只保留 snapshot，状态展示方便，但没有完整历史/audit，也没法干净恢复上下文变更和事件。




state/session.rs 是 session/thread 状态数据结构定义。它本身不读写文件、不跑 runtime、不调用 LLM，只定义“状态长什么样”。

核心类型在 src/state/session.rs (line 100)。

SessionSnapshot
这是最重要的结构：

pub struct SessionSnapshot {
    pub session_id: SessionId,
    pub parent_session_id: Option<SessionId>,
    pub root_session_id: SessionId,
    pub spawned_by_turn_id: Option<TurnId>,
    pub agent_role: AgentRole,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub reference_turn_context: Option<TurnContextItem>,
    pub conversation: Vec<ConversationMessage>,
    pub open_exec_sessions: Vec<ExecSessionRef>,
    pub latest_compaction: Option<CompactionSummary>,
    pub pending_approvals: Vec<PendingApproval>,
}
这里的 session_id 还是之前那个命名债：语义上是 thread id。

它是 当前 thread 状态的结构化镜像，主要给：

live_state.snapshot
thread_read
ThreadView
rollout cold restore 后的状态重建
但 LLM prompt 不直接从它取，而是从 ContextManager.items 取。SessionSnapshot.conversation 是同步出来的可读状态。

new_thread
SessionSnapshot::new_thread 在 line 148 (line 148)，thread_start 时会用它创建初始 snapshot：

root_session_id = session_id
agent_role = Primary
conversation = []
open_exec_sessions = []
pending_approvals = []
latest_compaction = None
所以新 thread 一开始只是一个空壳，后续 turn 才填 conversation/context。

TurnContextItem
在 line 172 (line 172)，它记录“某一轮 turn 当时的运行上下文”：

workspace_root
cwd
model
policy_mode
command_timeout_secs
max_output_bytes
current_utc_date
它会被写进 rollout，也会被 ContextManager 用来判断是否需要注入 context update。

ExecSessionRef / PendingApproval
这两个是 overlay/live view 用的“引用结构”，不是实际 handle。

ExecSessionRef：

exec_session_id
command
cwd
status
它只是告诉 UI/API 当前有一个 open exec session。真正的 child process 在 ExecSessionManager.sessions。

PendingApproval：

approval_id
requested_event_id
tool_name
reason
status
它只是告诉 UI/API 当前有 approval 等待处理。真正 pending command 在 PolicyManager.pending。

CompactionSummary
记录最近一次 compact 的 summary，给 snapshot/view 展示，也从 rollout 的 Compacted 恢复。

AgentRole
目前有：

Primary
Spec
Test
Judge
Implementation
现在主流程大多还是 Primary，后面如果做多 agent/thread spawn，这个会更有用。

和 rollout 的关系
rollout 恢复 snapshot 的地方在 src/state/rollout.rs (line 48)：

SessionMeta -> session_id/root/workspace/cwd/role
ResponseItem -> conversation
TurnContext -> reference_turn_context
Compacted -> latest_compaction + replacement_history
EventMsg -> 不进 snapshot
并且恢复时明确：

open_exec_sessions: vec![],
pending_approvals: vec![],
这就是 durable state 和 live-only state 的边界。

一句话总结：

session.rs = 状态类型定义
rollout.rs = 这些状态怎么持久化/恢复
runtime/thread_session = 运行时怎么更新这些状态
app_server/thread_manager = 怎么把这些状态投影成 ThreadView







对，rollout 恢复 snapshot 指的就是：从 rollout.jsonl 重新构建一个 SessionSnapshot。

场景一般是：

进程重启 / thread 不在内存里 / resume 冷加载
 -> 读 .exagent/threads/<thread_id>/rollout.jsonl
 -> snapshot_from_rollout_items(...)
 -> 得到 SessionSnapshot
 -> ThreadSession 初始化
 -> 后续可以继续 turn
但恢复时不只恢复 SessionSnapshot，实际上会恢复三块：

1. SessionSnapshot
   给 live_state / ThreadView / 当前状态镜像用

2. ContextManager
   给下一轮 LLM prompt 用

3. events
   给 replay / live view 历史事件用
也就是：

let snapshot = snapshot_from_rollout_items(...)
let context_manager = ContextManager::from_rollout_items(...)
let events = events_from_rollout_items(...)
你说的后半句也对：恢复后开始新 turn 时，会基于恢复出来的状态继续跑。

流程大概是：

恢复 ContextManager.items
恢复 SessionSnapshot.reference_turn_context
新 turn 开始
 -> PromptContext::for_turn(current config + cwd)
 -> ContextManager.apply_context_updates(...)
 -> 比较 previous TurnContextItem 和 current TurnContextItem
 -> 如果 cwd/model/policy 等变了，就 append context update
 -> append 新 user message
 -> LLM 继续跑
更精确一点：
比较上下文变化主要发生在 ContextManager 里，不是 SessionSnapshot 自己比较。SessionSnapshot.reference_turn_context 是被恢复出来后同步给可读状态的；真正控制 prompt 拼接的是 ContextManager.reference_turn_context。

所以一句话：

rollout 恢复出 SessionSnapshot 是为了恢复 thread 状态；
rollout 恢复出 ContextManager 是为了让下一轮 LLM 能接着已有上下文继续。
这两个都来自同一份 rollout，但用途不同。





对，目前 AgentRole 基本没有实际业务逻辑。

我查了一下，它现在只用于：

SessionSnapshot.agent_role
SessionMeta.agent_role
rollout 序列化/恢复
默认值 Primary
测试里构造 meta
没有看到 runtime/app-server 根据 Spec/Test/Judge/Implementation 做分支，也没有不同 tool registry、prompt、policy、model 的实际差异。

所以它现在更像是 为未来 multi-agent / role-based thread 预留的字段，不是当前主流程必需。

可以这样记录：

AgentRole 当前只作为 snapshot/rollout 元数据保存。
主流程固定为 Primary，没有按 role 改变 runtime 行为。
后续如果不做多 agent，可以删除或收敛。
如果做多 agent，它可以驱动不同 prompt、tool registry、policy、model、view 展示。
我建议先不删，因为 rollout schema 里已经有它，直接删会涉及数据兼容；但可以在文档里标成“reserved / currently metadata-only”。
