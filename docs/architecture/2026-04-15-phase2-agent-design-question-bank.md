# ExAgent Phase 2 Agent Design Question Bank

**Date:** 2026-04-15  
**Audience:** Agent 设计复盘、面试准备、架构自测  
**Status:** 基于当前 `codex/phase2-p0-runtime` worktree 的真实实现整理

## 1. 这份题库的定位

这份题库只聚焦一个主题：  
当前 Phase 2 里，我们到底把 `Agent` 设计成了什么，它的边界、职责、状态流转和扩展方式分别是什么。

和总题库的区别是：

- 总题库覆盖整个 runtime
- 这份题库只盯 `Agent` 本体
- 更适合你单独准备“Agent 设计”这一类面试问题

建议刷题方式：

1. 先刷“必答 Top 12”
2. 再刷主循环、状态、工具编排三块
3. 最后刷取舍、失败场景和未来演进

## 2. 必答 Top 12

1. 在当前 Phase 2 里，`Agent` 的职责到底是什么？
2. 这个 `Agent` 和一个“只会调 LLM 的聊天循环”本质区别是什么？
3. 为什么说当前 `Agent` 已经是 runtime orchestrator，而不只是 prompt wrapper？
4. `Agent::run_session` 的核心控制流是什么？
5. 为什么当前设计把 session、event、tool execution 都放进 `Agent` 的主循环里编排？
6. `Agent` 持有的“当前状态”和“外部能力”分别有哪些？
7. 为什么 assistant turn 要先记录 event，再执行 tool？
8. 为什么 tool result 不只返回文本，还要返回结构化 metadata？
9. 为什么 `Agent` 要负责推进 snapshot，而不是把状态更新全部下沉到 tool？
10. 当前 `Agent` 是如何支持 resume 旧 session 的？
11. 当前 `Agent` 是如何和 policy / approval 协同工作的？
12. 如果继续做下一阶段，你最想先改 `Agent` 的哪一部分？

## 3. Agent 的角色与边界

1. 当前系统里，`Agent` 和 `TranscriptStore` 的职责边界是什么？
2. 当前系统里，`Agent` 和 `ExecSessionManager` 的职责边界是什么？
3. 当前系统里，`Agent` 和 `PolicyManager` 的职责边界是什么？
4. 为什么 `Agent` 不应该自己直接承担所有持久化细节？
5. 为什么 `Agent` 也不应该退化成一个只负责“调用别的模块”的薄壳？
6. 你会如何一句话定义当前 `Agent` 的角色？
7. 当前 `Agent` 更像 controller、orchestrator、state machine，还是 workflow engine？为什么？
8. 当前实现里，`Agent` 的边界有没有还不够清晰的地方？
9. 哪些能力必须在 `Agent` 层统一编排，哪些能力可以继续模块化下沉？
10. 如果以后要支持更多工具，`Agent` 的边界应该如何保持稳定？

## 4. 主循环设计

1. `Agent::run_session` 每一轮做了哪些步骤？
2. 哪一步负责把 conversation 交给模型？
3. 哪一步负责把 assistant turn 写入 event log？
4. 哪一步负责工具分发？
5. 哪一步负责把 tool result 反映回 snapshot？
6. 当前 loop 的退出条件是什么？
7. 为什么这条 loop 既要推进“对话”，又要推进“runtime state”？
8. 为什么这条 loop 不是单纯的 request-response，而是一个持续推进的控制环？
9. 为什么 assistant turn 不能只留在内存里？
10. 如果某次 tool call 失败，当前主循环如何维持最小可恢复性？
11. 为什么这里的顺序设计对调试和审计很重要？
12. 如果以后要支持更复杂的 tool chaining，这条 loop 最可能先在哪些地方变复杂？

## 5. 状态建模

1. 当前 `Agent` 真正依赖的核心状态有哪些？
2. 哪些状态属于 session snapshot？
3. 哪些状态属于 event trace？
4. 哪些状态属于运行中的外部资源，比如 exec process？
5. 为什么 conversation 既是模型上下文，也是 runtime state 的一部分？
6. 为什么 open exec sessions 要进入 snapshot？
7. 为什么 pending approvals 也要进入 snapshot？
8. 当前 `Agent` 设计里，“当前状态”和“历史轨迹”为什么必须分离？
9. 如果只保留 conversation，不保留其他状态，会丢掉什么？
10. 当前状态模型里，最关键的数据一致性风险是什么？
11. 这套状态设计更偏向 stateful agent，还是 event-driven agent？为什么？
12. 如果未来加入 memory compaction，`Agent` 的状态模型会怎么变？

## 6. 工具编排与动作执行

1. 当前 `Agent` 是怎么决定“调用模型”还是“执行工具”的？
2. tool registry 在 `Agent` 设计里承担什么角色？
3. 为什么工具执行结果需要统一回到 `Agent` 主循环，而不是让工具直接改全局状态？
4. `ToolResult` 里的 metadata 对 `Agent` 有什么价值？
5. 为什么持久化 exec session 要被视为 `Agent` 能力的一部分，而不是一个普通工具？
6. 当前 `Agent` 如何处理 one-shot command 和 persistent exec 这两种执行模式？
7. 为什么说 `Agent` 已经开始管理 process state，而不只是“发命令”？
8. 如果某个工具未来需要多步交互，当前 `Agent` 结构还能不能承接？
9. 现在的工具编排方式更偏“函数调用”，还是更偏“动作系统”？为什么？
10. 如果你要支持外部插件工具，当前 `Agent` 设计还缺什么？

## 7. Resume 与持久化协同

