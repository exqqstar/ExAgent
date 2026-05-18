# ExAgent Session-Centered Agent Architecture

**Date:** 2026-05-18
**Status:** Implemented in current working tree
**Language:** Chinese study note

**Related local code:**

- `src/runtime/agent.rs`
- `src/runtime/thread_session/mod.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/thread_session/events.rs`
- `src/runtime/thread_runtime.rs`
- `src/app_server/thread_manager.rs`

**Related reference code:**

- `external-references/Codex/codex-rs/core/src/session/turn.rs`
- `external-references/Codex/codex-rs/core/src/session/mod.rs`
- `external-references/Codex/codex-rs/core/src/tools/parallel.rs`
- `external-references/Codex/codex-rs/core/src/stream_events_utils.rs`
- `external-references/gemini-cli-main 3/packages/core/src/core/client.ts`
- `external-references/gemini-cli-main 3/packages/core/src/core/geminiChat.ts`
- `external-references/gemini-cli-main 3/packages/core/src/core/turn.ts`
- `external-references/gemini-cli-main 3/packages/core/src/scheduler/scheduler.ts`
- `external-references/gemini-cli-main 3/packages/core/src/scheduler/tool-executor.ts`
- `external-references/gemini-cli-main 3/packages/core/src/agent/legacy-agent-session.ts`

## Executive Summary

这份文档整理 ExAgent agent/runtime 架构的目标方向，以及当前工作树已经落地的模块归位。

核心结论：

```text
ThreadSession 是唯一真实状态来源。
Turn loop 可以拆，tool execution 可以拆，但 history/snapshot/event 的写入点不能分裂。
```

重构前的 `Agent` 承担了太多职责：

```text
legacy Agent live-turn runner
  -> clone snapshot.conversation 到 messages
  -> 调模型
  -> 执行工具
  -> mutate snapshot.conversation
  -> mutate snapshot.open_exec_sessions
  -> mutate snapshot.pending_approvals
  -> 通过 LiveEventSink 写 assistant/tool/approval events
```

这让 `Agent` 同时像：

- model runner
- turn loop
- tool executor
- snapshot mutator
- event producer

当前实现已经收敛到更接近 Codex 的结构：

```text
ThreadRuntime
  -> ThreadRuntimeLoop
  -> ThreadSession
      -> run_turn / TurnLoop
          -> ModelClient sampling
          -> ToolCallRuntime execution
          -> ThreadSession records history/events/state
```

`ToolCallRuntime` 是工具执行运行时，不是新的状态中心。

当前源码板块：

```text
entrypoints/  -> CLI / HTTP adapters
app_server/   -> typed boundary, protocol, thread manager
runtime/      -> thread actor, ThreadSession, Agent sampling, ToolCallRuntime, policy, exec sessions
tools/        -> Tool trait, registry, built-in tools
state/        -> SessionSnapshot, RuntimeEvent, transcript persistence
model/        -> LLM adapter and conversation/tool-call model
```

## Terms

### Sampling

本文里的 `sampling` 指一次模型调用，也就是一次 model inference / streaming request。

一个用户 turn 里面可能有多次 sampling：

```text
user input
  -> sampling #1
      model returns tool call
  -> execute tool
  -> record tool output
  -> sampling #2
      model sees tool output and continues
  -> final assistant message
```

所以：

```text
turn != sampling
```

一个 turn 是用户发起的一轮工作；一个 turn 内部可能包含多次模型请求。

### Source Of Truth

`source of truth` 指某类状态唯一可信的存储位置。

对 ExAgent 的目标架构来说：

```text
conversation history     -> ThreadSession
SessionSnapshot          -> ThreadSession
pending approvals        -> ThreadSession
open exec sessions       -> ThreadSession
runtime events           -> ThreadSession / ThreadEventRecorder
tool execution state     -> ToolCallRuntime, but committed results go back to ThreadSession
prompt request payload   -> derived from ThreadSession, not stored as state
```

### Prompt Payload Vs Working Conversation

需要区分两种“复制 history”。

正常的复制：

```text
prompt = ThreadSession.history_for_prompt()
ModelClient.complete(prompt, tool_schemas)
drop(prompt)
```

