# ExAgent Phase 2 Runtime Interview Summary

**Date:** 2026-04-15  
**Audience:** 面试答题、项目复盘、口头汇报  
**Status:** 基于当前 `codex/phase2-p0-runtime` worktree 的真实实现

## 1. 怎么使用这份文档

这不是完整设计稿，也不是逐行源码注释。  
它是“面试答题卡”版本：

- 每个问题都尽量压成 30-90 秒可讲清的一段话
- 每题后面补“关键词”
- 再补“可能追问”

建议使用方式：

1. 先把“标准答法”读顺
2. 再把“关键词”变成你自己的说法
3. 最后只记结构，不背原文

## 2. 高频问题 1

### 问：Phase 2 主要解决了什么问题？

**标准答法**

Phase 1 的 ExAgent 已经有最小 agent loop，但更像一次性 demo。  
Phase 2 的目标是把它推进成一个可持续运行的 runtime substrate。当前已经落地的核心能力有五个：

- session 持久化
- resume 旧 session
- persistent exec session
- policy / approval hook
- event-based replay

所以可以把 Phase 2 理解成：从“单次对话 + 一次性工具调用”，升级到“带状态、可恢复、可审计、能拦截风险操作”的运行时底座。

**关键词**

- durable runtime
- session persistence
- resume
- persistent process
- approval boundary
- replayable events

**可能追问**

- 现在还没做什么
- 和 Phase 1 最本质的差别是什么

## 3. 高频问题 2

### 问：为什么需要同时有 snapshot 和 event log？

**标准答法**

因为它们解决的是两个不同问题。

`snapshot.json` 表示“当前状态”，用于快速恢复运行；  
`events.jsonl` 表示“发生过什么”，用于调试、审计和回放。

如果只有事件日志，resume 时就要从头重建上下文，成本高而且逻辑更复杂。  
如果只有 snapshot，又丢失了运行过程，没法解释 agent 当时是怎么走到当前状态的。

所以当前设计把两者并存：

- snapshot 优先服务恢复
- event log 优先服务可观测性

这是 Phase 2 里最重要的结构性判断之一。

**关键词**

- current state vs historical trace
- fast resume
- replayability
- auditability

**可能追问**

- snapshot 里具体存什么
- event log 里具体存什么

## 4. 高频问题 3

### 问：当前 session 模型长什么样？

**标准答法**

当前用 `SessionSnapshot` 表示运行时的最小可恢复状态。  
里面最关键的字段有：

- `session_id`
- `workspace_root`
- `cwd`
- `conversation`
- `open_exec_sessions`
- `pending_approvals`

其中 `conversation` 负责恢复模型上下文，`open_exec_sessions` 负责恢复还活着的进程引用状态，`pending_approvals` 负责继续未决的危险操作。

这说明 session 在当前实现里已经不只是“对话历史”，而是把 process state 和 approval state 一起纳入了 runtime snapshot。

**关键词**

- SessionSnapshot
- conversation
- open_exec_sessions
- pending_approvals

**可能追问**

- 为什么 `latest_compaction` 已经有字段但没实现

## 5. 高频问题 4

### 问：当前 runtime 的主循环是怎么工作的？

**标准答法**

主循环在 `Agent::run_session` 里。整体流程是：

1. 先创建或加载 session snapshot
2. 从 snapshot 取出 conversation，作为模型输入
3. 调 LLM 产出 `AssistantTurn`
4. 先把 assistant turn 写成 runtime event
5. 如果有 tool call，就通过 registry 分发执行
6. 再把 tool result 写成 runtime event
7. 用 tool result 的 metadata 反向更新 snapshot
8. 继续下一轮，直到 assistant 不再发 tool call

也就是说，主循环现在不只是“调模型”，而是在编排三类东西：

- 模型上下文
- runtime state
- runtime events

**关键词**

- Agent::run_session
- AssistantTurn
- ToolResult
- event first
- snapshot sync

**可能追问**

- 为什么 assistant turn 要先写 event 再执行 tool
- 为什么 snapshot 更新不直接在 tool 内部做

## 6. 高频问题 5

### 问：persistent exec session 为什么重要？

**标准答法**

