# Per-Turn Reasoning Effort Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a per-turn `reasoning_effort` override so clients can switch thinking depth per request without mutating global runtime configuration.

**Architecture:** Treat `reasoning_effort` as turn-scoped request metadata, parallel to `turn_context.cwd`. The value flows from protocol DTOs through `ThreadTurnContext` into `LlmRequestOptions`, and only the OpenAI-compatible chat-completions adapter serializes it as `reasoning_effort`. Persist the selected value in `TurnContextItem` for replay/debugging, but do not use mutable global `AgentConfig` for per-turn state.

**Tech Stack:** Rust, serde, async_trait, axum protocol DTOs, rollout persistence, OpenAI-compatible `/chat/completions` adapter.

---

## File Structure

- Modify `src/model/types.rs`: add `ReasoningEffort` and `LlmRequestOptions`.
- Modify `src/model/llm.rs`: extend `LlmClient` with option-aware completion, serialize `reasoning_effort` when set, keep existing `complete` compatibility.
- Modify `src/app_server/protocol.rs`: add `reasoning_effort` to `TurnContextOverrides`.
- Modify `src/runtime/thread_runtime.rs`: add `reasoning_effort` to `ThreadTurnContext`.
- Modify `src/runtime/agent.rs`: pass `LlmRequestOptions` into the LLM client.
- Modify `src/runtime/thread_session/turn.rs`: preserve the full turn context, pass LLM options into each assistant sample, and persist reasoning effort in turn context.
- Modify `src/runtime/context.rs`: add reasoning effort to `PromptContext::for_turn` and `TurnContextItem`.
- Modify `src/state/session.rs`: persist optional `reasoning_effort` on `TurnContextItem` with serde defaults for old rollouts.
- Modify `src/app_server/thread_manager.rs`: convert protocol overrides into `ThreadTurnContext` without losing effort-only turns.
- Modify `docs/protocol/app-server-boundary-v2.md`: document the new request field.
- Test `tests/api_server.rs`: HTTP route accepts `turn_context.reasoning_effort`.
- Test `tests/app_server_boundary.rs`: per-turn value is persisted without mutating thread cwd.
- Test `src/model/llm.rs`: request serialization includes/omits `reasoning_effort`.
- Test `src/runtime/thread_session/turn.rs`: LLM options receive per-turn values and do not leak to the next turn.

---

### Task 1: Shared Types And Protocol DTO

**Files:**
- Modify: `src/model/types.rs`
- Modify: `src/app_server/protocol.rs`
- Test: `tests/api_server.rs`

- [ ] **Step 1: Add the failing API route assertion**

In `tests/api_server.rs`, update `StubBoundary::turn_start` so it asserts the field is deserialized:

```rust
async fn turn_start(&self, params: TurnStartParams) -> anyhow::Result<TurnStartResponse> {
    self.calls.lock().unwrap().push("turn_start".into());
    assert_eq!(params.thread_id.as_str(), "session_123");
    assert_eq!(params.prompt, "continue phase2");
    assert_eq!(params.workspace_root.as_deref(), Some("."));
    assert_eq!(
        params
            .turn_context
            .as_ref()
            .and_then(|context| context.reasoning_effort),
        Some(exagent::types::ReasoningEffort::High)
    );
    Ok(self.turn_start_response.clone())
}
```

Then update `turn_start_route_accepts_thread_id_and_prompt` request body:

```rust
json!({
    "thread_id": "session_123",
    "prompt": "continue phase2",
    "workspace_root": ".",
    "turn_context": {
        "reasoning_effort": "high"
    }
})
```

- [ ] **Step 2: Run the failing API test**

Run:

```bash
cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt
```

Expected: compile failure because `ReasoningEffort` and `TurnContextOverrides::reasoning_effort` do not exist.

- [ ] **Step 3: Add `ReasoningEffort` and request options**