1. 为什么 resume 是 `Agent` 设计里的核心能力，而不是外围功能？
2. 当前 `Agent` 恢复旧 session 时，需要重新组装哪些状态？
3. 为什么“恢复一个 session”不等于“把旧消息重新塞给模型”？
4. `snapshot.json` 和 `events.jsonl` 分别怎样帮助 `Agent` 恢复执行？
5. 当前 `Agent` 如何知道一个 session 之前有哪些 open exec sessions？
6. 当前 `Agent` 如何继续处理 pending approvals？
7. 恢复路径里，哪些状态是必须完整恢复的，哪些状态可以容忍退化？
8. 如果 session 恢复时发现某个外部进程已经不存在，`Agent` 应该怎么理解这种不一致？
9. 如果 event log 和 snapshot 内容不一致，`Agent` 应该优先相信谁？为什么？
10. 这套 resume 设计体现了 `Agent` 的哪种工程取向？

## 8. Approval、Policy 与安全边界

1. 为什么 `Agent` 设计里必须考虑风险动作的审批边界？
2. 当前 `Agent` 和 `PolicyManager` 是怎样协作的？
3. 为什么 approval 决策不能只停留在 UI 层？
4. 为什么说 approval 是 runtime state，而不只是交互状态？
5. 当前 `Agent` 如何面对“命令还没执行，但 approval 也还没回来”的状态？
6. 为什么 pending approvals 要进入 snapshot？
7. 如果 resume 之后继续执行危险命令，`Agent` 应该延续什么历史信息？
8. 当前这套安全边界更像“最小可用治理”，还是“成熟权限系统”？为什么？
9. 如果以后要对接更细粒度的 sandbox，`Agent` 设计里最可能受影响的是什么？
10. 当前设计在安全性上最大的简化点是什么？

## 9. 并发、所有权与生命周期视角下的 Agent 设计

1. 从 Rust 的角度看，当前 `Agent` 设计里最重要的共享状态是什么？
2. 为什么 `Agent` 不能把所有状态都设计成短期借用？
3. 为什么跨 `.await`、跨 task 的状态通常要提升成 owned data 或 `Arc`？
4. 当前 `Agent` 为什么更适合和 `Arc<Mutex<...>>` 的 manager 协作？
5. 如果强行用引用把 manager 和 session state 串起来，最容易在哪出问题？
6. 为什么这里的生命周期设计会反过来影响模块边界？
7. 当前 `Agent` 设计如何降低数据竞争和悬垂引用风险？
8. 这套设计在哪些地方明显体现了 Rust 对架构的约束力？
9. 为什么现在还不急着把整个 `Agent` 改造成 actor？
10. 如果并发度进一步上升，当前 `Agent` 的共享状态管理会先暴露什么瓶颈？

## 10. 失败场景与异常处理

1. 如果 LLM 调用失败，当前 `Agent` 最合理的恢复策略是什么？
2. 如果 assistant turn 已经写入 event，但 tool 执行失败，会留下什么状态？
3. 如果 event 写入成功但 snapshot 更新失败，会带来什么问题？
4. 如果 snapshot 成功但 event 缺失，调试能力会损失什么？
5. 如果某个 exec session 崩掉了，`Agent` 应该如何感知？
6. 如果 approval 中断在半路，resume 之后 `Agent` 该怎么继续？
7. 当前设计更偏向“尽量继续执行”，还是“先保证状态可解释”？为什么？
8. 你觉得当前 `Agent` 设计里最脆弱的异常边界是什么？
9. 如果要增强鲁棒性，你会优先补哪类失败测试？

## 11. 设计取舍与演进

1. 为什么当前 `Agent` 没有直接做成完整 workflow engine？
2. 为什么当前 `Agent` 还保留了一些“中心编排”的味道，而没有完全去中心化？
3. 为什么当前 `Agent` 设计不是纯 actor model？
4. 为什么当前 `Agent` 设计不是纯 event-sourcing？
5. 为什么当前 `Agent` 先做最小 durable runtime，而不是一开始就做复杂 memory system？
6. 当前 `Agent` 设计里最明显的阶段性妥协是什么？
7. 哪些地方是为了先把边界跑通，所以暂时没有继续抽象？
8. 如果 Phase 3 要继续演进，`Agent` 设计最值得拆分的部分是什么？
9. 如果未来支持多用户、多 workspace、多并发 session，当前 `Agent` 边界还撑得住吗？
10. 你如何向面试官解释：这套 `Agent` 设计虽然还早期，但方向是对的？

## 12. 反问自己

1. 我能不能一句话讲清当前 `Agent` 的职责？
2. 我能不能把主循环从 session 加载讲到 tool 执行和 snapshot 更新？
3. 我能不能解释为什么 assistant turn 要先入 event？
4. 我能不能解释为什么 `Agent` 不只是 prompt wrapper？
5. 我能不能讲清 `Agent` 和 exec session / policy / transcript store 的边界？
6. 我能不能说清当前 `Agent` 为什么已经是 stateful runtime orchestrator？
7. 我能不能用 Rust 的所有权和生命周期语言解释这套 `Agent` 边界？
8. 我能不能指出这套 `Agent` 设计的两个最大妥协点？
9. 我能不能讲出一个靠谱的下一阶段演进方向？

## 13. 配套文档

如果你刷这些题时卡住了，可以回看：

- `2026-04-15-phase2-runtime-study-guide.md`
- `2026-04-15-phase2-runtime-code-walkthrough.md`
- `2026-04-15-phase2-runtime-interview-summary.md`
- `2026-04-15-phase2-runtime-question-bank.md`
- `2026-04-15-phase2-runtime-rust-features-notes.md`
