# ExAgent Phase 2 Rust Features Mock Interview

**Date:** 2026-04-15  
**Audience:** 练习“为什么这部分用 Rust”  
**Format:** 面试官追问 + 参考答法

## 1. 你为什么觉得这部分适合用 Rust 来实现？

**参考答法**

因为这部分已经不是简单脚本，而是 runtime substrate。  
我需要解决的核心问题包括：

- 运行时状态建模
- 并发安全的共享状态
- 长生命周期进程控制
- 风险命令审批边界
- 结构化持久化和 replay

Rust 在这些方面非常适合，因为它能把状态、所有权和错误路径都做得很显式，不容易在系统变复杂后失控。

## 2. 你说的“状态显式”具体体现在哪？

**参考答法**

主要体现在两类地方。

第一类是 typed id，比如 `SessionId`、`EventId`、`ExecSessionId`，我没有到处传裸字符串。  
第二类是 enum 状态机，比如 `ToolStatus`、`ExecSessionStatus`、`PolicyDecision`、`RuntimeEventKind`。  
这样系统状态不是隐含在字符串约定里，而是直接进入类型系统。

## 3. 这里的并发问题主要是什么？

**参考答法**

当前 runtime 的并发问题主要有三块：

1. agent loop 本身是 async 的，要等待 LLM
2. persistent exec session 需要后台持续读取 stdout/stderr
3. API server 可能在多个请求之间共享 runtime manager

所以问题不是“会不会并发”，而是“并发状态怎么安全共享、怎么明确谁能改”。

## 4. 为什么你用了 `Arc` 和 `Mutex`？

**参考答法**

因为有些状态必须共享，而且必须可变。

比如：

- `ExecSessionManager` 要被 agent、tool 和后台任务共享
- `PolicyManager` 要跨请求持有 pending approvals
- 每个 active exec session 的 stdout/stderr、lifecycle 也会变化

这类场景很适合 `Arc<Mutex<...>>`：

- `Arc` 负责共享所有权
- `Mutex` 负责串行化可变访问

这是一种很直接、很清楚的 P0 写法。

## 5. 为什么这里不用 actor model？

**参考答法**

不是不能用，而是当前阶段没必要先把模型做重。  
现在状态形态还不复杂，而且还要频繁 snapshot 和 poll 当前状态，所以 `Arc<Mutex<...>>` 会更直接。

如果后续 runtime 再复杂一些，比如有更多 background orchestration、更多控制消息，那时再考虑 actor 化会更合理。

## 6. 所有权设计上你最看重什么？

**参考答法**

我最看重的是把“长期状态”和“短期上下文”分开。

长期状态比如：

- exec session manager
- policy manager
- session snapshot

这类对象要么被拥有，要么被 `Arc` 共享。

短期上下文比如：

- 一次 tool 调用时的只读 config
- 当前 prompt
- 某轮的消息切片

这类只在当前调用栈里短借用。  
这样系统边界会清楚很多。

## 7. 你提到生命周期设计，这里明明没写很多 lifetime 参数，为什么还说有生命周期设计？

**参考答法**

因为生命周期设计不等于“代码里写很多 `'a`”。  
真正的生命周期设计，是你怎么决定哪些数据该拥有，哪些只该借用，以及哪些状态绝对不能跨 async 边界借用。

当前实现里其实用了一个很明确的策略：

- 跨 `.await`、跨 task、跨 request 的状态，尽量提升为拥有所有权的对象或 `Arc`
- 短期上下文才借用

这本质上就是在主动降低生命周期复杂度。

## 8. 能举一个生命周期设计的具体例子吗？

**参考答法**

最典型的是 `ExecSessionManager::start` 和后台 stdout/stderr 读取任务。

如果我把局部借用引用直接带进 `tokio::spawn`，生命周期会非常麻烦，也容易出错。  
当前做法是把需要长期存在的状态包装进 `ActiveExecSession`，再放进 `Arc` 管理的共享结构里。后台任务拿到的是可安全移动和共享的句柄，而不是依赖外层栈帧活着的借用引用。

## 9. 为什么 `Tool` 和 `LlmClient` 这里用了 trait object，而不是泛型？

**参考答法**

因为这里的核心需求是运行时多态，不是编译期特化。

比如：

- LLM 可能是 mock，也可能是 OpenAI-compatible client
- Tool registry 需要在运行时装多种不同工具

如果全用泛型，`Agent` 和 `ToolRegistry` 的类型参数会迅速膨胀。  
trait object 更适合当前这个“接口稳定、实现可替换”的 runtime 结构。

## 10. Serde 在这里为什么重要？

**参考答法**

因为当前 runtime 已经离不开结构化持久化了。  
我需要把这些东西可靠地落盘和读回：

- session snapshot
- runtime events
- API payload
- OpenAI-compatible 协议对象

Serde 的价值是，它让类型定义和持久化格式直接绑定起来。  
这对 replay、调试和演化都很重要。

## 11. 你觉得最有 Rust 味道的三个设计点是什么？

**参考答法**

我会选这三个：

1. 用 newtype 包装核心 id，而不是裸字符串
2. 用 enum 显式表示 runtime 状态机
3. 用 `Arc<Mutex<...>>` 和所有权模型明确共享状态边界

这三个点叠在一起，说明这个项目已经不只是“能跑”，而是在认真做 runtime engineering。

## 12. 如果面试官说“这些别的语言也能做”，你怎么回答？

**参考答法**

我会说，当然别的语言也能做，但 Rust 在这里的优势不是“只有它能做”，而是：

- 它更擅长把状态边界做清楚
- 更擅长在编译期暴露状态机遗漏和类型混用
- 更适合长期控制共享可变状态
- 更适合在 async runtime 下避免模糊的生命周期问题

所以这不是能力存在与否的问题，而是长期工程质量和演化成本的问题。

## 13. 最后一句怎么收尾

**参考答法**

如果要收尾，我会说：

> 这部分用 Rust 的意义，不是单纯为了性能，而是为了把 runtime 的状态、并发、所有权边界和持久化模型做得显式、可验证，而且能支撑系统继续变复杂。