这只是从真实 history 派生一次请求 payload。它是只读、短生命周期、用完就丢。

有风险的复制：

```text
let mut messages = snapshot.conversation.clone();
loop {
  llm.complete(&messages, ...)
  messages.push(...)
  snapshot.conversation.push(...)
}
```

这变成第二份会持续变化的 working conversation。它和 `snapshot.conversation` 必须手动保持一致，一旦新增特殊路径、错误路径、approval 路径、interrupt 路径，就容易漂移。

重构前 `Agent` 里的 `messages` 属于第二种。

## Current ExAgent Architecture

当前整体形态已经从早期 request-scoped agent execution 进化到了 runtime actor，并且 turn loop 已经收归 `ThreadSession`：

```text
CLI / HTTP API
  -> AppServerService
  -> ThreadManager
  -> ThreadRuntime
  -> ThreadRuntimeLoop
  -> ThreadSession
      -> Agent sampling
      -> ToolCallRuntime
          -> ToolRegistry / Tools
```

当前职责大致是：

```text
ThreadManager
  - process-level loaded runtime registry
  - app-server boundary handling
  - config override / thread creation / runtime lookup

ThreadRuntime
  - live thread handle
  - mailbox sender
  - active turn reservation
  - interrupt/status/live_view surface

ThreadRuntimeLoop
  - serializes ThreadOp for one thread
  - owns ThreadSession while loop is alive

ThreadSession
  - owns loaded thread live_state
  - owns ThreadEventRecorder
  - records lifecycle events
  - owns turn loop
  - applies tool effects into snapshot/history/events

ThreadEventRecorder
  - assigns event id
  - writes snapshot checkpoint
  - appends event JSONL
  - updates live_state
  - broadcasts RuntimeEvent

Agent
  - owns LLM client
  - owns ToolRegistry
  - owns ExecSessionManager reference
  - owns PolicyManager reference
  - samples assistant turns
  - constructs per-turn ToolCallRuntime

ToolCallRuntime
  - owns per-turn tool execution context
  - dispatches ToolRegistry
  - converts tool result metadata into typed effects
```

这一层结构的方向是对的：`ThreadRuntime` 串行化 thread ops，`ThreadSession` 是唯一 live state writer，`Agent` 和 `ToolCallRuntime` 都只是执行组件。

## Current Pain Points

### 1. Agent Has Two Conversation Views

重构前的 legacy Agent live-turn runner 会从 snapshot clone 出一份 `messages`：

```text
let mut messages = snapshot.conversation.clone();
```

之后 assistant/tool message 会同时 append 到：

```text
messages
snapshot.conversation
```

这带来一个隐含约束：所有路径必须同时更新两份数据。如果后续增加：

- tool call item 和 tool output item 分开记录
- approval wait/resume
- cancelled tool result
- background exec session continuation
- compaction item
- injected context item
- hook continuation message

这两份 conversation 很容易不同步。

目标上，prompt payload 应该每次 sampling 前从 `ThreadSession` 派生，而不是在 `Agent` 内维护一份持续增长的 local conversation。

### 2. Agent Mutates Snapshot

当前 `Agent` 会直接改：

```text
snapshot.conversation
snapshot.open_exec_sessions
snapshot.pending_approvals
```

这和 `ThreadSession is authoritative` 的目标冲突。

如果 `Agent` 可以改 snapshot，那么后续 state API 很难收敛：

```text
ThreadSession.record_assistant_turn(...)
ThreadSession.record_tool_result(...)
ThreadSession.request_approval(...)
ThreadSession.update_exec_session(...)
```

这些 API 的价值就在于所有状态变更都经过同一条路径。

### 3. LiveEventSink Creates Reverse Dependency

`src/runtime/agent.rs` 依赖：

```rust
use crate::runtime::thread_session::LiveEventSink;
```

这说明底层 execution 组件反过来知道 runtime/session event sink。

更理想的边界是：

```text
TurnLoop / ToolCallRuntime
  -> returns structured result/effects
ThreadSession
  -> applies effects
  -> records events
```

或者，如果执行函数仍然需要实时发送事件，也应该拿一个更窄的 `SessionRuntimeApi`，而不是直接拿 `LiveEventSink` 和 `&mut SessionSnapshot`。

