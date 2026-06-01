# Runtime Agent

## Responsibility

`src/runtime/agent.rs` binds together runtime configuration, LLM client, tool registry, exec sessions, and policy.

It is intentionally thin. It does not own thread lifecycle or event recording.

## State

Owned values:

- `AgentConfig`
- boxed `LlmClient`
- `ToolRegistry`
- shared `ExecSessionManager`
- shared `PolicyManager`

## Key Functions

- `sample_assistant_turn`: calls the LLM.
- `tool_runtime`: creates a turn-scoped `ToolCallRuntime`.
- `max_turns` and `config`: expose runtime settings to session logic.

## Connections

- Created by `ThreadManager` through an `AgentFactory`.
- Called by `ThreadSession`.
- Uses `model` and `tools`.





ThreadManager.ensure_runtime_loaded
  -> ThreadRuntime::spawn(
       ThreadRuntimeOptions::new(
         thread_id,
         config,
         self.runtime_agent_factory()
       )
     )

agent.rs 很薄，它不是 agent loop。真正 loop 在 thread_session/turn.rs。Agent 的角色是把执行 turn 需要的依赖绑在一起：

Agent {
    config,
    llm,
    registry,
    exec_sessions,
    policy,
}
它持有什么

AgentConfig：model、max_turns、workspace/cwd、timeout、policy mode 等配置。
LlmClient：真正调用模型的接口。
ToolRegistry：当前 agent 可用的 tools。
ExecSessionManager：给 run_command 的 persistent command 用。
PolicyManager：给 command approval / policy decision 用。
它怎么创建

ThreadManager.runtime_agent_factory() 创建 AgentFactory，然后 ThreadSession::new 里调用：

agent_factory(config.clone())
最终会走：

Agent::with_runtime(config, llm, registry, exec_sessions, policy)
所以 Agent 是在 ThreadSession 创建时一起创建的。

它做什么

核心只有两个动作。

第一个：

sample_assistant_turn(prompt, tool_schemas)
就是：

调用 self.llm.complete(prompt, tool_schemas)
也就是问 LLM 要下一次 assistant turn。

第二个：

tool_runtime(session_id, turn_id, workspace_root, cwd)
创建一个 turn-scoped 的 ToolCallRuntime。它会 clone config，然后把这次 turn 的 workspace_root/cwd 写进去，再把 registry、exec_sessions、policy、session_id、turn_id 传过去。

一句话

Agent = LLM client + tool registry + runtime resources 的薄封装；
ThreadSession 通过它调用 LLM，并为每个 turn 创建 ToolCallRuntime。



-----

AgentFactory 这个类型 不是在 agent.rs 里定义的，是在 thread_runtime.rs (line 21) 定义的：
rust



pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;

但它返回的对象是 agent.rs 里的：
rust



Agent

所以关系是：
text



thread_runtime.rs
  定义 AgentFactory 这个函数类型

app_server/thread_manager.rs
  创建一个 AgentFactory 闭包
  闭包内部调用 Agent::with_runtime(...)

thread_session/mod.rs
  调用 agent_factory(config.clone())
  真正创建 Agent

runtime/agent.rs
  定义 Agent 结构和 Agent::with_runtime

也就是：
text



之前只是把“怎么创建 Agent”的闭包一路传递；
到了 ThreadSession::new，闭包被调用；
闭包内部才真正用 agent.rs 的 Agent::with_runtime 创建 Agent。

agent.rs 本身的作用就是定义 Agent 这个运行时对象：
text



Agent = config + llm + tool registry + exec sessions + policy

然后提供两个核心动作：
text



sample_assistant_turn
  -> 调 LLM

tool_runtime
  -> 创建当前 turn 的 ToolCallRuntime

所以你可以记成：
text



AgentFactory 是创建 Agent 的延迟工厂；
agent.rs 是 Agent 的定义和两个执行入口。


完整说法可以这样记：
thread_runtime.rs
  定义 AgentFactory 类型：
  “给我 AgentConfig，我返回 Agent”

app_server/thread_manager.rs
  创建 AgentFactory 的具体闭包：
  闭包里知道怎么 build LLM、registry、exec_sessions、policy
  并调用 Agent::with_runtime(...)

thread_runtime.rs
  ThreadRuntimeOptions 携带这个 AgentFactory
  ThreadRuntime::spawn 把它继续传给 ThreadSessionOptions

thread_session/mod.rs
  ThreadSession::new 里正式调用：
  agent_factory(config.clone())
  这里才真正创建 Agent

agent.rs
  定义 Agent::with_runtime
  创建出来的 Agent 后续提供：
  sample_assistant_turn
  tool_runtime





----
这里的 ThreadManager 是 app_server 里的：
src/app_server/thread_manager.rs (line 585)
链路重新串一下：
text



