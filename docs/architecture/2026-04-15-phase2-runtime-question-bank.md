# ExAgent Phase 2 Runtime Interview Question Bank

**Date:** 2026-04-15  
**Audience:** 面试准备、项目复盘、自测刷题  
**Status:** 基于当前 `codex/phase2-p0-runtime` worktree 的真实实现整理

## 1. 怎么使用这份题库

这份文档刻意把重点放在“问题”而不是“答案”上。  
它适合你后面单独拿出来刷，不一定要和其他文档一起看。

建议使用方式：

1. 先只刷“必答 Top 15”
2. 每题先尝试用 30-90 秒回答
3. 说不出来的题，回去对照总览版和面试答题版
4. 第二轮再刷 Rust、取舍、未来规划这些更深入的问题

如果你想更像真实面试，可以给每题打一个状态：

- `A`: 能独立讲清楚
- `B`: 能讲大意，但细节不稳
- `C`: 基本答不出来

## 2. 必答 Top 15

1. Phase 2 主要解决了什么问题？
2. 它和 Phase 1 的本质差别是什么？
3. 为什么这个阶段要从“一次性 agent loop”升级成 runtime substrate？
4. 为什么当前设计同时保留 `snapshot.json` 和 `events.jsonl`？
5. `SessionSnapshot` 里最关键的状态有哪些？
6. `Agent::run_session` 的主循环是怎么推进的？
7. assistant turn 为什么要先写 event，再执行 tool？
8. tool result 为什么不只返回文本，还要带 metadata？
9. persistent exec session 为什么是 Phase 2 的关键能力？
10. `ExecSessionManager` 解决了什么问题？
11. policy / approval hook 为什么要尽早进入 runtime，而不是以后再补？
12. 为什么这部分实现适合用 Rust？
13. 这里的并发状态为什么用 `Arc<Mutex<...>>`？
14. 这里的所有权和生命周期设计，最核心的约束是什么？
15. 如果继续做 Phase 3，你会先推进哪几件事？

## 3. 项目背景与目标

1. 这个项目的目标是什么？
2. 当前 ExAgent 想解决的是哪类 agent 场景？
3. 为什么说 Phase 1 更像 demo，而不是 durable runtime？
4. 在进入 Phase 2 之前，系统缺的最关键能力是什么？
5. 为什么 resume 能力不是“锦上添花”，而是 runtime 的基础能力？
6. 为什么把 process state 也纳入 session snapshot 很重要？
7. 为什么 approval state 也应该进 snapshot，而不只放在内存里？
8. 当前 Phase 2 的边界是怎么定义的？
9. 哪些设计已经铺好了接口，但还没完整落地？
10. 你怎么判断一个 agent 项目已经从“玩具系统”进入“runtime 阶段”？

## 4. 架构总览

1. 你能先高层讲一下当前 Phase 2 的模块划分吗？
2. `agent.rs`、`transcript.rs`、`exec_session.rs`、`policy.rs` 分别负责什么？
3. 当前 runtime 里，哪些东西是“当前状态”，哪些东西是“历史轨迹”？
4. 为什么 session、event、tool execution、policy 需要同时存在，而不是把逻辑都塞进 agent loop？
5. 当前实现里，哪些模块是强耦合的，哪些模块是相对独立的？
6. 如果新人第一次进入这个代码库，你会推荐先读哪几个文件？为什么？
7. 这套结构最重要的架构判断是什么？
8. 当前代码里，哪些设计明显在为后续阶段预留扩展点？

## 5. Session、Snapshot 与 Event Log

1. `SessionSnapshot` 的职责是什么？
2. `RuntimeEvent` 的职责是什么？
3. 为什么 snapshot 和 event log 不能互相替代？
4. 如果只有 snapshot，会损失什么？
5. 如果只有 event log，会带来什么复杂度？
6. `conversation` 为什么属于 snapshot 的核心组成部分？
7. `open_exec_sessions` 为什么也要进入 snapshot？
8. `pending_approvals` 为什么必须持久化？
9. `latest_compaction` 这种字段反映了什么设计意图？
10. `events.jsonl` 为什么适合用 append-only 方式写？
11. event log 更偏向“恢复”还是“可观测性”？为什么？
12. 这套 snapshot + events 设计和经典 event sourcing 有什么像和不像的地方？
13. 如果 event 写入成功但 snapshot 更新失败，会发生什么？
14. 如果 snapshot 写入成功但 event 丢了，会影响什么？
15. 未来如果要做 compaction，这套数据结构该怎么演进？

## 6. 主循环与 Tool 执行

1. `Agent::run_session` 的核心控制流是什么？
2. agent loop 每一轮真正做了哪几件事？
3. 为什么 assistant turn 也要进入 runtime event，而不是只保留最终结果？
4. tool call 的分发边界在哪里？
5. tool result 的 metadata 在这套系统里扮演什么角色？
6. snapshot 的更新为什么发生在 tool 执行之后？
7. 为什么不把 snapshot 更新逻辑直接散落在每个 tool 里？
8. 当前 loop 的退出条件是什么？
9. 这套 loop 如何兼顾“模型上下文推进”和“runtime 状态推进”？
10. 如果 tool 执行失败，当前 loop 该怎么保持可恢复性？
11. 这里的“event first”设计具体意味着什么？
12. 这种执行顺序更偏向一致性、可调试性，还是实现简单？为什么？