### 4. Tool Metadata Is Parsed Back Into Session State

当前 exec session 和 approval 更新通过 `ToolResult.meta` 字段回读：

```text
meta["exec_session_id"]
meta["lifecycle"]
meta["approval_id"]
meta["approval_status"]
```

这可以工作，但它把强类型状态更新藏在 JSON meta 字符串里。后续更好的方式是工具执行返回结构化 effects：

```rust
enum ToolEffect {
    ExecSessionStarted { ... },
    ExecSessionEnded { ... },
    ApprovalRequested { ... },
    ApprovalResolved { ... },
    ConversationItem { ... },
}
```

然后由 `ThreadSession` apply。

## Codex Reference

Codex 的结构更接近我们应该走的方向：

```text
app-server
  -> ThreadManager
  -> CodexThread
  -> Codex
  -> submission_loop
  -> Session
  -> run_turn
```

关键不是类型名字，而是职责：

```text
Session
  - owns live state
  - owns history
  - owns event delivery
  - owns persistence path

run_turn
  - receives Arc<Session>
  - builds prompt from Session history
  - calls model
  - dispatches tools through ToolCallRuntime
  - records every committed item back through Session

ToolCallRuntime
  - per-turn tool execution context
  - dispatches tool calls
  - controls cancellation and parallelism
  - does not become conversation owner
```

Codex 每次 sampling 前会从 `Session` clone history 并构造 prompt：

```text
sess.clone_history()
  .await
  .for_prompt(...)
```

模型输出完成后，Codex 会立刻记录 response item：

```text
record_completed_response_item(...)
  -> sess.record_conversation_items(...)
```

如果 response item 是 tool call，则会：

```text
record model tool call item into Session history
start tool future through ToolCallRuntime
```

工具完成后，再把 tool output 记录回 Session history：

```text
drain_in_flight(...)
  -> response_input.into()
  -> sess.record_conversation_items(...)
```

所以 Codex 的写入点不只有工具完成。它会记录：

- accepted user input
- context updates
- skill/plugin/context injection items
- assistant message items
- reasoning items
- tool call items
- tool output items
- hook continuation prompt
- warnings/synthetic messages
- compaction/context management items

但这些写入都回到同一个 `Session`。

这就是我们要复制的核心：不是复制 Codex 所有类型，而是复制它的 ownership model。

## Gemini CLI Reference

Gemini CLI 的结构更偏 client/chat object：

```text
CLI / React UI
  -> GeminiClient
      -> GeminiChat
          -> AgentChatHistory
      -> Turn
  -> Scheduler
      -> SchedulerStateManager
      -> ToolExecutor
```

职责：

```text
GeminiChat
  - owns AgentChatHistory
  - user/functionResponse enters history before model request
  - model response enters history after stream completion

Turn
  - adapts model stream into events
  - emits ToolCallRequest
  - does not execute tools

Scheduler
  - owns tool lifecycle state
  - handles validate/policy/approval/parallel/cancel
  - emits tool state updates through MessageBus

ToolExecutor
  - executes a single tool
  - converts result to functionResponse parts
```

Gemini 的 source of truth 是 `GeminiChat.agentHistory`。

普通 loop 是：

```text
currentParts = user input
GeminiClient.sendMessageStream(currentParts)
  -> GeminiChat pushes user content to history
  -> model emits tool call
  -> GeminiChat pushes model response to history
Scheduler executes tools
currentParts = tool functionResponse parts
GeminiClient.sendMessageStream(currentParts)
  -> GeminiChat pushes functionResponse to history
  -> model continues
```

Gemini 也不是在 `Turn` 里维护一整份 working conversation。`currentParts` 只是下一次要发给模型的输入，完整上下文在 `GeminiChat.agentHistory`。

对 ExAgent 的启发：

```text
可以拆 Turn 和 ToolScheduler，但一定要有一个 history owner。
```

不过 ExAgent 现在已有 `ThreadRuntime -> ThreadSession` actor 形态，更接近 Codex。因此不建议直接照搬 Gemini 的完整 `Scheduler` 和 `GeminiChat` 分层。我们更适合 Codex-like 的 `ThreadSession + ToolCallRuntime`。

## Target Architecture

目标架构：

