# ExAgent Phase 2 Rust Features Interview One-Pager

**Date:** 2026-04-15  
**Audience:** 快速面试回答、1 页复习  
**Status:** 面向当前 `codex/phase2-p0-runtime` 实现

## 核心结论

如果面试官问“为什么这部分用 Rust 做得更合适”，最短的回答是：

> 因为这已经不是简单脚本了，而是 runtime substrate。  
> 我需要显式状态建模、并发安全的共享状态、清晰的所有权边界、结构化持久化，以及长期可演化的接口。  
> Rust 在这些点上非常强。

## 一分钟答法

当前 Phase 2 里，我最主要利用了几类 Rust 特性。

第一类是强类型状态建模。我用 newtype 包了 `SessionId`、`EventId`、`ExecSessionId` 这些关键 id，用 enum 建模了 `ToolStatus`、`RuntimeEventKind`、`PolicyDecision` 这些运行时状态。这样状态边界是显式的，不容易混用，也能让编译器帮助我检查分支完整性。

第二类是并发和共享状态管理。因为 runtime 里有 persistent exec session、approval cache、API server 共享 manager，所以我用了 `Arc` 和 Tokio 的 `Mutex` 来管理共享可变状态。这样能安全地跨 async task 和 request 共享状态，而不是靠隐式全局变量。

第三类是所有权和生命周期设计。这个阶段虽然没有写很多显式 lifetime 参数，但设计上我已经在利用 Rust 的所有权模型：长期状态尽量用拥有所有权的数据结构或 `Arc`，短期上下文再借用。这样避免把短生命周期引用带过 `.await` 或后台任务边界。

第四类是 Serde 和 async/Tokio。Serde 让 session snapshot、runtime event、API payload 能和类型系统直接打通；Tokio 则让 agent loop、subprocess 和 HTTP API 能在同一个异步 runtime 下协同工作。

## 高频关键词

- strong typing
- explicit state machine
- ownership boundary
- async-safe shared state
- `Arc<Mutex<...>>`
- trait object for runtime polymorphism
- Serde-backed persistence
- approval before sandbox

## 并发怎么答

> 当前 runtime 的并发主要体现在三件事上：  
> agent 要异步调用 LLM，persistent process 要后台持续读 stdout/stderr，API server 还要在多请求下共享 runtime manager。  
> 所以我用了 Tokio async model 配合 `Arc<Mutex<...>>`。这样共享状态是显式的，读写边界也很清楚。

## 所有权和生命周期怎么答

> 这个实现里我没有刻意炫复杂 lifetime 参数，而是通过架构选择降低生命周期复杂度。  
> 跨 request、跨 task、跨 `.await` 的状态，我尽量用 `Arc` 和拥有所有权的结构；短期上下文再借用。  
> 这样做的好处是 runtime service 更容易组合，也避免把局部借用错误地带进长期状态。

## 为什么不用别的写法

- 不到处用裸 `String`：因为 id 容易混
- 不把状态都做成字符串：因为 runtime 状态机会失控
- 不全用泛型：因为 registry 和 LLM 接口本质上需要运行时多态
- 不先上 actor model：因为 P0 阶段 `Arc<Mutex<...>>` 更直接、更容易验证
- 不到处传引用：因为 async runtime 下跨 `.await` 的生命周期成本太高

## 一句话收尾

> 我对 Rust 的利用重点，不是性能炫技，而是用它把 runtime 的状态、并发、边界和持久化变得显式、可验证、可长期演化。
