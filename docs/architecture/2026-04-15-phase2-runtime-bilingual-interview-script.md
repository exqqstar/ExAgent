# ExAgent Phase 2 Runtime Bilingual Interview Script

**Date:** 2026-04-15  
**Audience:** 面试口述、英文自我表达、项目复盘  
**Status:** 严格基于当前 `codex/phase2-p0-runtime` 的真实实现

## 1. 使用方式

这份稿子不是逐字死背稿，而是口述模板。

建议分三层准备：

1. 先背 `30 秒版`
2. 再熟悉 `90 秒版`
3. 最后根据面试官深挖方向切到 `3 分钟版` 和 `Q&A`

答题原则：

- 不夸大未完成功能
- 明确“已完成”和“下一步”
- 多用 trade-off 语言，而不是只列功能

## 2. 30 秒版

### 中文

我做的这个项目本质上是在把一个最小 agent loop 升级成一个可持续运行的 coding-agent runtime。  
Phase 2 里我重点做了五件事：session 持久化、resume 恢复、persistent exec session、policy/approval hook，以及 event-based replay。  
所以它不再只是一次性调用模型和工具，而是开始具备状态、审计能力和风险控制边界。

### English

This project is essentially about evolving a minimal agent loop into a durable coding-agent runtime.  
In Phase 2, I focused on five core capabilities: session persistence, resume support, persistent exec sessions, a policy and approval hook, and event-based replay.  
So instead of being a one-shot model-and-tool demo, it now has state, auditability, and an explicit risk-control boundary.

## 3. 90 秒版

### 中文

Phase 1 已经证明了最小闭环，也就是 user、LLM、tool call、tool result、next turn 这一套流程能跑通。  
但那个版本更像 demo，因为每次运行都是一次性的，命令不能跨轮存活，也没有真正的 session 概念。

Phase 2 的目标，是把它推进成 runtime substrate。当前已经落地的核心能力有：

- 用 `SessionSnapshot` 持久化当前状态
- 用 `RuntimeEvent` 记录运行过程
- 支持 `resume(session_id, prompt)` 恢复旧会话
- 支持 persistent process 的 start、poll、stdin write 和 terminate
- 对危险命令引入 `review_required` 审批流

这套设计里一个很关键的点是，我把 snapshot 和 event log 分开了。  
snapshot 解决的是快速恢复，event log 解决的是调试、审计和 replay。  
这样系统不只是“能继续跑”，而且“能解释之前发生了什么”。

### English

Phase 1 proved that the minimal loop worked: user input, LLM completion, tool call, tool result, and then the next turn.  
But that version was still closer to a demo, because each run was one-shot, commands could not survive across turns, and there was no real session model.

The goal of Phase 2 was to move it toward a runtime substrate. The main capabilities I implemented were:

- persisting current state with `SessionSnapshot`
- recording execution history with `RuntimeEvent`
- supporting `resume(session_id, prompt)`
- supporting persistent processes with start, poll, stdin write, and terminate
- adding a `review_required` approval path for risky commands

One key design decision was separating snapshot state from the event log.  
The snapshot is for fast recovery, while the event log is for debugging, auditing, and replay.  
That means the system can not only continue execution, but also explain how it got there.

## 4. 3 分钟版

### 中文

我会把这个项目描述成一个 runtime-oriented agent 演进过程。

一开始系统只有最小 agent loop，也就是把 conversation 发给模型，模型决定要不要调用工具，工具执行完以后再把结果喂回模型。这个闭环足够证明方向，但不适合真实 coding workflow，因为它缺少三个关键能力：第一，没有 durable session；第二，没有长生命周期进程；第三，没有对高风险命令的安全边界。

所以在 Phase 2 里，我做的核心不是“增加很多工具”，而是先把 runtime primitive 搭出来。

第一个 primitive 是 session model。我引入了 `SessionSnapshot` 来存当前状态，包括 conversation、open exec sessions 和 pending approvals。与此同时，我引入了 `RuntimeEvent` 来记录 assistant turn、tool result、exec output 以及 approval 相关事件。这样 snapshot 和 event log 分工明确，前者解决恢复，后者解决可观测性。