```text
AppServer / CLI
  -> ThreadManager
  -> ThreadRuntime
      -> runtime mailbox
      -> active turn
      -> interrupt
      -> status
      -> live view
  -> ThreadRuntimeLoop
      -> serializes ThreadOp
  -> ThreadSession
      -> source of truth
      -> TurnLoop
          -> ModelClient sampling
          -> ToolCallRuntime execution
          -> ThreadSession applies effects
```

更详细：

```text
ThreadManager
  - loaded runtime registry
  - thread start/resume/read boundary
  - no direct Agent execution

ThreadRuntime
  - live handle
  - submit user input
  - interrupt active turn
  - subscribe events
  - expose live_view
  - no conversation mutation

ThreadRuntimeLoop
  - owns ThreadSession
  - serializes operations
  - dispatches op to ThreadSession
  - no model/tool business logic

ThreadSession
  - owns SessionSnapshot
  - owns conversation history
  - owns pending approvals
  - owns open exec sessions
  - owns event recording
  - owns persistence checkpoint
  - owns public state mutation API

TurnLoop
  - one user turn internal control flow
  - while model needs follow-up:
      prompt = ThreadSession.history_for_prompt()
      output = ModelClient.sample(prompt, tools)
      ThreadSession.record_model_output(output)
      ToolCallRuntime.execute(tool calls)
      ThreadSession.record_tool_outputs(...)

ModelClient
  - calls LLM provider
  - returns model output/tool calls
  - no snapshot/history mutation

ToolCallRuntime
  - per-turn tool runtime
  - dispatches ToolRegistry
  - handles cancellation
  - handles policy/approval
  - handles parallel/sequential constraints
  - returns ToolResult/ToolEffect
  - no durable history ownership

ThreadEventRecorder
  - assign event id
  - write snapshot checkpoint
  - append event JSONL
  - update live_state
  - broadcast RuntimeEvent
```

## Target Turn Flow

Target user turn:

```text
ThreadRuntime.submit_user_input(...)
  -> ThreadRuntimeLoop receives ThreadOp::UserInput
  -> ThreadSession.handle_user_input(...)
```

Inside `ThreadSession`:

```text
1. record user message
   snapshot.conversation.push(user message)
   record RuntimeEventKind::TurnStarted

2. create TurnContext
   turn_id
   cwd
   config
   cancellation token
   tool schemas
   policy references

3. create ToolCallRuntime for this turn

4. sampling loop
   prompt = self.history_for_prompt()
   model_output = model_client.sample(prompt, tool_schemas)

5. record model output
   if assistant text:
      self.record_assistant_message(...)
   if tool call:
      self.record_tool_call(...)

6. if no tool call
      record TurnCompleted
      return final assistant turn

7. execute tool calls
   tool_outputs = tool_runtime.execute_all(tool_calls)

8. record tool outputs
   self.record_tool_result(...)
   self.apply_tool_effects(...)

9. continue loop
   next prompt is derived again from self.history_for_prompt()
```

Important invariant:

```text
Every committed item is in ThreadSession before the next sampling request.
```

That means model context cannot drift from runtime state.

## Proposed APIs

The exact Rust names can change, but the ownership should look like this.

### ThreadSession State API

```rust
impl ThreadSession {
    fn history_for_prompt(&self) -> Vec<ConversationMessage>;

    fn record_user_message(
        &mut self,
        turn_id: &TurnId,
        message: ConversationMessage,
    ) -> Result<()>;

    fn record_assistant_turn(
        &mut self,
        turn_id: &TurnId,
        turn: AssistantTurn,
    ) -> Result<()>;

    fn record_tool_call(
        &mut self,
        turn_id: &TurnId,
        call: ToolCall,
    ) -> Result<()>;

    fn record_tool_result(
        &mut self,
        turn_id: &TurnId,
        result: ToolResult,
    ) -> Result<()>;

    fn apply_tool_effects(
        &mut self,
        turn_id: &TurnId,
        effects: Vec<ToolEffect>,
    ) -> Result<()>;
}
```

The point is not the exact method list. The point is that state changes are named operations on `ThreadSession`, not anonymous mutation in `Agent`.

### TurnLoop Shape

`TurnLoop` can initially be a method in `ThreadSession`:

