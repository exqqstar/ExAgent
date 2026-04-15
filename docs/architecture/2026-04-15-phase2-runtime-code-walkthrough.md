# ExAgent Phase 2 Runtime Code Walkthrough

**Date:** 2026-04-15  
**Audience:** 想从源码层面真正吃透当前 Phase 2 实现的人  
**Status:** 对应当前 `codex/phase2-p0-runtime` worktree

## 1. 这份文档怎么用

上一份 study guide 偏“架构视角”。这一份偏“读代码视角”。

你可以把它当成一张阅读地图：

- 先看哪些文件
- 每个文件先看哪些结构和函数
- 关键逻辑是怎么一层层接起来的
- 哪些地方是设计上的分界线
- 哪些地方是当前实现的妥协点

如果你边看边打开源码，建议按下面顺序：

1. `src/types.rs`
2. `src/session.rs`
3. `src/events.rs`
4. `src/transcript.rs`
5. `src/registry.rs`
6. `src/agent.rs`
7. `src/tools/run_command.rs`
8. `src/exec_session.rs`
9. `src/policy.rs`
10. `src/api.rs`
11. `src/cli.rs`

原因很简单：  
先理解“数据长什么样”，再理解“逻辑怎么流动”。

## 2. 最先看：类型层

### `src/types.rs`

这个文件是整个运行时的最底层基础类型。

最值得先看的地方：

- `SessionId` / `TurnId` / `EventId`
- `ToolCall`
- `ToolResult`
- `AssistantTurn`
- `ConversationMessage`
- `ToolStatus`

几个关键点：

1. `string_id!` 宏把几个重要 id 包成了独立类型，而不是到处裸传 `String`。  
这会让 session、turn、event 在语义上更明确，也更适合后面继续扩展校验。

2. `ToolStatus` 现在不是只有 `success` / `error`，还加了 `review_required`。  
这意味着工具返回值已经不只是“执行成功/失败”，而是开始承载 runtime policy 语义。

3. `ConversationMessage` 依然是 agent 给模型看的消息层。  
换句话说，它是“模型上下文格式”，不是“运行时事件格式”。这和 `RuntimeEvent` 是两套平行结构。

源码定位建议：

- `src/types.rs:7` 开始看 `string_id!`
- `src/types.rs:24` 开始看 `ToolCall`
- `src/types.rs:29` 开始看 `ToolStatus`
- `src/types.rs:45` 开始看 `ToolResult`
- `src/types.rs:79` 开始看 `ConversationMessage` 构造函数

## 3. Session 和 Event 分层

### `src/session.rs`

这个文件定义的是“当前状态”。

`SessionSnapshot` 不是完整历史，而是恢复运行所需的**当前快照**：

- `conversation`
- `open_exec_sessions`
- `pending_approvals`
- `workspace_root`
- `cwd`

注意它和事件日志的分工：

- `snapshot.json` 负责“现在是什么状态”
- `events.jsonl` 负责“之前发生了什么”

当前最值得看的结构：

- `ExecSessionRef`
- `PendingApproval`
- `SessionSnapshot`

特别注意：

`latest_compaction` 已经进了结构体，但 compaction 逻辑还没实现。这是一个典型的“为后续阶段留位”的字段。

源码定位建议：

- `src/session.rs:25` 开始看 `ExecSessionId`
- `src/session.rs:28` 开始看 `ExecSessionStatus`
- `src/session.rs:58` 开始看 `PendingApproval`
- `src/session.rs:67` 开始看 `SessionSnapshot`

### `src/events.rs`

这个文件定义的是“运行过程中发生的事实”。

最值得记住的是：  
`RuntimeEvent` 不等于 `ConversationMessage`。

`ConversationMessage` 是喂给模型的。  
`RuntimeEvent` 是给 runtime 自己审计、调试、回放的。

当前事件类型有：

- `AssistantTurn`
- `ToolResult`
- `ExecOutput`
- `ApprovalRequested`
- `ApprovalDecision`
- `CompactionWritten`
- `RuntimeError`

代码上最重要的点是 `RuntimeEventKind` 用了 `#[serde(tag = "type")]`，所以落盘后的 JSONL 很容易直接按事件类型读和过滤。

源码定位建议：

- `src/events.rs:13` 看 `RuntimeEvent`
- `src/events.rs:22` 看 `RuntimeEventKind`

## 4. 持久化工具层

### `src/transcript.rs`

这个文件名仍叫 transcript，但它现在已经不只是“对话转录”了，而是 session persistence helper。

它负责四类事：

1. `append_json_line`  
把 event 追加写入 `events.jsonl`