## 7. Persistent Exec Session

1. 什么是 persistent exec session？
2. 为什么 coding agent 不能只靠 one-shot command？
3. `ExecSessionManager` 维护的核心状态有哪些？
4. 为什么 exec session 要按 `session_id` 分桶？
5. `start`、`poll`、`write stdin`、`terminate` 四种操作分别解决什么问题？
6. stdout 和 stderr 为什么要持续读取，而不是只在结束时拿一次？
7. 为什么后台 reader task 是合适的实现方式？
8. `RuntimeEvent::ExecOutput` 的价值是什么？
9. 内存 buffer 和 event log 各自承担什么职责？
10. 如果一个子进程持续输出，当前方案可能遇到什么压力？
11. 如果 process 已经退出，但 snapshot 里还留着记录，该如何理解这种状态？
12. persistent exec 和 resume 的关系是什么？
13. 如果你要支持更多 shell / TTY 特性，现在哪些部分会先变复杂？

## 8. Policy 与 Approval

1. 为什么 Phase 2 就要把 policy / approval 放进来？
2. `PolicyManager` 主要负责什么？
3. 哪些命令或动作会被视为需要 approval？
4. approval cache 的作用是什么？
5. 为什么 approval 不能只靠一次性 UI 提示，而要进入 runtime state？
6. `pending_approvals` 进入 snapshot 后，resume 能力得到了什么增强？
7. 当前 policy 设计是“强策略引擎”还是“最小 hook”？为什么这样选？
8. 如果后面要接更复杂的 sandbox / permission model，现在哪些接口最关键？
9. 为什么 policy 模块最好独立出来，而不是散在 tool implementation 里？
10. 当前 approval 流程里最大的简化点是什么？

## 9. API、CLI 与 Resume 路径

1. 当前用户是怎么进入 `resume` 路径的？
2. CLI 和 API 在 session 恢复上分别扮演什么角色？
3. 为什么对外接口要暴露 session id？
4. 为什么“恢复旧 session”不是简单地把旧对话再发给模型？
5. 当前 transcript 目录结构如何支持恢复和调试？
6. 如果用户提供了错误的 session id，系统应该怎么表现？
7. 如果 API 层要做多用户隔离，当前设计还缺什么？

## 10. Rust 语言特性与实现理由

1. 为什么这部分更适合用 Rust，而不是先用动态语言快速堆起来？
2. 为什么这里的价值重点不是“性能”，而是“runtime 边界”？
3. `SessionId`、`TurnId`、`EventId` 为什么做成 newtype？
4. `macro_rules!` 在这里主要帮你减少了什么重复？
5. 为什么状态建模会优先用 `enum`？
6. trait 和 trait object 在这套 runtime 里解决了什么扩展问题？
7. 为什么异步场景下常常会优先传 owned data，而不是到处借用引用？
8. 为什么这里没有大量显式 lifetime 参数，但你仍然说生命周期设计很重要？
9. 跨 `.await` 保存借用为什么危险？
10. 为什么 `Arc<Mutex<...>>` 是当前阶段合理的选择？
11. 为什么现在没有一上来就做 actor model？
12. `Option` 和 `Result` 在当前实现里分别承担什么语义？
13. `PathBuf` 为什么比裸字符串更合适？
14. `serde` 在这套持久化模型里的作用是什么？
15. `anyhow` 为什么适合作为当前阶段的错误聚合方式？

## 11. 并发、所有权、借用、生命周期

1. 当前 Phase 2 真正的并发点有哪些？
2. 哪些状态是“跨任务共享”的，哪些状态是“单次调用局部”的？
3. 为什么 `ExecSessionManager` 这种共享状态需要显式同步？
4. 为什么当前更适合 `Arc<Mutex<...>>`，而不是把所有共享状态拆成大量 channel actor？
5. 这套设计里最典型的所有权边界是什么？
6. 哪些数据应该短期借用，哪些数据应该长期拥有？
7. 为什么跨 `.await`、跨 task、跨 request 的数据经常会被提升成 owned data？
8. 这里的生命周期设计，最重要的不是“写 lifetime 参数”，而是什么？
9. 你如何解释“Rust 的生命周期规则参与了架构设计，而不只是编译细节”？
10. 如果你强行用大量引用把这套 runtime 串起来，最容易在哪些地方出问题？
11. 当前设计如何降低悬垂引用和数据竞争风险？
12. 如果未来并发量更高，`Mutex` 粒度会不会成为问题？
13. `Send` / `Sync` 之类的约束在这里会怎么影响结构设计？
14. 如果要把某块逻辑移到后台 task，你首先会检查哪些所有权问题？
15. 你如何向不懂 Rust 的面试官解释 ownership 对 runtime 的实际价值？