因为 coding agent 不能只依赖 one-shot command。  
真实工程场景里，经常需要一个进程跨多轮继续活着，比如交互式 shell、REPL、长时间测试进程、持续输出日志的后台任务。

所以当前实现引入了 `ExecSessionManager`，按 `session_id -> exec_session_id` 维护活跃子进程，并支持四种基本操作：

- start
- poll
- write stdin
- terminate

这让 agent 从“一次性执行命令”升级到“持有运行中的进程状态”。  
这一步是 runtime 从 demo 走向真实 coding workflow 的关键。

**关键词**

- persistent process
- ExecSessionManager
- poll
- stdin write
- terminate

**可能追问**

- 为什么要按 `session_id` 分桶
- stdout/stderr 是怎么收集的

## 7. 高频问题 6

### 问：persistent exec 的输出是怎么处理的？

**标准答法**

当前不是靠 poll 主动去读输出，而是启动后台任务持续读取 stdout/stderr。

每个输出 chunk 会做两件事：

1. 追加到内存里的 stdout/stderr buffer
2. 立刻写成 `RuntimeEvent::ExecOutput`

这样做的好处是：

- 后续 poll 能拿到累计输出
- event log 里保留了流式过程

所以当前设计同时满足“当前状态可读”和“过程可回放”。

**关键词**

- background reader
- buffered output
- RuntimeEvent::ExecOutput
- stream capture

**可能追问**

- 为什么不只保存最终输出
- 这样会不会导致 event 太多

## 8. 高频问题 7

### 问：policy / approval hook 是怎么设计的？

**标准答法**

当前的 policy 目标不是完整 sandbox，而是先建立一个可审计的审批边界。  
也就是说，在真正做 OS 级隔离之前，先把“哪些命令应该直接执行，哪些应该拦下审批”这条边界做出来。

当前支持三种 policy decision：

- `Allow`
- `Deny`
- `ReviewRequired`

如果命中 risky pattern，就不会直接执行，而是：

1. 创建 `PendingCommandApproval`
2. 写 `ApprovalRequested` event
3. 返回 `review_required` 的 tool result
4. 等后续通过 `approval_id + decision` 再继续

这保证了危险操作有显式审批流，而且决策过程可以落到 event log 里。

**关键词**

- approval before sandbox
- PolicyDecision
- PendingCommandApproval
- ApprovalRequested
- ApprovalDecision

**可能追问**

- 为什么不直接在 registry 层拦截
- 风险命令是怎么识别的

## 9. 高频问题 8

### 问：为什么危险命令不是直接失败，而是进入 review_required？

**标准答法**

因为 runtime 需要区分三种不同语义：

- 命令执行成功
- 命令执行失败
- 命令本身还没被允许执行

第三种语义如果也塞进普通 error，会把“系统出错”和“等待审批”混在一起。  
所以现在把它提升成 `ToolStatus::ReviewRequired`，让 agent 和上层调用方都知道：

这不是执行失败，而是 runtime 主动进入人工决策阶段。

这个区分非常重要，因为它决定了后续流程是“修 bug”还是“等待批准”。

**关键词**

- explicit runtime state
- review_required
- error vs pending approval

**可能追问**

- 模型收到 review_required 之后会怎么处理

## 10. 高频问题 9

### 问：approval 是怎么恢复执行的？

**标准答法**

危险命令被拦下时，并没有丢掉，而是被缓存成 `PendingCommandApproval`。  
后续如果外部传入：

- `approval_id`
- `decision = approved | denied`

那么 `run_command` 会走 `handle_approval_decision`：

- 如果 approved，就把之前缓存的命令真正执行掉
- 如果 denied，就写 `ApprovalDecision` event，然后返回拒绝结果

这个设计的关键点是：  
审批不是“重新描述一遍命令再执行”，而是“对先前缓存的命令对象做决定”。

这样更一致，也更可审计。

**关键词**

- approval_id
- cached command
- handle_approval_decision
- deterministic resume

**可能追问**

- 批准之后 one-shot 和 persistent command 有什么不同

## 11. 高频问题 10

### 问：为什么 `run_command.rs` 会这么复杂？

**标准答法**

因为 Phase 2 当前刻意把多个 runtime path 收敛到一个高杠杆工具上，而不是过早拆出很多 overlapping tools。

现在它同时承载：