第二个 primitive 是 resume。现在 agent 可以通过 `session_id` 找到 `snapshot.json`，把新的 user prompt 接到原来的 conversation 后面，再继续跑主循环。这个能力同时暴露到了 CLI 和 HTTP API，所以后续接 UI 或 eval harness 都更容易。

第三个 primitive 是 persistent exec session。我加了 `ExecSessionManager`，让命令不只是 one-shot execution，而是可以 start、poll、write stdin 和 terminate。stdout 和 stderr 会被后台任务持续读取，并写成 `RuntimeEvent::ExecOutput`，所以系统既能看到当前缓冲输出，也能回放流式过程。

第四个 primitive 是 policy and approval hook。当前实现还不是完整 sandbox，但已经建立了一个明确的 runtime boundary。危险命令不会直接执行，而是返回 `review_required`，创建 pending approval，写 `ApprovalRequested` event，后续再通过 `approval_id + decision` 决定执行还是拒绝。这样风险命令的控制流程是显式和可审计的。

如果从工程取舍来看，我最看重的判断有两个。第一，approval boundary 应该先于完整 sandbox 落地；第二，snapshot 和 event log 必须并存，因为恢复状态和解释过程本来就是两个问题。

当然，当前版本也有明确边界。比如 compaction 还没有真正实现，policy 仍然是基于简单规则匹配，`run_command.rs` 也承担了比较多分支逻辑。但我认为这套 Phase 2 已经把系统从 demo 推进到了可以持续扩展的 runtime 底座。

### English

I would describe this project as an evolution from a minimal agent loop into a runtime-oriented agent system.

Initially, the system only had the basic loop: send the conversation to the model, let the model decide whether to call a tool, execute the tool, and feed the result back into the next turn. That was enough to validate the direction, but it was not sufficient for a real coding workflow, because it lacked three things: a durable session model, long-lived processes, and a safety boundary for risky commands.

So in Phase 2, my main focus was not to add many tools. It was to build the core runtime primitives first.

The first primitive was the session model. I introduced `SessionSnapshot` to persist current state, including conversation history, open exec sessions, and pending approvals. In parallel, I introduced `RuntimeEvent` to record assistant turns, tool results, exec output, and approval-related events. That created a clear split: the snapshot is for recovery, and the event log is for observability.

The second primitive was resume support. The agent can now use a `session_id` to load `snapshot.json`, append a new user prompt to the previous conversation, and continue the same runtime loop. I exposed that through both the CLI and the HTTP API, which also makes future UI or evaluation tooling easier.

The third primitive was persistent exec sessions. I added an `ExecSessionManager`, so commands are no longer only one-shot executions. They can now be started, polled, written to through stdin, and terminated. Stdout and stderr are continuously read by background tasks and appended as `RuntimeEvent::ExecOutput`, so the runtime can both inspect the current buffered state and replay the streaming process.

The fourth primitive was the policy and approval hook. This is not a full sandbox yet, but it already establishes an explicit runtime boundary. Risky commands do not execute immediately. Instead, they return `review_required`, create a pending approval object, write an `ApprovalRequested` event, and later continue only if an `approval_id` is explicitly approved. That makes risky actions both controlled and auditable.

From an engineering trade-off perspective, I think two decisions matter most. First, an approval boundary should land before a full sandbox. Second, snapshot state and event history need to coexist, because restoring current state and explaining historical execution are fundamentally different problems.

There are still clear limitations. Compaction is not implemented yet, the current policy is still rule-based rather than sandbox-based, and `run_command.rs` carries a lot of branching logic. But I think Phase 2 successfully moves the system from a demo into a runtime foundation that can keep evolving.

## 5. 项目亮点答法

### 中文

如果面试官问“你觉得这个项目最亮的点是什么”，我会强调两点：

第一，我没有一开始就堆功能，而是先把 runtime primitive 搭出来。  
第二，我把“恢复状态”和“解释过程”拆成了 snapshot 和 event log 两条持久化路径，这让系统既能继续工作，也能被调试和审计。

### English

If the interviewer asks what I think is the strongest part of this project, I would emphasize two things.

First, I did not start by adding many features. I started by building the runtime primitives first.  
Second, I separated “restoring state” from “explaining execution” by introducing both a snapshot and an event log. That makes the system both durable and inspectable.