## 12. 设计取舍与替代方案

1. 为什么当前不是纯 event-sourcing？
2. 为什么当前不直接上数据库，而是先用文件持久化？
3. 为什么 event log 选 `jsonl`，而不是二进制格式？
4. 为什么当前不先做完整的 workflow engine？
5. 为什么 approval 只做到 hook 和 pending state，而没有做复杂策略 DSL？
6. 为什么当前不先做 full actor runtime？
7. 为什么当前不先做 distributed multi-worker 架构？
8. 为什么先做 session / exec / approval，再做更高级功能？
9. 当前实现里最明显的工程化妥协有哪些？
10. 哪些地方是“先跑通，再抽象”的典型例子？
11. 如果你现在重做一次，有哪两个地方你会改得更彻底？

## 13. 测试与验证

1. 当前 Phase 2 主要靠哪些测试来验证？
2. `tests/resume.rs` 最关键验证了什么？
3. `tests/exec_session.rs` 最关键验证了什么？
4. `tests/policy.rs` 最关键验证了什么？
5. `tests/api_server.rs` 在 Phase 2 语境下有什么价值？
6. 为什么 resume / exec / policy 这三类测试必须单独存在？
7. 现有测试更偏向单元测试、集成测试，还是端到端测试？
8. 现在最缺的测试是什么？
9. 如果引入更多并发路径，测试难度会怎么变？
10. 你如何证明这套 runtime 现在至少已经跨过了“只能 demo、不能持续演进”的门槛？

## 14. 未来规划

1. 如果继续推进 Phase 3，你的优先级排序是什么？
2. `latest_compaction` 暗示了后续会做什么能力？
3. event log 未来是否需要裁剪、聚合或索引？
4. exec session 未来是否要支持更强的 TTY / shell 语义？
5. approval 流程后面会如何接入更真实的权限系统？
6. 当前 session model 如果继续扩展，最容易膨胀的是什么？
7. 如果要支持多 workspace 或多用户，会先改哪些边界？
8. 如果要做 observability 面板，现有 event log 能直接复用多少？
9. 如果要做 crash recovery，现在哪些能力已经有基础，哪些还没有？
10. 这套 runtime 离“真正生产可用”还差哪几步？

## 15. 追问型问题

1. 你说这是 durable runtime，具体“durable”体现在哪？
2. 你说这是 replayable，当前 replay 到了什么程度，还没到什么程度？
3. 你说 approval state 进入 snapshot 很重要，能给个失败场景例子吗？
4. 你说 Rust 的价值不是性能，那为什么不用 Go？
5. 你说 `Arc<Mutex<...>>` 是阶段性选择，那你什么时候会考虑换成 actor？
6. 你说生命周期设计影响了架构，能举一个跨 `.await` 的具体例子吗？
7. 你说 event-first 更利于调试，能具体说说一次问题排查会怎么做吗？
8. 你说 current state 和 historical trace 分离，这个判断和数据库里的 state table + audit log 有什么相似点？
9. 你说这套东西已经不是 demo，那你会用什么标准反驳“这还只是个原型”？
10. 如果面试官质疑“这套实现还很早期”，你会怎么回应？

## 16. 反问自己

1. 我能不能不用看文档，独立讲清楚 Phase 2 的目标和边界？
2. 我能不能解释 snapshot 和 event log 为什么必须并存？
3. 我能不能把主循环从头到尾口述出来？
4. 我能不能说清 persistent exec 为什么比 one-shot command 高一个层级？
5. 我能不能讲清 approval 为什么属于 runtime，而不只是 UI 交互？
6. 我能不能把 Rust 的价值讲成“状态安全和边界清晰”，而不是只会说“快”和“安全”？
7. 我能不能解释 `Arc<Mutex<...>>` 的合理性和局限？
8. 我能不能把 ownership / borrowing / lifetimes 讲成架构设计问题，而不是语法细节？
9. 我能不能明确说出现在还没做好的地方？
10. 我能不能讲出一个靠谱的 Phase 3 路线图？

## 17. 推荐刷题顺序

如果你时间不多，建议按这个顺序刷：

1. 先刷第 2 节“必答 Top 15”
2. 再刷第 5、6、7、8 节，掌握 runtime 本体
3. 然后刷第 10、11 节，把 Rust 设计理由补齐
4. 最后刷第 12、13、14、15 节，应对追问和质疑

## 18. 配套文档

如果某类题答不出来，可以回看这些资料：

- 架构总览版：`2026-04-15-phase2-runtime-study-guide.md`
- 源码走读版：`2026-04-15-phase2-runtime-code-walkthrough.md`
- 面试答题版：`2026-04-15-phase2-runtime-interview-summary.md`
- 双语面试稿：`2026-04-15-phase2-runtime-bilingual-interview-script.md`
- Rust 特性笔记：`2026-04-15-phase2-runtime-rust-features-notes.md`
- Rust 一页面试版：`2026-04-15-phase2-rust-features-interview-one-pager.md`
- Rust mock interview：`2026-04-15-phase2-rust-features-mock-interview.md`