In `src/model/types.rs`, add after `LlmCompletion`:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmRequestOptions {
    pub reasoning_effort: Option<ReasoningEffort>,
}
```

In `src/app_server/protocol.rs`, import `ReasoningEffort` and extend `TurnContextOverrides`:

```rust
use crate::types::{EventId, ReasoningEffort, SessionId, ToolCall, TurnId};
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnContextOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}
```

- [ ] **Step 4: Fix existing struct literals**

Every existing `TurnContextOverrides { cwd: ... }` literal must add:

```rust
reasoning_effort: None,
```

Use:

```bash
rg -n "TurnContextOverrides \\{" src tests
```

Expected: all literals compile with both fields.

- [ ] **Step 5: Run the API test**

Run:

```bash
cargo test --test api_server turn_start_route_accepts_thread_id_and_prompt
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/model/types.rs src/app_server/protocol.rs tests/api_server.rs
git commit -m "feat: add per-turn reasoning effort protocol field"
```

---

### Task 2: LLM Adapter Serialization

**Files:**
- Modify: `src/model/llm.rs`
- Test: `src/model/llm.rs`

- [ ] **Step 1: Add failing serialization tests**

Add a `#[cfg(test)] mod tests` at the bottom of `src/model/llm.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmRequestOptions, ReasoningEffort};

    #[test]
    fn chat_completion_request_serializes_reasoning_effort_when_set() {
        let request = ChatCompletionRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![],
            tools: vec![],
            reasoning_effort: Some(ReasoningEffort::High),
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["reasoning_effort"], "high");
    }

    #[test]
    fn chat_completion_request_omits_reasoning_effort_when_unset() {
        let request = ChatCompletionRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![],
            tools: vec![],
            reasoning_effort: None,
        };

        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("reasoning_effort").is_none());
    }

    #[test]
    fn default_llm_request_options_do_not_set_reasoning_effort() {
        assert_eq!(LlmRequestOptions::default().reasoning_effort, None);
    }
}
```

- [ ] **Step 2: Run failing LLM tests**

Run:

```bash
cargo test llm::tests::chat_completion_request_serializes_reasoning_effort_when_set llm::tests::chat_completion_request_omits_reasoning_effort_when_unset
```

Expected: compile failure because `ChatCompletionRequest` has no `reasoning_effort`.

- [ ] **Step 3: Add option-aware LLM completion**

Update imports in `src/model/llm.rs`:

```rust
use crate::types::{
    AssistantTurn, ConversationMessage, LlmCompletion, LlmRequestOptions, MessageRole, TokenUsage,
    ToolCall,
};
```

Extend the trait without breaking existing mock implementations:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<LlmCompletion>;

    async fn complete_with_options(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        let _ = options;
        self.complete(messages, tools).await
    }
}
```

- [ ] **Step 4: Serialize `reasoning_effort` in chat-completions requests**

Extend `OpenAiCompatibleLlm` implementation:

```rust
async fn complete(
    &self,
    messages: &[ConversationMessage],
    tools: &[serde_json::Value],
) -> Result<LlmCompletion> {
    self.complete_with_options(messages, tools, &LlmRequestOptions::default())
        .await
}

async fn complete_with_options(
    &self,
    messages: &[ConversationMessage],
    tools: &[serde_json::Value],
    options: &LlmRequestOptions,
) -> Result<LlmCompletion> {
    let request = ChatCompletionRequest {
        model: self.model.clone(),
        messages: build_request_messages(messages)?,
        tools: build_request_tools(tools)?,
        reasoning_effort: options.reasoning_effort,
    };

    // keep the existing reqwest send/status/body/parse_response code unchanged
}
```

Extend `ChatCompletionRequest`:

```rust
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatRequestMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatRequestTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<crate::types::ReasoningEffort>,
}
```

- [ ] **Step 5: Run LLM tests**

Run:

```bash
cargo test llm::tests
cargo test --test llm_http
cargo test --test llm_mock
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/model/llm.rs
git commit -m "feat: serialize reasoning effort for chat completions"
```

---

### Task 3: Runtime Propagation And Persistence