ThreadManager.ensure_runtime_loaded
  -> ThreadRuntime::spawn(
       ThreadRuntimeOptions::new(
         thread_id,
         config,
         self.runtime_agent_factory()
       )
     )

runtime_agent_factory() 是 app_server ThreadManager 创建的。它会捕获：
text



llm_factory
registry_factory
exec_sessions
policy

然后返回一个闭包：
rust



Arc<dyn Fn(AgentConfig) -> Result<Agent>>

也就是：
text



给我一个 AgentConfig，我就能创建一个 Agent。

这个闭包就是 AgentFactory。
接着进入 runtime：
text



ThreadRuntime::spawn
  -> ThreadSession::new(
       ThreadSessionOptions::new(thread_id, config, agent_factory)
     )

ThreadSession::new 是在 thread_session/mod.rs (line 133) 里定义的。它是在 ThreadRuntime::spawn 里面被触发的，不是外部直接触发。
在 ThreadSession::new 里面有这句：
rust



let agent = agent_factory(config.clone())?;

也就是说，真正创建 Agent 是在 session 初始化时发生的。
Agent::with_runtime(...) 的意思就是“用完整 runtime 依赖创建 Agent”。它接收：
text



config
llm
registry
exec_sessions
policy

然后放进 Agent struct 里。
Agent::new(...) 是简单版本，只传：
text



config
llm
registry

然后自己创建默认 ExecSessionManager 和 PolicyManager。
with_runtime(...) 是生产路径用的，因为 app_server 要把共享的 exec/policy 传进去。
所以完整链路是：
text



app_server ThreadManager
  -> runtime_agent_factory()
      -> 创建 AgentFactory 闭包

ThreadManager.ensure_runtime_loaded
  -> ThreadRuntime::spawn(... AgentFactory ...)

ThreadRuntime::spawn
  -> ThreadSession::new(... AgentFactory ...)

ThreadSession::new
  -> agent_factory(config)
      -> Agent::with_runtime(...)

你最后的总结是对的：

Agent 主要两个能力：
1. sample_assistant_turn：给 LLM 发请求
2. tool_runtime：给工具执行创建环境









-----
就是 Rust 命名习惯，但这里也确实容易混。我用一个小例子讲清楚。

pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Result<Agent> + Send + Sync>;
这行定义的是 类型别名。

意思是：

AgentFactory 这个类型
= 一个可共享的函数/闭包
= 输入 AgentConfig
= 输出 Result<Agent>
然后代码里这些是 变量 / 字段 / 参数：

agent_factory: AgentFactory
意思是：

变量名叫 agent_factory
它的类型是 AgentFactory
类比一下：

let name: String = "abc".to_string();
这里：

String = 类型
name = 变量
同理：

let agent_factory: AgentFactory = ...
这里：

AgentFactory = 类型
agent_factory = 变量
具体到这条链：

fn runtime_agent_factory(&self) -> AgentFactory
意思是：

这个函数返回一个 AgentFactory 类型的值
返回的值是：

Arc::new(move |config: AgentConfig| {
    ...
    Ok(Agent::with_runtime(...))
})
这个 move |config| { ... } 就是闭包，也就是一个可以之后调用的函数值。

然后它被放进：

ThreadRuntimeOptions {
    agent_factory: AgentFactory,
}
再传到：

ThreadSessionOptions {
    agent_factory: AgentFactory,
}
最后调用：

let agent = agent_factory(config.clone())?;
这句就是：

调用这个闭包
把 config.clone() 作为参数传进去
拿到 Agent
所以关系是：

AgentFactory 是类型
agent_factory 是这个类型的一个变量/字段
闭包是 agent_factory 变量里装的实际函数
关于 AgentConfig：对，它也是一路带进来的。

链路是：

ThreadManager.base_config
  -> override_policy 生成 request/thread config
  -> ThreadRuntimeOptions.config
  -> ThreadSessionOptions.config
  -> ThreadSession::new(options)
在 ThreadSession::new 里，config 同时有两个用途：

给 Agent：
let agent = agent_factory(config.clone())?;
给 session 初始化自己用：
rollout_paths(&config.workspace_root, &thread_id)
所以它不只是 turn config，更像是 这个 loaded thread/runtime 的基础配置。

每个 turn 可以临时传 turn_context.cwd，但底层还是基于这份 AgentConfig：

AgentConfig = thread/runtime/session 的基础配置
ThreadTurnContext = 单个 turn 的临时覆盖，目前主要是 cwd
你最后那句是对的：llm_factory / registry_factory / exec_sessions / policy 都在 ThreadManager 手里，所以让 ThreadManager 负责组装 AgentFactory，分层会更干净。