```rust
impl ThreadSession {
    async fn run_turn_loop(
        &mut self,
        turn_context: TurnContext,
    ) -> Result<AssistantTurn> {
        let tool_runtime = ToolCallRuntime::new(...);

        for _ in 0..self.config.max_turns {
            let prompt = self.history_for_prompt();
            let model_output = self.model_client.complete(&prompt, tool_runtime.schemas()).await?;

            self.record_assistant_turn(&turn_context.turn_id, model_output.clone())?;

            if model_output.tool_calls.is_empty() {
                return Ok(model_output);
            }

            let results = tool_runtime.execute_all(model_output.tool_calls).await?;
            for result in results {
                self.record_tool_result(&turn_context.turn_id, result)?;
            }
        }

        Err(anyhow!("turn reached max turns without final assistant output"))
    }
}
```

Later it can become a separate `TurnLoop` struct if the function grows, but it should still operate through a `ThreadSession` API.

### ToolCallRuntime Shape

```rust
pub(crate) struct ToolCallRuntime {
    registry: ToolRegistry,
    exec_sessions: Arc<ExecSessionManager>,
    policy: Arc<PolicyManager>,
    turn_context: Arc<TurnContext>,
    parallel_execution: Arc<RwLock<()>>,
}

impl ToolCallRuntime {
    async fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
    ) -> Result<ToolExecutionOutcome>;
}

pub(crate) struct ToolExecutionOutcome {
    result: ToolResult,
    effects: Vec<ToolEffect>,
}
```

`ToolCallRuntime` 可以知道 `TurnContext`，也可以知道 policy、exec sessions、tool registry。它不应该拥有 durable conversation history。

## Before And After

### Before

```text
ThreadSession
  -> clones snapshot
  -> pushes user message
  -> records TurnStarted
  -> calls legacy Agent live-turn runner(&mut snapshot, sink)

Agent
  -> clones snapshot.conversation into messages
  -> loops:
      llm.complete(&messages)
      messages.push(assistant)
      snapshot.conversation.push(assistant)
      sink.record(AssistantTurn)
      ToolRegistry.execute(...)
      parse ToolResult.meta
      snapshot.open_exec_sessions/pending_approvals mutation
      messages.push(tool result)
      snapshot.conversation.push(tool result)
      sink.record(ToolResult)

ThreadSession
  -> records TurnCompleted
```

### After

```text
ThreadSession
  -> pushes user message
  -> records TurnStarted
  -> owns turn loop

TurnLoop
  -> prompt = ThreadSession.history_for_prompt()
  -> model_output = ModelClient.complete(prompt, schemas)
  -> ThreadSession.record_assistant_turn(model_output)
  -> ToolCallRuntime.execute(...)
  -> ThreadSession.record_tool_result(...)
  -> prompt = ThreadSession.history_for_prompt()
  -> repeat until final assistant

ThreadSession
  -> records TurnCompleted
```

The big difference:

```text
Before: Agent mutates snapshot and keeps local messages.
After: ThreadSession mutates state; prompt is only a derived payload.
```

## Why This Design

### Reason 1: One State Owner

Thread state is not only conversation text. It includes:

- event ids
- live event buffer
- persisted event log
- pending approvals
- open exec sessions
- status
- cwd
- lineage
- future compaction state
- future tool batch state

If `Agent` writes some of these and `ThreadSession` writes others, then the architecture has no single authority.

Putting all mutation behind `ThreadSession` gives one place to reason about correctness.

### Reason 2: Prompt Context Cannot Drift

If every model request derives prompt from `ThreadSession.history_for_prompt()`, then model context is always based on committed state.

The sequence becomes:

```text
commit item
derive prompt
model call
commit item
derive next prompt
```

There is no hidden local conversation that may diverge.

### Reason 3: Tool Runtime Can Grow Without Taking Over Session

Tool execution will likely grow:

- parallel execution
- approval waits
- cancellation
- background commands
- shell session continuation
- sandbox retry
- policy updates
- MCP tools
- subagents

This deserves a `ToolCallRuntime`.

But making `ToolCallRuntime` a state owner would recreate the same problem. It should be an execution runtime whose committed outputs return to `ThreadSession`.

### Reason 4: Codex Alignment

