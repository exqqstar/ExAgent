# Tool Registry

## Responsibility

`src/tools/registry.rs` registers tools, exposes tool schemas, and dispatches tool calls.

## State

- `tools`: map from tool name to tool implementation.

## Key Types

- `ToolContext`
- `ToolRegistry`

## Key Flow

1. Runtime asks registry for schemas.
2. LLM returns a `ToolCall`.
3. `ToolRegistry::execute` finds the tool by name.
4. Tool receives the call plus `ToolContext`.
5. Tool returns `ToolResult`.



tools/registry.rs 可以理解成 工具目录 + 工具路由器。
核心类型有两个。
第一个是 ToolContext，在 src/tools/registry.rs (line 13)：
rust



pub struct ToolContext {
    pub config: AgentConfig,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub exec_sessions: Arc<ExecSessionManager>,
    pub policy: Arc<PolicyManager>,
}

这个就是 tool 执行时拿到的运行环境。
比如 read_file/write_file 需要 config.workspace_root；run_command 需要 policy 和 exec_sessions。
第二个是 ToolRegistry，在 line 22 (line 22)：
rust



tools: HashMap<String, Arc<dyn Tool>>

也就是：
text



tool name -> tool implementation

它有三个主要方法：
register(tool)
把一个 tool 放进 registry。

schemas()
把所有 tool 的 name / description / input_schema 组装出来，给 LLM。
LLM 看到这些 schema 后，才知道可以调用哪些工具。

execute(call, ctx)
按 call.name 找对应 tool，然后调用它的 execute。
找不到 tool 就返回 Unknown tool，没有 context 就返回 Tool context missing。

Tool trait 在 src/tools/mod.rs (line 14)：
rust



trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult;
}

所以一个新 tool 要接进系统，基本就是：
text



实现 Tool trait
 -> 注册进 ToolRegistry
 -> schemas 自动暴露给 LLM
 -> execute 自动能被路由

read_file.rs 和 write_file.rs 是最简单的两个 tool。
read_file：
参数：path/start_line/end_line
用 resolve_workspace_path 限制在 workspace 内
读 UTF-8 文本
可按行截取
返回 ToolResult { content, meta.path }
见 src/tools/read_file.rs (line 11)。
write_file：
参数：path/content
用 resolve_workspace_path 限制在 workspace 内
自动创建 parent dirs
写 UTF-8 文本
返回写入路径
见 src/tools/write_file.rs (line 11)。
所以 tools 模块整体关系是：
text



Tool trait
  定义工具统一接口

ToolRegistry
  保存工具，实现 schema 暴露和调用路由

ToolContext
  给工具运行时环境

Concrete tools
  read_file / write_file / run_command

这里也能看到我们前面说的“工具暴露策略”还比较粗：registry 里注册了什么，LLM 就看到什么。后续如果要按 workspace、role、policy mode 动态暴露，就会主要改 ToolRegistry 的构建和 schemas() 输出。