2. `write_json` / `read_json`  
读写 `snapshot.json`

3. `session_paths`  
把 `workspace_root + session_id` 解析成统一目录结构

4. `replay_session`  
把 `events.jsonl` 重新读成结构化 `RuntimeEvent`

这个文件看似简单，但它其实是 runtime 持久化边界的核心。  
因为上层都默认信任这里提供的路径和序列化格式。

值得特别注意的实现点：

- `new_session_id()` 使用时间戳 + 计数器，而不是 UUID
- `read_json_lines()` 对不存在文件返回空数组，这让 replay 和初始化逻辑更简单

源码定位建议：

- `src/transcript.rs:23` 看 `append_json_line`
- `src/transcript.rs:36` 看 `write_json`
- `src/transcript.rs:67` 看 `new_session_id`
- `src/transcript.rs:76` 看 `session_paths`
- `src/transcript.rs:93` 看 `replay_session`

## 5. Tool dispatch 层

### `src/registry.rs`

这个文件很短，但很关键。  
它定义了 `ToolContext` 和 `ToolRegistry`。

### `ToolContext` 为什么重要

`ToolContext` 现在已经不只是 config：

- `config`
- `session_id`
- `exec_sessions`
- `policy`

这代表一个重要架构变化：

工具不再是“纯函数 + config”，而是开始能接触 runtime services。

这也是为什么 Phase 2 的 `run_command` 已经具备：

- persistent exec 能力
- policy 判断能力
- approval event 写盘能力

因为这些能力都从 `ToolContext` 注入进去了。

### `ToolRegistry` 在做什么

它做两件事：

1. 暴露 schema 给 LLM
2. 根据 tool name 分发到具体工具

注意它仍然保持得很薄，没有加入 policy 或 risk logic。  
这是有意为之：policy 目前还是工具内部处理，而不是 registry 统一拦截。

源码定位建议：

- `src/registry.rs:11` 看 `ToolContext`
- `src/registry.rs:31` 看 `register`
- `src/registry.rs:38` 看 `schemas`
- `src/registry.rs:52` 看 `execute`

## 6. 主循环：`src/agent.rs`

这是当前 runtime 的中心文件。

建议阅读顺序：

1. `Agent` 字段
2. `run_with_meta`
3. `resume`
4. `run_session`
5. `apply_exec_session_update`
6. `apply_pending_approval_update`

### 第一步：先看 `Agent` 持有什么

`Agent` 里现在不只是：

- `config`
- `llm`
- `registry`

还多了：

- `exec_sessions`
- `policy`

这说明 `Agent` 已经从“单轮调度器”变成了“runtime 编排器”。

源码定位：

- `src/agent.rs:18`

### 第二步：看构造函数怎么分层

这里有三个构造方式：

- `new`
- `with_exec_sessions`
- `with_runtime`

真正的最终构造入口是 `with_runtime`。  
前两个只是给默认依赖注入留了更方便的入口。

这类写法的价值是：

- CLI/API 用默认 runtime service 很方便
- 测试时也可以替换共享依赖

源码定位：

- `src/agent.rs:33`
- `src/agent.rs:44`
- `src/agent.rs:59`

### 第三步：看 `run_with_meta`

`run_with_meta` 的职责非常清晰：

1. 创建初始 `SessionSnapshot`
2. 生成新的 `session_id`
3. 把第一条 user message 放进 conversation
4. 交给 `run_session`

这里的关键不是复杂逻辑，而是把“新 session 的初始化”独立出来了。

源码定位：

- `src/agent.rs:79`

### 第四步：看 `resume`

`resume` 的逻辑也故意做得很薄：

1. 根据 `session_id` 找 `snapshot.json`
2. 读出旧 snapshot
3. 把新 prompt 追加进 conversation
4. 再次进入 `run_session`

也就是说，resume 本身不是一个新循环，只是 `run_session` 的另一种入口。

源码定位：

- `src/agent.rs:93`

### 第五步：精读 `run_session`

这是整个 runtime 的主流程。

建议看代码时按这个顺序理解：

#### 5.1 预处理

- 拿到 `session_id`
- 算出 `snapshot_path` 和 `events_path`
- 先把 snapshot 落盘
- 从 snapshot 拿出 conversation 作为 `messages`
- 构造 `ToolContext`

源码定位：

- `src/agent.rs:102-123`

#### 5.2 每轮调用 LLM

每一轮里，先做：

- `llm.complete(...)`
- 生成新的 `turn_id`
- 立刻写 `AssistantTurn` event

这一点很重要：  
assistant turn 会先记事件，再继续执行 tool。