Codex is a stronger match than Gemini for ExAgent because ExAgent already has:

- runtime actor
- per-thread session
- event stream
- persisted transcript
- app-server boundary

Gemini is useful as a contrast, especially its clean split of `Turn` and `Scheduler`, but its source of truth is `GeminiChat`, not a runtime `Session`.

For ExAgent, `ThreadSession` should be the `GeminiChat.agentHistory` equivalent and the Codex `Session` equivalent.

## Tradeoffs

### Benefit: Simpler Correctness Model

The main benefit is conceptual:

```text
If state changed, ThreadSession did it.
If a model saw context, it came from ThreadSession.
If an event was emitted, ThreadSession recorded it.
```

This makes debugging and replay easier.

### Benefit: Better Live UI Behavior

A future GUI or app-server client wants stable event semantics:

```text
TurnStarted
AssistantTurn / AssistantDelta
ToolCallStarted
ApprovalRequested
ToolResult
TurnCompleted
```

If `ThreadSession` owns event ordering, live view and replay can stay consistent.

### Benefit: Tool Runtime Can Evolve

`ToolCallRuntime` gives a dedicated place for tool lifecycle policy without making `Agent` larger.

This is especially useful once tools need:

- running state
- awaiting approval state
- cancellation state
- per-call telemetry
- parallel scheduling
- stateful exec sessions

### Cost: More Plumbing

Moving from `legacy Agent live-turn runner(&mut snapshot, sink)` to `ThreadSession` state APIs will add explicit methods and more structured data types.

Short term, this is more code.

Long term, it removes implicit mutation and JSON meta parsing.

### Cost: ThreadSession Can Become Too Large

If all logic moves blindly into `ThreadSession`, it may become a god object.

The guardrail is:

```text
ThreadSession owns state mutation.
TurnLoop owns control flow.
ToolCallRuntime owns tool execution.
ModelClient owns model calls.
```

The code can be split into modules, but the authority boundary remains `ThreadSession`.

### Cost: Effects Need Careful Design

Returning `ToolEffect` / `TurnEffect` is cleaner than mutating snapshot, but effects must be typed well.

Bad effect design would become another unstructured event bus. Good effect design should model actual domain changes:

```text
ConversationAppended
ApprovalRequested
ApprovalResolved
ExecSessionOpened
ExecSessionClosed
ToolResultRecorded
```

## Why Not Full Gemini Scheduler Now

Gemini's Scheduler is powerful:

```text
Scheduler
  -> SchedulerStateManager
  -> MessageBus
  -> ToolModificationHandler
  -> ToolExecutor
```

It is useful when tool state is a UI-first live object independent of session history.

ExAgent does not need that whole shape yet. We can get most of the correctness benefit with a smaller Codex-like runtime:

```text
ToolCallRuntime
  -> execute calls
  -> enforce parallel/serial rules
  -> handle approval/policy/cancel
  -> return results/effects
```

If future UI needs richer live tool batch semantics, we can add a `ToolCallStateManager` inside `ToolCallRuntime` later. We should not start with the full scheduler unless the product surface needs it.

## Why Not Keep Agent As The Core Type

Keeping `Agent` as the core type is tempting because it already has:

- LLM client
- tool registry
- config
- policy
- exec session manager

But a core type should own one coherent domain.

`Agent` currently owns execution mechanics and session mutation. Those are different domains:

```text
execution mechanics
  - call model
  - detect tool call
  - execute tool
  - loop

session mutation
  - append history
  - assign event ids
  - persist snapshot/events
  - update approvals
  - update open exec sessions
```

The second group belongs to `ThreadSession`.

So `Agent` can remain as a thin facade temporarily, but the stable target should be:

```text
Agent -> removed, renamed, or reduced
ModelClient -> model calls
TurnLoop -> per-turn loop
ToolCallRuntime -> tool execution
ThreadSession -> state owner
```

## Migration Plan

### Phase 1: Move Snapshot Mutation Back To ThreadSession

Goal:

```text
Agent no longer mutates SessionSnapshot directly.
```

Actions:

- Add `ThreadSession` methods for assistant/tool/approval/exec session updates.
- Change `legacy Agent live-turn runner` to return structured turn steps or effects.
- Remove direct `snapshot.conversation.push(...)` from `Agent`.
- Remove direct `snapshot.open_exec_sessions` mutation from `Agent`.
- Remove direct `snapshot.pending_approvals` mutation from `Agent`.

Expected shape:

```text
Agent emits:
  AgentStep::AssistantTurn(...)
  AgentStep::ToolResult(...)
  AgentStep::ApprovalRequested(...)

ThreadSession applies:
  append history
  update snapshot
  record event
```

### Phase 2: Remove Long-Lived Local Messages

Goal:

```text
No mutable working conversation inside Agent.
```

Actions:

- Replace `let mut messages = snapshot.conversation.clone()` with per-sampling prompt derivation.
- Prompt is built by `ThreadSession.history_for_prompt()`.
- The turn loop asks for fresh prompt before each model call.

Expected invariant:

```text
No code appends to both local messages and snapshot.conversation.
```

### Phase 3: Introduce ToolCallRuntime

Goal:

```text
Tool execution is centralized without becoming state ownership.
```

Actions:

- Create `src/runtime/tool_call_runtime.rs` or `src/tools/runtime.rs`.
- Move `ToolRegistry.execute(...)` orchestration there.
- Move policy/approval/exec session effect creation there.
- Add typed `ToolExecutionOutcome`.

Expected shape:

```text
ToolCallRuntime::execute(call) -> ToolExecutionOutcome {
  result,
  effects,
}
```

### Phase 4: Re-home Turn Loop Under ThreadSession

Goal:

```text
ThreadSession drives the turn loop.
```

Actions:

- Move `for _ in 0..max_turns` loop from `Agent` into `ThreadSession` or `ThreadSession::turn`.
- `Agent` becomes a model adapter or disappears.
- `ThreadSession` controls when each item is committed and when the next sampling starts.

Expected final path:

```text
ThreadSession::handle_user_input
  -> ThreadSession::run_turn_loop
      -> ModelClient
      -> ToolCallRuntime
```

### Phase 5: Clean Up Old Compatibility Surface

Goal:

```text
Names and docs match reality.
```

Actions:

- Rename `Agent` if it no longer represents an agent.
- Remove `LiveEventSink` dependency from model/tool execution modules.
- Replace JSON `ToolResult.meta` parsing with typed effects.
- Update architecture docs and tests.

## Testing Strategy

Important tests should assert behavior, not just type boundaries.

### History Source Tests

Verify that each sampling request is built from committed `ThreadSession` history.

Example:

```text
user message committed
assistant tool call committed
tool result committed
second sampling sees all three items
```

### No Duplicate Conversation Tests

Assert that a tool loop grows conversation monotonically through `ThreadSession`, without local duplication or missing tool results.

### Event Ordering Tests

Expected order:

```text
TurnStarted
AssistantTurn / ToolCall
ApprovalRequested? / ApprovalDecision?
ToolResult
AssistantTurn
TurnCompleted
```

### Interrupt Tests

Verify cancellation while:

- model is streaming
- tool is executing
- approval is pending

The persisted state and live view should agree.

### Replay Tests

Persisted event log and live view should reconstruct the same meaningful turn state.

### Architecture Guard Searches

Useful checks:

```bash
rg "snapshot\\.conversation\\.push|open_exec_sessions|pending_approvals" src/runtime/agent.rs
rg "LiveEventSink" src/runtime/agent.rs src/tools src/model/llm
rg "approval_status|exec_session_id|lifecycle" src/runtime/agent.rs
```

Eventually these should disappear or move behind typed session APIs.

## Final Target Invariants

The final architecture should satisfy:

1. `ThreadSession` is the only runtime state writer.
2. `ThreadSession` is the only history owner.
3. `ThreadSession` is the only event recorder for live turns.
4. Every sampling request derives prompt from committed `ThreadSession` history.
5. `ModelClient` does not know `SessionSnapshot`.
6. `ToolCallRuntime` does not own durable history.
7. Tool results become typed outcomes/effects, not JSON meta side channels.
8. Interrupt, approval, and replay use the same state/event path.

If these invariants hold, ExAgent can support richer GUI, background tools,
approval resume, subagents, and context compaction without repeatedly changing
the core ownership model.