**Files:**
- Modify: `src/state/session.rs`
- Modify: `src/runtime/context.rs`
- Modify: `src/runtime/thread_runtime.rs`
- Modify: `src/runtime/agent.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/app_server/thread_manager.rs`
- Test: `src/runtime/thread_session/turn.rs`
- Test: `tests/app_server_boundary.rs`

- [ ] **Step 1: Add a failing thread-session propagation test**

In `src/runtime/thread_session/turn.rs` test module, add a recording LLM that overrides `complete_with_options`:

```rust
struct ReasoningRecordingLlm {
    turns: AsyncMutex<VecDeque<AssistantTurn>>,
    efforts: Arc<Mutex<Vec<Option<ReasoningEffort>>>>,
}

#[async_trait]
impl LlmClient for ReasoningRecordingLlm {
    async fn complete(
        &self,
        messages: &[ConversationMessage],
        tools: &[serde_json::Value],
    ) -> Result<LlmCompletion> {
        self.complete_with_options(messages, tools, &LlmRequestOptions::default())
            .await
    }

    async fn complete_with_options(
        &self,
        _messages: &[ConversationMessage],
        _tools: &[serde_json::Value],
        options: &LlmRequestOptions,
    ) -> Result<LlmCompletion> {
        self.efforts.lock().unwrap().push(options.reasoning_effort);
        self.turns
            .lock()
            .await
            .pop_front()
            .map(AssistantTurn::into_completion)
            .ok_or_else(|| anyhow::anyhow!("ReasoningRecordingLlm is out of scripted turns"))
    }
}
```

Add this test:

```rust
#[tokio::test]
async fn thread_session_passes_reasoning_effort_only_for_the_current_turn() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("session_reasoning_effort");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };
    write_rollout_meta(&config, &thread_id);
    let efforts = Arc::new(Mutex::new(vec![]));
    let efforts_for_agent = efforts.clone();
    let agent_factory: AgentFactory = Arc::new(move |config| {
        Ok(Agent::new(
            config,
            Box::new(ReasoningRecordingLlm {
                turns: AsyncMutex::new(
                    vec![
                        AssistantTurn {
                            text: Some("high".into()),
                            tool_calls: vec![],
                        },
                        AssistantTurn {
                            text: Some("default".into()),
                            tool_calls: vec![],
                        },
                    ]
                    .into(),
                ),
                efforts: efforts_for_agent.clone(),
            }),
            ToolRegistry::new(),
        ))
    });
    let mut session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory,
    ))
    .expect("create thread session");

    session
        .handle_user_input(
            TurnId::new("turn_1"),
            "think harder".into(),
            Some(ThreadTurnContext {
                cwd: None,
                reasoning_effort: Some(ReasoningEffort::High),
            }),
            None,
        )
        .await
        .expect("run high effort turn");

    session
        .handle_user_input(TurnId::new("turn_2"), "default effort".into(), None, None)
        .await
        .expect("run default turn");

    assert_eq!(
        efforts.lock().unwrap().as_slice(),
        &[Some(ReasoningEffort::High), None]
    );
}
```

Update test imports:

```rust
use crate::types::{
    AssistantTurn, ConversationMessage, LlmCompletion, LlmRequestOptions, ReasoningEffort,
    SessionId, TokenUsage, ToolCall, TurnId,
};
```

- [ ] **Step 2: Run failing thread-session test**

Run:

```bash
cargo test thread_session_passes_reasoning_effort_only_for_the_current_turn
```

Expected: compile failure because runtime context and LLM options are not wired.

- [ ] **Step 3: Persist reasoning effort in turn context**

In `src/state/session.rs`, extend `TurnContextItem`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reasoning_effort: Option<crate::types::ReasoningEffort>,
```

In `src/runtime/context.rs`, update `PromptContext::for_turn`:

```rust
pub(crate) fn for_turn(
    config: &AgentConfig,
    paths: TurnPaths,
    reasoning_effort: Option<crate::types::ReasoningEffort>,
) -> Self {
    Self {
        turn_context: TurnContextItem {
            workspace_root: paths.workspace_root,
            cwd: paths.cwd,
            model: config.model.clone(),
            policy_mode: config.policy_mode,
            command_timeout_secs: config.command_timeout_secs,
            max_output_bytes: config.max_output_bytes,
            current_utc_date: Some(current_utc_date()),
            reasoning_effort,
        },
    }
}
```

Update all existing `TurnContextItem` literals in tests to include:

```rust
reasoning_effort: None,
```

- [ ] **Step 4: Add runtime context field**

In `src/runtime/thread_runtime.rs`, update:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTurnContext {
    pub cwd: Option<PathBuf>,
    pub reasoning_effort: Option<crate::types::ReasoningEffort>,
}
```