- one-shot command
- persistent exec start
- persistent exec control
- policy decision
- approval flow

好处是：

- 行为集中
- 调试路径更短
- 对 agent 来说接口更统一

代价是：

- 单文件复杂度上升
- 分支逻辑越来越像状态机入口

所以当前这是一个务实的 P0 取舍，不一定是最终形态。

**关键词**

- high-leverage tool
- narrow interface
- centralized execution path
- future refactor candidate

**可能追问**

- 后续最可能怎么拆

## 12. 高频问题 11

### 问：API 和 CLI 在当前架构里分别扮演什么角色？

**标准答法**

CLI 主要是本地开发入口，支持：

- 普通 run
- resume 旧 session
- 启动 API server

API 则是最小外部驱动面，当前只提供：

- `GET /health`
- `POST /run`

但它的重要性不在于“做产品接口”，而在于让 runtime 更容易被外部系统驱动，比如：

- 未来 UI
- eval harness
- 自动化测试

另外，API 层的 `DefaultAgentRunner` 会共享 `ExecSessionManager` 和 `PolicyManager`，说明 runtime service 生命周期已经开始超出单个请求。

**关键词**

- thin entrypoint
- resume via API
- shared runtime services
- future integration point

**可能追问**

- 为什么 API 层要共享 manager

## 13. 高频问题 12

### 问：当前实现最重要的工程权衡是什么？

**标准答法**

我觉得有四个最重要的权衡：

1. 先做 approval boundary，再做完整 sandbox  
这是风险控制上的渐进式路线。

2. snapshot 和 event log 并存  
这是恢复效率和可观测性之间的平衡。

3. `run_command` 先集中承载复杂度  
这是接口统一和代码纯净之间的平衡。

4. 用 `ToolResult.meta` 反向更新 snapshot  
这是轻量实现和强类型 state delta 之间的平衡。

所以当前版本最强的地方不是“完美架构”，而是 runtime primitive 已经足够清晰，后续知道该往哪里继续拆。

**关键词**

- engineering tradeoff
- approval before sandbox
- snapshot vs event log
- meta-driven sync
- pragmatic P0

**可能追问**

- 你下一步最想重构哪里

## 14. 高频问题 13

### 问：这套 runtime 目前还缺什么？

**标准答法**

当前最明显还没完成的是三块：

1. context compaction  
目前只是有字段，没有真正压缩上下文的流程。

2. eval harness  
现在已经有 replay 和测试，但还没有固定场景回归体系。

3. 更强的 policy / sandbox  
目前主要还是基于字符串规则，不是完整系统隔离。

换句话说，Phase 2 已经把底座骨架搭起来了，但长期运行能力和安全边界还需要下一阶段继续加强。

**关键词**

- compaction
- eval harness
- stronger sandbox

**可能追问**

- 你会先做 compaction 还是 eval

## 15. 高频问题 14

### 问：如果让你用一句话总结当前 Phase 2，你会怎么说？

**标准答法**

我会说：  
Phase 2 把 ExAgent 从“单轮 agent loop”推进成了“带 session、event log、resume、persistent process 和 approval boundary 的 runtime substrate”，虽然还不完整，但已经从 demo 进入了可持续演化的工程形态。

**关键词**

- runtime substrate
- session-aware
- replayable
- approval-aware
- durable

## 16. 一页速记版

如果你临近面试，只想背最短版本，可以记这 8 句：

1. Phase 2 的本质是把 agent loop 升级成 durable runtime。
2. snapshot 解决恢复，event log 解决审计和回放。
3. session 现在不仅存 conversation，也存 exec session 和 pending approval。
4. 主循环在 `Agent::run_session`，负责把 LLM、tool、snapshot、events 串起来。
5. persistent exec 是 coding runtime 可信度的关键能力。
6. `run_command` 是当前最核心的高杠杆工具入口。
7. policy 先做 approval boundary，再考虑更强 sandbox。
8. 当前最缺的是 compaction、eval harness 和更强安全边界。

## 17. 最后提醒

真正回答时不要像背稿。  
最好的讲法是：

- 先讲目标
- 再讲结构
- 再讲取舍
- 最后讲没做完的地方

只要你能把这四层讲顺，听起来就会很像你真的做过这套 runtime，而不是只看过文档。