## 6. 高频追问 Q&A

### Q1. 你为什么不直接做 sandbox，而是先做 approval？

**中文**

因为 approval boundary 更轻量，也更容易先验证正确性。  
在 Phase 2 这个阶段，我更关心的是让 runtime 先具备明确的风险控制流程和审计能力，而不是一步到位做完整隔离。approval 先落地，能更快建立安全边界，也给后续 sandbox 留出更清晰的接口。

**English**

Because an approval boundary is lighter-weight and easier to validate first.  
At this stage, I cared more about giving the runtime an explicit risk-control flow and audit trail than immediately building full isolation. Approval gives us a clear boundary early, and it also creates a cleaner interface for a stronger sandbox later.

### Q2. 为什么 `run_command` 要同时处理这么多逻辑？

**中文**

这是一个 P0 阶段的刻意取舍。  
我希望 agent-facing interface 先保持窄而统一，所以把 one-shot execution、persistent exec 和 approval decision 都收敛到了一个高杠杆工具里。这样做的好处是接口简单、调试路径短；代价是单文件复杂度更高，后续很可能会继续拆分。

**English**

That was a deliberate P0 trade-off.  
I wanted the agent-facing interface to stay narrow and consistent, so I concentrated one-shot execution, persistent exec, and approval decisions into a single high-leverage tool. The advantage is a simpler interface and a shorter debugging path. The downside is that the file becomes more complex, and it is a likely refactor target later.

### Q3. persistent exec 和普通 one-shot command 的本质区别是什么？

**中文**

本质区别在于 one-shot command 只关心一次结果，而 persistent exec 关心进程生命周期。  
one-shot command 更像 RPC；persistent exec 更像 runtime-managed process。后者需要保存状态、支持 stdin 写入、支持轮询当前输出，还要把流式 stdout/stderr 记录进事件日志。

**English**

The core difference is that a one-shot command only cares about a single result, while persistent exec cares about process lifecycle.  
One-shot execution is closer to an RPC call; persistent exec is closer to a runtime-managed process. The latter needs state, stdin support, polling, and streaming stdout/stderr events.

### Q4. 这个项目里你最满意的一个设计判断是什么？

**中文**

我最满意的是把 snapshot 和 event log 分开。  
很多系统一开始只保留一种持久化形式，后面会发现恢复和调试需求互相打架。现在这两个概念在架构上就是独立的，所以后续做 compaction、eval 或可视化调试都会更顺。

**English**

The design decision I’m most satisfied with is separating snapshot state from the event log.  
Many systems start with only one persistence model and later discover that recovery and debugging pull the design in different directions. By separating them early, future work like compaction, evaluation, and visualization becomes much cleaner.

### Q5. 当前最大的不足是什么？

**中文**

当前最大不足有三个：compaction 还没落地、policy 还主要靠规则匹配、`run_command` 的复杂度偏高。  
换句话说，这一版已经把 runtime 的骨架做出来了，但长 session 管理、系统级安全、以及更优雅的边界拆分还需要下一阶段继续推进。

**English**

The biggest limitations right now are threefold: compaction is not implemented yet, policy is still mostly rule-based, and `run_command` carries too much complexity.  
In other words, the runtime skeleton is now in place, but long-session management, stronger isolation, and cleaner boundary decomposition still belong to the next phase.

## 7. 结束语模板

### 中文

如果让我总结这一阶段，我会说：我做的不是单纯加功能，而是先把一个最小 agent loop 推进成了一个有状态、可恢复、可审计、并且带风险控制边界的 runtime 底座。

### English

If I had to summarize this phase in one sentence, I would say that I did not just add features. I turned a minimal agent loop into a runtime foundation that is stateful, resumable, auditable, and equipped with an explicit risk-control boundary.

## 8. 最后提醒

口述时尽量避免这两种问题：

- 只报功能名，不讲 trade-off
- 把未完成的 compaction 或 stronger sandbox 说成已经做了

最稳的表达结构是：

1. 先说问题
2. 再说我做了哪些 runtime primitive
3. 再说设计权衡
4. 最后说还没做完的部分