Update all `ThreadTurnContext { cwd: ... }` literals to include `reasoning_effort`.

- [ ] **Step 5: Pass options through `Agent`**

In `src/runtime/agent.rs`, import `LlmRequestOptions` and change `sample_assistant_turn`:

```rust
pub(crate) async fn sample_assistant_turn(
    &self,
    prompt: &[ConversationMessage],
    tool_schemas: &[serde_json::Value],
    options: &LlmRequestOptions,
) -> Result<LlmCompletion> {
    self.llm
        .complete_with_options(prompt, tool_schemas, options)
        .await
}
```

- [ ] **Step 6: Preserve full turn context in `ThreadSession`**

In `src/runtime/thread_session/turn.rs`, avoid consuming `turn_context` at the start:

```rust
let turn_cwd = turn_context.as_ref().and_then(|context| context.cwd.clone());
let request_options = LlmRequestOptions {
    reasoning_effort: turn_context
        .as_ref()
        .and_then(|context| context.reasoning_effort),
};
```

Update calls to `record_user_turn_start`:

```rust
self.record_user_turn_start(
    &turn_id,
    prompt,
    turn_cwd,
    request_options.reasoning_effort,
    &mut snapshot,
)?;
```

Update `run_session_turn` signature and calls:

```rust
request_options: LlmRequestOptions,
```

Inside the loop:

```rust
let completion = match agent
    .sample_assistant_turn(&prompt, &tool_runtime.schemas(), &request_options)
    .await
{
    // keep existing match arms
};
```

For retry after context-window compaction, use the same `request_options` for the retry assistant sample.

Update `record_user_turn_start` signature and `PromptContext::for_turn` call:

```rust
fn record_user_turn_start(
    &mut self,
    turn_id: &TurnId,
    prompt: String,
    turn_cwd: Option<PathBuf>,
    reasoning_effort: Option<ReasoningEffort>,
    snapshot: &mut SessionSnapshot,
) -> Result<()>
```

```rust
let prompt_context = PromptContext::for_turn(
    self.agent.config(),
    TurnPaths {
        workspace_root: snapshot.workspace_root.clone(),
        cwd: context_cwd,
    },
    reasoning_effort,
);
```

In `restore_retry_context_after_compaction`, pass `None` unless the function is extended to accept the same reasoning effort. Prefer extending it:

```rust
reasoning_effort: Option<ReasoningEffort>,
```

Then call `PromptContext::for_turn(..., reasoning_effort)`.

- [ ] **Step 7: Convert protocol overrides in `ThreadManager`**

In `src/app_server/thread_manager.rs`, add a helper:

```rust
fn resolve_thread_turn_context(
    snapshot: &SessionSnapshot,
    overrides: Option<crate::app_server::protocol::TurnContextOverrides>,
) -> Result<Option<ThreadTurnContext>> {
    let Some(overrides) = overrides else {
        return Ok(None);
    };
    let reasoning_effort = overrides.reasoning_effort;
    let resolved = OverridePolicy::apply_turn_context(snapshot, overrides)?;
    Ok(Some(ThreadTurnContext {
        cwd: Some(resolved.cwd),
        reasoning_effort,
    }))
}
```

Use it in both `run_turn_through_runtime` and `start_turn_in_background`:

```rust
let turn_context = resolve_thread_turn_context(&live_view.snapshot, params.turn_context)?;
```

Then pass `turn_context` directly to `submit_user_input_and_wait` / `submit_user_input`.

- [ ] **Step 8: Add boundary persistence test**