源码定位：

- `src/agent.rs:124-138`

#### 5.3 更新 conversation

如果 assistant 有文本或 tool call，就把它转成 `ConversationMessage::assistant(...)`：

- push 到内存 messages
- push 到 snapshot.conversation
- 回写 snapshot

这一步是模型上下文层的更新，不是 runtime event 层的更新。

源码定位：

- `src/agent.rs:140-148`

#### 5.4 如果没有 tool call，结束

assistant 一旦不再发 tool call，就认为 run 到终点，返回 `AgentRunOutput`。

源码定位：

- `src/agent.rs:150-157`

#### 5.5 执行每个 tool call

这里是最关键的阶段：

- `registry.execute(...)`
- 根据 `ToolResult.meta` 更新 snapshot
- 写 `ToolResult` event
- 构造成 `ConversationMessage::tool(...)`
- 继续下一轮 LLM

源码定位：

- `src/agent.rs:159-180`

### 第六步：看两个“反向同步”函数

#### `apply_exec_session_update`

这个函数说明了一个核心架构事实：

`run_command` 的某些 runtime 结果并不会直接改 snapshot，  
而是通过 `ToolResult.meta` 反向同步进 `SessionSnapshot`。

它会：

- 找 `exec_session_id`
- 清掉旧的同 id 记录
- 如果 lifecycle 还是 `running`，就重新写入 `open_exec_sessions`

源码定位：

- `src/agent.rs:191`

#### `apply_pending_approval_update`

它和上面完全同构，只不过同步的是 `pending_approvals`。

这里再次证明了当前架构的一个特点：  
**tool result meta 正在承担 runtime state delta 的角色。**

源码定位：

- `src/agent.rs:230`

## 7. 复杂度最高的工具：`src/tools/run_command.rs`

这是当前最值得慢读的文件。

因为它已经不是“跑个命令”这么简单，而是把四条分支揉在了一起：

1. 普通 one-shot command
2. persistent exec start
3. persistent exec poll / stdin / terminate
4. approval decision handling

### 先看入参结构

`RunCommandArgs` 里面这些字段的组合，基本决定了它走哪条分支：

- `command`
- `persistent`
- `exec_session_id`
- `stdin`
- `terminate`
- `approval_id`
- `decision`

源码定位：

- `src/tools/run_command.rs:21`

### 核心路由函数：`run_command`

读这一段时不要陷进细节，先看它的分发顺序：

1. 如果有 `approval_id`，进入审批决策路径
2. 如果有 `exec_session_id`，进入现有 persistent session 路径
3. 如果 `persistent=true`，启动新的 persistent session
4. 否则执行 one-shot command

源码定位：

- `src/tools/run_command.rs:89`

这个顺序本身就是这个工具的“状态机入口”。

### `start_persistent_command`

它做的事很直接：

- 校验 `command`
- 解析 `cwd`
- 从 `ToolContext` 取 `session_id`
- 先过 policy
- 再调用 `ExecSessionManager::start(...)`

源码定位：

- `src/tools/run_command.rs:115`

### `run_persistent_command`

这个函数只负责对一个已经存在的 exec session 做三种操作：

- `terminate`
- `stdin write`
- `poll`

你可以把它理解成一个很窄的“session control plane”。

源码定位：

- `src/tools/run_command.rs:220`

### `handle_approval_decision`

这是当前 runtime 最容易被忽略、但最关键的一段逻辑之一。

它在做什么：

1. 根据 `approval_id` 从 `PolicyManager` 取出待执行命令
2. 如果 `approved`
   - 先写 `ApprovalDecision` 事件
   - 再真正执行缓存下来的命令
3. 如果 `denied`
   - 写 `ApprovalDecision` 事件
   - 返回错误结果，不执行命令

也就是说，被拦截的命令并没有消失，而是被暂存起来等待后续 decision。

源码定位：

- `src/tools/run_command.rs:139`

### `maybe_require_approval`

这里是 policy hook 真正插入命令执行前的地方。

如果命中：

- `Allow`: 返回 `None`
- `Deny`: 直接返回 error outcome
- `ReviewRequired`: 创建 `PendingCommandApproval`，写 `ApprovalRequested` 事件，并返回 `review_required`

源码定位：

- `src/tools/run_command.rs:267`

### `run_one_shot_command`

这部分反而最接近 Phase 1：

- `sh -lc`
- timeout
- stdout / stderr 截断
- exit code 写进 meta

这里保留了最小 one-shot command 的实现，不让 persistent logic 污染它的主流程。

源码定位：

- `src/tools/run_command.rs:326`

