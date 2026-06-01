# Run Command Tool

## Responsibility

`src/tools/run_command.rs` executes shell commands inside the workspace.

It supports:

- one-shot commands
- persistent commands
- polling persistent command output
- writing stdin
- terminating persistent commands
- approval decisions

## State Connections

- Uses `PolicyManager` for command classification and approval storage.
- Uses `ExecSessionManager` for persistent subprocesses.
- Returns metadata interpreted by `ToolCallRuntime` as exec session or approval effects.

## Key Branches

- `approval_id` present: handle approval decision.
- `exec_session_id` present: operate on persistent session.
- `persistent = true`: start persistent command.
- otherwise: run one-shot command.

## Safety

Commands run via `sh -lc` in a resolved workspace cwd. Risky commands can require approval depending on policy mode.




对，可以理解成：run_command 给 LLM 暴露了一组“命令执行动作”，但它不是白名单命令列表，而是一个 shell 执行器。

它现在支持的动作大概是 4 类，不是 3 类：

1. one-shot command
   { command: "ls", cwd: "src" }

2. start persistent command
   { command: "npm run dev", persistent: true }

3. operate persistent command
   { exec_session_id: "exec_1" }
   { exec_session_id: "exec_1", stdin: "...\n" }
   { exec_session_id: "exec_1", terminate: true }

4. approval decision
   { approval_id: "approval_1", decision: "approved" }
   { approval_id: "approval_1", decision: "denied" }
“哪些命令可以使用”现在还没有细定义。
当前就是 sh -lc <command>，再用 PolicyManager 做很粗的 deny/review 判断。所以后续确实需要定义：

哪些命令默认允许。
哪些命令需要 approval。
哪些命令永远禁止。
persistent 命令是否默认需要 approval。
不同 workspace/agent role/policy mode 下能用什么。
ToolCallRuntime interpret metadata 你的理解对。流程是：

LLM 生成 tool call
 -> ToolCallRuntime.execute
 -> ToolRegistry.execute
 -> run_command.execute
 -> 返回 ToolResult { content, meta }
 -> ToolCallRuntime 从 meta 里提取 effects
 -> ThreadSession.record_tool_outcome
ToolResult 本身会写进 conversation/context/rollout，让下一轮 LLM 能看到工具结果。

effects 是 runtime 额外要更新的 live 状态，不只是给 LLM 看。比如：

meta.exec_session_id + lifecycle=running
 -> ExecSessionUpdate
 -> overlay.open_exec_sessions

meta.approval_id + approval_status=pending
 -> ApprovalUpdate::Requested
 -> overlay.pending_approvals
 -> ApprovalRequested event
approval effect 的作用主要有三个：

让 UI/API 知道现在有 pending approval
因为它会进入 overlay.pending_approvals，ThreadView 可以展示。

让 interrupt 能清理等待中的 approval
如果用户取消，runtime 会清 overlay，并让 PolicyManager 删除 pending command。

记录事件
会发 ApprovalRequested / ApprovalDecision，外部订阅者能知道发生了审批请求或审批结果。

所以简单说：

ToolResult = 给 LLM 和历史看的工具输出
ToolEffect = 给 runtime live state 用的副作用
这两个分开，是为了避免 runtime 去读自然语言输出，而是读结构化 meta。