In `tests/app_server_boundary.rs`, add:

```rust
#[tokio::test]
async fn turn_start_persists_reasoning_effort_in_turn_context() {
    let dir = tempdir().unwrap();
    let service = AppServerService::with_llm(
        AgentConfig {
            workspace_root: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            ..AgentConfig::default()
        },
        Box::new(MockLlm::new(vec![AssistantTurn {
            text: Some("reasoned".into()),
            tool_calls: vec![],
        }])),
        ToolRegistry::new,
    );

    let thread = service
        .thread_start(ThreadStartParams {
            workspace_root: None,
            cwd: None,
        })
        .unwrap();
    let turn = service
        .turn_start(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            prompt: "use high effort".into(),
            workspace_root: None,
            turn_context: Some(TurnContextOverrides {
                cwd: None,
                reasoning_effort: Some(exagent::types::ReasoningEffort::High),
            }),
        })
        .await
        .unwrap();
    wait_for_turn_completed(&service, &thread.thread.id, &turn.turn.id).await;

    let snapshot = read_thread_snapshot(&thread.thread);
    assert_eq!(
        snapshot
            .reference_turn_context
            .as_ref()
            .and_then(|context| context.reasoning_effort),
        Some(exagent::types::ReasoningEffort::High)
    );
}
```

- [ ] **Step 9: Run focused runtime tests**

Run:

```bash
cargo test thread_session_passes_reasoning_effort_only_for_the_current_turn
cargo test --test app_server_boundary turn_start_persists_reasoning_effort_in_turn_context
cargo test --test app_server_boundary turn_start_applies_validated_context_override_with_user_input
cargo test --test app_server_boundary turn_context_cwd_is_used_for_tools_without_becoming_thread_cwd
```

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add src/state/session.rs src/runtime/context.rs src/runtime/thread_runtime.rs src/runtime/agent.rs src/runtime/thread_session/turn.rs src/app_server/thread_manager.rs tests/app_server_boundary.rs
git commit -m "feat: pass reasoning effort through runtime turns"
```

---

### Task 4: Protocol Documentation And Full Verification

**Files:**
- Modify: `docs/protocol/app-server-boundary-v2.md`
- Verify: full test suite

- [ ] **Step 1: Document the new field**

In `docs/protocol/app-server-boundary-v2.md`, update the `/turn/start` example:

```json
{
  "thread_id": "session_...",
  "prompt": "Summarize this runtime.",
  "workspace_root": ".",
  "turn_context": {
    "cwd": "src",
    "reasoning_effort": "high"
  }
}
```

Add this paragraph after the `turn_context.cwd` paragraph:

```markdown
`turn_context.reasoning_effort` is optional. When present, it applies only to
model calls for that user turn and is persisted in the rollout turn context for
debugging and replay inspection. Supported protocol values are `none`,
`minimal`, `low`, `medium`, `high`, and `xhigh`. If omitted, ExAgent does not
send a reasoning-effort override and lets the model provider use its default.
```

- [ ] **Step 2: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: PASS. If it fails, run:

```bash
cargo fmt
```

Then re-run `cargo fmt --check`.

- [ ] **Step 3: Run the full test suite**

Run:

```bash
cargo test
```

Expected: PASS.

- [ ] **Step 4: Inspect generated request behavior by tests**

Run:

```bash
cargo test llm::tests::chat_completion_request_serializes_reasoning_effort_when_set -- --nocapture
```

Expected: PASS; no network call is made.

- [ ] **Step 5: Commit**

```bash
git add docs/protocol/app-server-boundary-v2.md
git commit -m "docs: document per-turn reasoning effort"
```

---

## Self-Review

- Spec coverage: The plan covers per-turn protocol input, non-global runtime propagation, OpenAI-compatible serialization, rollout persistence, provider-default behavior when omitted, docs, and focused/full tests.
- Placeholder scan: No placeholders or undefined task references remain.
- Type consistency: The plan consistently uses `ReasoningEffort`, `LlmRequestOptions`, `reasoning_effort`, and `complete_with_options`.