### 为什么这个文件值得反复看

因为它把三个主题汇合到了一个点上：

- process execution
- session runtime
- policy boundary

你理解了这个文件，Phase 2 当前实现基本就吃透一半了。

## 8. Persistent process runtime：`src/exec_session.rs`

这个文件负责把“进程活着”这件事真正做出来。

### 先看数据结构

最重要的是两个结构：

- `ExecSessionManager`
- `ActiveExecSession`

`ExecSessionManager` 内部是：

- `session_id -> exec_session_id -> ActiveExecSession`

这表示 persistent exec session 被 runtime session 隔离了，不是全局平铺。

源码定位：

- `src/exec_session.rs:18`
- `src/exec_session.rs:34`

### `ActiveExecSession` 里有什么

- `child`
- `stdin`
- `state`
- `command`
- `cwd`
- `session_id`
- `exec_session_id`

其中 `state` 又维护：

- `stdout`
- `stderr`
- `status`
- `exit_code`

也就是说，当前版本把“进程控制句柄”和“用户可观察状态”分开了。

### `start`

它负责：

1. spawn 子进程
2. 拿出 stdin/stdout/stderr 管道
3. 构造 `ActiveExecSession`
4. 注册进 manager map
5. 启动两个后台读取任务
6. 返回第一次 snapshot

源码定位：

- `src/exec_session.rs:53`

### `write_stdin`

先校验这个 session 还在 `Running`，再往 stdin 写入并 flush。  
这一步很关键，因为它让命令真正跨多次 tool call 保持活性。

源码定位：

- `src/exec_session.rs:114`

### `poll`

它只是拿当前 snapshot。  
注意：poll 自己不会主动读 stdout/stderr；输出读取是后台任务在做。

源码定位：

- `src/exec_session.rs:145`

### `terminate`

它会：

- 尝试 kill 子进程
- 关闭 stdin
- 把状态标成 `Terminated`

源码定位：

- `src/exec_session.rs:154`

### `spawn_output_task`

这是这个文件真正最有 runtime 味道的部分。

它在后台循环读 stdout/stderr：

- 读 chunk
- 追加进内存 buffer
- 立刻落一个 `RuntimeEvent::ExecOutput`

这保证了两件事同时成立：

1. 用户后续 poll 时能看到累积 stdout/stderr
2. 事件日志里有完整流式痕迹

源码定位：

- `src/exec_session.rs:240`

## 9. Policy 核心：`src/policy.rs`

这个文件很小，但它定义了 Phase 2 最重要的安全边界。

### `PolicyMode`

当前支持：

- `Off`
- `Advisory`
- `Enforced`

不过当前实现里真正有行为变化的是 `Enforced`。  
`Advisory` 目前只是模式位，还没单独扩展更多行为。

源码定位：

- `src/policy.rs:15`

### `PolicyDecision`

这是运行时判断命令时最关键的三态：

- `Allow`
- `Deny`
- `ReviewRequired`

源码定位：

- `src/policy.rs:41`

### `PolicyManager`

当前做两件事：

1. `classify_command(...)`
2. 管理 `pending approvals`

它没有做复杂的 parser，也没有操作系统级隔离。  
当前版本只是一个最小、可验证的 policy boundary。

源码定位：

- `src/policy.rs:60`

### `classify_command`

它先查 hard deny pattern，再查 review required pattern。

也就是说：

- `rm -rf /` 这种直接 deny
- `rm -rf scratch` 这种进入 approval

源码定位：

- `src/policy.rs:65`

### `create_command_approval` / `take_pending_command`

这里说明了 approval 的内存模型：

- 风险命令先缓存
- 后续带 `approval_id` 再把它取出来执行或放弃

源码定位：

- `src/policy.rs:80`
- `src/policy.rs:108`

## 10. API 与 CLI 入口

### `src/api.rs`

这个文件的重点不是 HTTP 细节，而是 runtime service 的共享方式。

先看 `DefaultAgentRunner`：

- 内部持有 `Arc<ExecSessionManager>`
- 内部持有 `Arc<PolicyManager>`

这意味着 API server 不是“每个请求都新建一个完全隔离的 runtime 世界”，而是复用同一套 runtime manager。

源码定位：

- `src/api.rs:63`

然后看 `run(...)`：

- 建 config
- 建 LLM
- 用共享 exec/policy 构造 `Agent`
- 如果带 `session_id` 就走 `resume`

源码定位：

- `src/api.rs:77`

最后看 `run_agent(...)`：

它只是把 HTTP JSON 请求映射到 `AgentRunner::run(...)`，保持得很薄。

源码定位：

