# ExAgent Phase 3 Session Orchestration Notes

**Date:** 2026-04-15  
**Purpose:** 记录 Phase 3 建议的推进方式，方便新 session 直接沿用  
**Current Baseline:** `main` and `codex/phase3-p0-runtime` are aligned at `8328232`
**Formal Implementation Plan:** `docs/plans/2026-04-15-exagent-phase3-p0-thin-orchestration-implementation-plan.md`

## 1. 背景判断

Phase 2 现在已经可以视为 Phase 3 的正式开发基线，但 Phase 3 还不适合一上来就让多个 agent 同时改核心实现。

原因不是 subagent 没价值，而是当前最容易出错的地方还不是“实现速度”，而是：

- Phase 3 的目标边界还没有最终锁住
- 需要先明确 goals / non-goals / acceptance criteria
- 需要先决定哪些部分能独立拆分，哪些不能

如果在这些问题还没钉住时就并行推进，很容易出现多个 agent 各自理解一套 Phase 3，然后主线程花很多时间收拾冲突。

## 2. 总体建议

推荐采用：

**先串行定方向，再并行做中段，最后串行收口。**

也就是：

1. 先由主线程确定问题边界
2. 再让少量 agent 并行产出 spec / test input
3. 主线程收敛成统一计划
4. 计划锁定后，再开 implementation agents
5. 最后由 review / judge 做独立挑战

不推荐一开始就同时开：

- 一个写 plan
- 一个写测试
- 一个直接实现
- 一个 judge

因为在 plan 还没定的情况下，让实现和 judge 同时开跑，通常只会放大歧义。

## 3. 推荐角色划分

### Lead / Integrator

由主线程承担。职责是：

- 定最终目标
- 定任务边界
- 决定拆分方式
- 整合子 agent 输出
- 做最终验证和合并

这个角色不建议外包。

### Spec / Goal Agent

职责：

- 起草 Phase 3 的目标
- 写 `goals / non-goals / milestone / acceptance criteria`
- 标出关键风险和依赖

注意：

- 它提供草稿
- 不负责拍板

### Test / Validation Agent

职责：

- 先梳理回归风险
- 识别新增能力该补哪些测试
- 输出测试矩阵和验收检查单

它非常适合在计划阶段并行，因为不需要先改核心实现。

### Implementation Agent(s)

只有在 spec 锁定后才建议开启。

拆分原则：

- 写入范围尽量不重叠
- 每个 agent 只负责一个清晰子系统
- 不把高度耦合的核心控制流拆给多个 agent 同时改

### Judge / Review Agent

这个角色很有价值，但不应该从第一分钟就参与“共同写 plan”。

更合理的定位是：

- 在 plan 初稿完成后做挑战和审核
- 在实现完成后做一次独立 review

也就是说，judge 更像：

- reviewer
- critic
- scope checker
- risk challenger

而不是主导作者。

## 4. 写 Plan 时要不要有 Judge

结论：

**要有，但最好作为“第二阶段 reviewer”，不要从第一步就变成共写作者。**

原因：

- 太早引入 judge，容易让讨论变成来回否定，拖慢收敛
- 在没有初稿时，judge 没有稳定对象可以审
- 先有一版 plan，再由 judge 独立挑刺，效率更高

推荐流程是：

1. Lead 先给出 Phase 3 的问题定义
2. Spec Agent 起草 plan skeleton
3. Test Agent 起草测试和验收矩阵
4. Lead 合并为一个统一 plan draft
5. Judge Agent 审 plan draft

Judge 在 plan 阶段重点检查：

- goal 是否清晰
- non-goals 是否明确
- 是否 scope 过大
- 是否缺 acceptance criteria
- 是否缺风险和回退策略
- 是否把多个里程碑混成一次实现
- 是否存在无法并行拆分却硬拆的问题

所以答案不是“不要 judge”，而是：

**不要让 judge 太早介入写作本身，要让它在 draft 出来后发挥最大价值。**

## 5. 推荐的下一 Session 启动顺序

如果下个 session 要正式启动 Phase 3，建议按这个顺序：

1. 主线程快速复述当前基线
2. 明确这次 Phase 3 要推进的是哪一个 milestone
3. 开 `Spec / Goal Agent`
4. 开 `Test / Validation Agent`
5. 主线程并行阅读 Phase 2 基线和相关文档
6. 汇总成一份统一的 Phase 3 draft plan
7. 开 `Judge / Review Agent` 审 plan
8. plan 定稿后，再决定是否开 1-2 个 implementation agents

## 6. 哪些任务适合并行

适合并行的：

- spec 草稿
- test strategy
- risk checklist
- docs mapping
- 独立子模块实现

不适合过早并行的：

- 主 runtime 控制流重构
- 还没定边界时的核心实现
- 同时修改同一批核心文件
- 还没定义清楚验收标准时的功能推进

## 7. 下个 Session 的推荐模式

推荐模式：

**Lead + Spec Agent + Test Agent -> Lead Synthesis -> Judge Review -> Implementation Agents -> Final Review**

不推荐模式：

**Spec + Test + Implementation + Judge 全部同时启动**

## 8. 直接可用的判断准则

如果下个 session 里出现下面任一情况，就先不要开 implementation agents：

- 还说不清楚本次 Phase 3 的单一目标
- 还没写出 non-goals
- 还没定义 acceptance criteria
- 还没判断能否按文件或子系统拆分

如果这些已经清楚了，再开 implementation agents 才是真正提效。

## 9. 一句话结论

Phase 3 可以用 subagents 提效，但最佳时机不是最开始。  
最值得先开的，是 `Spec Agent` 和 `Test Agent`。  
`Judge` 在写 plan 时也值得有，但更适合在 **draft 完成之后** 介入，而不是从第一步就一起共写。