- `src/api.rs:139`

### `src/cli.rs`

CLI 逻辑更简单，纯粹做参数分发：

- `run`
- `resume`
- `api`

源码定位：

- `src/cli.rs:5`
- `src/cli.rs:12`

## 11. 四条最重要的源码路径

### 路径 A：新建 session 并完成一次普通运行

1. 入口进入 `Agent::run_with_meta`
2. 构造初始 `SessionSnapshot`
3. 进入 `run_session`
4. LLM 返回 `AssistantTurn`
5. assistant event 落盘
6. tool 执行
7. tool result 落盘
8. conversation 与 snapshot 更新
9. assistant 不再发 tool call 时返回

重点看：

- `src/agent.rs:79`
- `src/agent.rs:102`

### 路径 B：恢复旧 session

1. 入口进入 `Agent::resume`
2. 读取旧 `snapshot.json`
3. 追加一条新的 user message
4. 再进入同一个 `run_session`

重点看：

- `src/agent.rs:93`

### 路径 C：启动一个 persistent exec

1. `run_command` 看到 `persistent=true`
2. 进入 `start_persistent_command`
3. 先过 policy
4. 调 `ExecSessionManager::start`
5. spawn 后台读 stdout/stderr
6. 返回 `exec_session_id`
7. `Agent` 再把这个信息同步进 snapshot

重点看：

- `src/tools/run_command.rs:98`
- `src/tools/run_command.rs:115`
- `src/exec_session.rs:53`
- `src/agent.rs:191`

### 路径 D：危险命令进入 approval

1. `run_command` 调 `maybe_require_approval`
2. `PolicyManager::classify_command` 返回 `ReviewRequired`
3. 创建 `PendingCommandApproval`
4. 写 `ApprovalRequested` event
5. 返回 `ToolStatus::ReviewRequired`
6. `Agent` 再把 pending approval 同步进 snapshot
7. 后续用 `approval_id + decision` 再进入 `handle_approval_decision`

重点看：

- `src/tools/run_command.rs:267`
- `src/policy.rs:65`
- `src/tools/run_command.rs:139`
- `src/agent.rs:230`

## 12. 读这套代码时最容易忽略的点

### 点 1：状态更新不是只在一个地方发生

当前 runtime state 的更新分成三类：

- tool 内部自己处理实际行为
- `events.jsonl` 写运行时事件
- `Agent` 再从 `ToolResult.meta` 反向同步 snapshot

这不是最纯粹的架构，但它很务实。

### 点 2：persistent exec 的“实时输出”不是靠 poll 拿到的

真正的 stdout/stderr 收集发生在后台任务里，poll 只是读取当前累积结果。

### 点 3：approval 是“缓存命令后再决定”

不是简单地“弹个提示”，而是真的把未来要执行的命令对象暂存到了 `PolicyManager` 里。

### 点 4：API 层的 runtime service 是共享的

这会影响你后面设计更复杂 server runtime 时的思路，因为 manager 生命周期已经开始超出单个 request 了。

## 13. 当前实现的几个工程妥协

### 妥协 1

`run_command.rs` 太重了。  
它现在是状态机入口、policy 网关、process tool、approval control plane 的混合体。

### 妥协 2

approval 事件 id 和 exec 输出事件 id 用的是各自独立计数器，当前没有和 `Agent` 的 `evt_<n>` 序列统一。

### 妥协 3

`PolicyMode::Advisory` 目前只是模式占位，没有形成完整行为闭环。

### 妥协 4

snapshot 的更新依赖 `ToolResult.meta` 约定字段，这比显式 state delta 类型更轻，但也更脆弱。

## 14. 复盘时建议问自己的问题

读完这套代码后，建议你检查自己能不能回答下面这些问题：

1. 为什么 Phase 2 需要 snapshot 和 event log 两套持久化？
2. 为什么 persistent exec manager 要按 `session_id` 分桶？
3. 为什么 approval 是先返回 `review_required`，而不是直接在 registry 层阻断？
4. 为什么 `Agent` 需要 `apply_exec_session_update` 和 `apply_pending_approval_update` 这两个同步函数？
5. 如果下一步做 compaction，最自然接入的位置会在 `agent.rs` 的哪里？

如果这五个问题你都能讲清楚，说明当前 Phase 2 的骨架已经真正进入你的脑子里了。

## 15. 一句话总结

这一版源码最核心的价值，不是“加了很多能力”，而是把 runtime 的几个关键边界第一次拆清楚了：  
模型上下文、当前状态、历史事件、长生命周期进程、以及危险命令的审批边界，终于不再混成一个 demo 级循环。
