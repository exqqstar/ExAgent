# ExAgent Compaction Token Budget Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Codex-style token budget accounting and rollout-backed context compaction to ExAgent without changing the durable storage model away from `rollout.jsonl`.

**Architecture:** Keep `rollout.jsonl` as the append-only durable fact log. Extend ExAgent's existing `ContextManager` into the prompt-history owner that also tracks token usage and decides whether history is over budget. Add a small compaction engine that summarizes old prompt history, replaces live prompt history with a summary, and appends `RolloutItem::Compacted` plus runtime events so replay rebuilds the compacted view.

**Tech Stack:** Rust, Tokio, serde, reqwest, existing `AgentConfig`, `LlmClient`, `ContextManager`, `ThreadSession`, `RolloutStore`, `RuntimeEvent`, and app-server boundary events.

## Reference Files

Read these Codex files first:

- `external-references/Codex/codex-rs/protocol/src/openai_models.rs`
- `external-references/Codex/codex-rs/protocol/src/protocol.rs`
- `external-references/Codex/codex-rs/core/src/context_manager/history.rs`
- `external-references/Codex/codex-rs/core/src/state/session.rs`
- `external-references/Codex/codex-rs/core/src/session/mod.rs`
- `external-references/Codex/codex-rs/core/src/session/turn.rs`
- `external-references/Codex/codex-rs/core/src/compact.rs`

Read these ExAgent files before editing:

- `src/config.rs`
- `src/model/types.rs`
- `src/model/llm.rs`
- `src/runtime/agent.rs`
- `src/runtime/context.rs`
- `src/runtime/thread_session/turn.rs`
- `src/state/rollout.rs`
- `src/state/events.rs`
- `src/state/session.rs`
- `src/app_server/protocol.rs`
- `src/app_server/thread_manager.rs`

## Target Shape

```text
AgentConfig
  -> context window and auto compact limit

LlmClient
  -> returns LlmCompletion { turn, token_usage }

ContextManager
  -> owns model-visible history
  -> owns TokenUsageInfo
  -> estimates active context tokens
  -> replaces history after compaction

ThreadSession turn loop
  -> pre-turn token check
  -> compact if over budget
  -> sample model
  -> record assistant turn
  -> update token usage
  -> emit token count

RolloutStore
  -> appends RolloutItem::Compacted
  -> replay uses replacement_history as logical history cut
```

## Design Decisions

- Keep `AssistantTurn` pure. It should represent assistant content and tool calls only.
- Add `LlmCompletion` for model-call metadata. `token_usage` belongs there, not inside `AssistantTurn`.
- Keep `TokenUsageInfo.total_token_usage` as cumulative API-reported usage.
- Use `TokenUsageInfo.last_token_usage.total_tokens` as the last model-call context baseline.
- Estimate local items added after the last assistant message and add that to the active-context budget.
- First implementation only needs local compaction. Do not add Codex remote compaction.
- First implementation should support pre-turn auto compaction. Context-window error retry can be a follow-up task in this same plan.
- `rollout.jsonl` remains append-only. Compaction is logical: replay replaces in-memory conversation with `replacement_history`; old JSONL lines remain.

## Acceptance Criteria

- `AgentConfig` exposes optional `model_context_window` and `auto_compact_token_limit`, and derives a safe default auto-compact threshold at 90% of the configured context window.
- `LlmClient::complete` returns assistant content plus optional token metadata through `LlmCompletion`, while `AssistantTurn` remains limited to model-visible assistant content and tool calls.
- The OpenAI-compatible adapter parses response `usage` into `TokenUsage`, including cached input and reasoning output tokens when present.
- `ContextManager` owns prompt-visible history and `TokenUsageInfo`, can estimate active context usage, can merge API-reported token usage, and can mark usage as full after a context-window failure.
- `ThreadSession` checks token budget before recording a new user turn and runs pre-turn compaction when active context tokens meet or exceed the configured threshold.
- A successful compaction replaces live prompt history with `replacement_history`, appends `RolloutItem::Compacted`, records `RuntimeEventKind::CompactionWritten`, and leaves existing `rollout.jsonl` lines intact.
- Cold replay from `rollout.jsonl` rebuilds the compacted prompt-visible conversation from the latest `Compacted.replacement_history` instead of resurrecting logically compacted history.
- Runtime emits a `TokenCount` event after model token usage changes, and app-server event replay can return that event without polluting the compact `ThreadView` item list.
- Context-window errors are classified well enough for the OpenAI-compatible adapter, trigger a single compact-and-retry path, and never loop indefinitely.
- Existing durable storage invariants remain true: new runtime state uses `.exagent/threads/<thread_id>/rollout.jsonl`; legacy `snapshot_path/events_path` remain compatibility-only until protocol v3 work.
- Documentation describes the env vars, token count event, compaction event, and logical compaction behavior.
- Final verification passes with `cargo fmt --check` and `cargo test --all`.

## Task 1: Add Token Budget Config

**Files:**

- Modify: `src/config.rs`
- Test: `src/config.rs`

**Step 1: Write failing config tests**

Add tests for:

- default config has no context window unless env is set
- `auto_compact_token_limit()` returns explicit limit when no context window exists
- `auto_compact_token_limit()` clamps explicit limit to 90% of context window
- `auto_compact_token_limit()` derives 90% of context window when no explicit limit exists

Example expected API:

```rust
let config = AgentConfig {
    model_context_window: Some(100_000),
    auto_compact_token_limit: None,
    ..AgentConfig::default()
};

assert_eq!(config.resolved_auto_compact_token_limit(), Some(90_000));
```

**Step 2: Run failing tests**

Run:

```bash
cargo test config::tests::auto_compact --lib
```

Expected: FAIL because the fields and helper do not exist.

**Step 3: Implement minimal config support**

Add fields:

```rust
pub struct AgentConfig {
    pub model: String,
    pub max_turns: usize,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub policy_mode: PolicyMode,
    pub model_context_window: Option<i64>,
    pub auto_compact_token_limit: Option<i64>,
}
```

Add helper:

```rust
impl AgentConfig {
    pub fn resolved_auto_compact_token_limit(&self) -> Option<i64> {
        let context_limit = self
            .model_context_window
            .map(|context_window| (context_window * 9) / 10);

        match (self.auto_compact_token_limit, context_limit) {
            (Some(configured), Some(context_limit)) => Some(configured.min(context_limit)),
            (Some(configured), None) => Some(configured),
            (None, Some(context_limit)) => Some(context_limit),
            (None, None) => None,
        }
    }
}
```

Parse optional env vars in `Default`:

```text
EXAGENT_MODEL_CONTEXT_WINDOW
EXAGENT_AUTO_COMPACT_TOKEN_LIMIT
```

Invalid env values should be ignored rather than panic.

**Step 4: Verify**

Run:

```bash
cargo test config::tests::auto_compact --lib
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add token budget config"
```

## Task 2: Add Token Usage Model Types

**Files:**

- Modify: `src/model/types.rs`
- Test: `src/model/types.rs`

**Step 1: Write failing tests**

Add tests for:

- `TokenUsage::add_assign`
- `TokenUsageInfo::new_or_append`
- `TokenUsageInfo::full_context_window`
- `LlmCompletion::from_turn`

Expected API:

```rust
let usage = TokenUsage {
    input_tokens: 10,
    cached_input_tokens: 2,
    output_tokens: 5,
    reasoning_output_tokens: 0,
    total_tokens: 15,
};

let info = TokenUsageInfo::new_or_append(&None, Some(&usage), Some(100_000))
    .expect("token info");

assert_eq!(info.last_token_usage.total_tokens, 15);
assert_eq!(info.total_token_usage.total_tokens, 15);
assert_eq!(info.model_context_window, Some(100_000));
```

**Step 2: Run failing tests**

Run:

```bash
cargo test model::types::tests::token_usage --lib
```

Expected: FAIL because token usage types do not exist.

**Step 3: Implement types**

Add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageInfo {
    pub total_token_usage: TokenUsage,
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmCompletion {
    pub turn: AssistantTurn,
    pub token_usage: Option<TokenUsage>,
}
```

Add methods:

```rust
impl TokenUsage {
    pub fn add_assign(&mut self, other: &TokenUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

impl TokenUsageInfo {
    pub fn new_or_append(
        info: &Option<TokenUsageInfo>,
        last: Option<&TokenUsage>,
        model_context_window: Option<i64>,
    ) -> Option<Self> {
        if info.is_none() && last.is_none() && model_context_window.is_none() {
            return None;
        }

        let mut next = info.clone().unwrap_or(Self {
            total_token_usage: TokenUsage::default(),
            last_token_usage: TokenUsage::default(),
            model_context_window,
        });

        if let Some(last) = last {
            next.total_token_usage.add_assign(last);
            next.last_token_usage = last.clone();
        }
        if model_context_window.is_some() {
            next.model_context_window = model_context_window;
        }

        Some(next)
    }

    pub fn full_context_window(context_window: i64) -> Self {
        let usage = TokenUsage {
            total_tokens: context_window,
            ..TokenUsage::default()
        };
        Self {
            total_token_usage: usage.clone(),
            last_token_usage: usage,
            model_context_window: Some(context_window),
        }
    }
}
```

Add:

```rust
impl AssistantTurn {
    pub fn into_completion(self) -> LlmCompletion {
        LlmCompletion {
            turn: self,
            token_usage: None,
        }
    }
}
```

**Step 4: Verify**

Run:

```bash
cargo test model::types::tests::token_usage --lib
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/model/types.rs
git commit -m "feat: add model token usage types"
```

## Task 3: Return LlmCompletion From LlmClient

**Files:**

- Modify: `src/model/llm.rs`
- Modify: `src/runtime/agent.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: tests using `LlmClient`
- Test: `tests/llm_mock.rs`

**Step 1: Write failing adapter tests**

Add or update tests for:

- `MockLlm::complete` returns `LlmCompletion`
- `OpenAiCompatibleLlm::parse_response` parses `usage`
- missing `usage` is allowed and returns `token_usage: None`

Example OpenAI-compatible response body:

```json
{
  "choices": [
    {
      "message": {
        "content": "hello"
      }
    }
  ],
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 5,
    "total_tokens": 15,
    "prompt_tokens_details": {
      "cached_tokens": 2
    },
    "completion_tokens_details": {
      "reasoning_tokens": 1
    }
  }
}
```

Expected parsed usage:

```rust
TokenUsage {
    input_tokens: 10,
    cached_input_tokens: 2,
    output_tokens: 5,
    reasoning_output_tokens: 1,
    total_tokens: 15,
}
```

**Step 2: Run failing tests**

Run:

```bash
cargo test llm --lib
cargo test --test llm_mock
```

Expected: FAIL until the trait and callers are updated.

**Step 3: Update trait and adapters**

Change:

```rust
async fn complete(...) -> Result<AssistantTurn>;
```

to:

```rust
async fn complete(...) -> Result<LlmCompletion>;
```

Update `MockLlm` to store `VecDeque<LlmCompletion>` or accept `Vec<AssistantTurn>` and convert internally. Prefer accepting `Vec<AssistantTurn>` for existing test ergonomics, plus a constructor for completions:

```rust
pub fn new(turns: Vec<AssistantTurn>) -> Self
pub fn new_completions(completions: Vec<LlmCompletion>) -> Self
```

Update `OpenAiCompatibleLlm::parse_response` to return `LlmCompletion`.

Update `src/runtime/agent.rs`:

```rust
pub(crate) async fn sample_assistant_turn(...) -> Result<LlmCompletion> {
    self.llm.complete(prompt, tool_schemas).await
}
```

Update all test implementations of `LlmClient`.

**Step 4: Verify**

Run:

```bash
cargo test llm --lib
cargo test --test llm_mock
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/model/llm.rs src/runtime/agent.rs src/app_server/thread_manager.rs tests
git commit -m "feat: return token metadata from llm completions"
```

## Task 4: Add Token Accounting to ContextManager

**Files:**

- Modify: `src/runtime/context.rs`
- Test: `src/runtime/context.rs`

**Step 1: Write failing tests**

Add tests for:

- `estimate_token_count` counts serialized conversation content
- `update_token_info_from_usage` stores cumulative and last usage
- `active_context_tokens` uses last usage plus local items after last assistant
- fallback to full local estimate when no API usage exists
- `set_token_usage_full` marks context as full
- `replace_history` clears or preserves token baseline as intended

Example:

```rust
let mut manager = ContextManager::new();
manager.record_items([
    ConversationMessage::user("hello"),
    ConversationMessage::assistant(Some("hi".to_string()), vec![]),
]);
manager.update_token_info_from_usage(
    &TokenUsage {
        total_tokens: 100,
        ..TokenUsage::default()
    },
    Some(1_000),
);
manager.record_items([ConversationMessage::tool("call_1", "large output")]);

assert!(manager.active_context_tokens() > 100);
```

**Step 2: Run failing tests**

Run:

```bash
cargo test runtime::context::tests::token --lib
```

Expected: FAIL because token accounting methods do not exist.

**Step 3: Implement token fields and helpers**

Add to `ContextManager`:

```rust
token_info: Option<TokenUsageInfo>,
```

Add helpers:

```rust
pub(crate) fn token_info(&self) -> Option<TokenUsageInfo>;
pub(crate) fn set_token_info(&mut self, info: Option<TokenUsageInfo>);
pub(crate) fn update_token_info_from_usage(
    &mut self,
    usage: &TokenUsage,
    model_context_window: Option<i64>,
);
pub(crate) fn set_token_usage_full(&mut self, context_window: i64);
pub(crate) fn estimate_token_count(&self) -> i64;
pub(crate) fn active_context_tokens(&self) -> i64;
```

Use a simple estimator:

```rust
fn approx_tokens(text: &str) -> i64 {
    let bytes = i64::try_from(text.len()).unwrap_or(i64::MAX);
    (bytes + 3) / 4
}
```

For `ConversationMessage`, estimate `serde_json::to_string(message).len() / 4`.

Implement `items_after_last_assistant_message`:

```rust
let start = self
    .items
    .iter()
    .rposition(|item| item.role == MessageRole::Assistant)
    .map_or(self.items.len(), |index| index.saturating_add(1));
```

Then:

```rust
pub(crate) fn active_context_tokens(&self) -> i64 {
    let Some(info) = &self.token_info else {
        return self.estimate_token_count();
    };

    let local_added = self.items_after_last_assistant_message()
        .iter()
        .map(estimate_message_tokens)
        .fold(0i64, i64::saturating_add);

    info.last_token_usage.total_tokens.saturating_add(local_added)
}
```

**Step 4: Verify**

Run:

```bash
cargo test runtime::context::tests::token --lib
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/context.rs
git commit -m "feat: track token usage in context manager"
```

## Task 5: Add Token Count Runtime Event

**Files:**

- Modify: `src/state/events.rs`
- Modify: `src/state/rollout.rs`
- Modify: `src/app_server/protocol.rs`
- Modify: `src/app_server/thread_manager.rs`
- Test: `src/state/rollout.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write failing event tests**

Add tests for:

- `RuntimeEventKind::TokenCount` serializes as `token_count`
- rollout persistence policy keeps `TokenCount`
- events replay can filter `TokenCount`
- `ThreadView` does not add noisy visible `ThreadItem` for token count unless explicitly desired

Expected event shape:

```rust
RuntimeEventKind::TokenCount {
    info: Some(TokenUsageInfo { ... }),
}
```

**Step 2: Run failing tests**

Run:

```bash
cargo test token_count --lib
cargo test --test app_server_boundary token_count
```

Expected: FAIL because the event kind and filter do not exist.

**Step 3: Implement event support**

Add to `RuntimeEventKind`:

```rust
TokenCount {
    info: Option<TokenUsageInfo>,
}
```

Add to `RuntimeEventKindFilter`:

```rust
TokenCount,
```

Update:

- `should_persist_event` in `src/state/rollout.rs`
- `latest_turn_state` in `src/app_server/thread_manager.rs`
- `build_turn_views` in `src/app_server/thread_manager.rs`
- `thread_item_from_event` should return `None` for `TokenCount`
- `runtime_event_kind_matches`

**Step 4: Verify**

Run:

```bash
cargo test token_count --lib
cargo test --test app_server_boundary token_count
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/state/events.rs src/state/rollout.rs src/app_server/protocol.rs src/app_server/thread_manager.rs tests/app_server_boundary.rs
git commit -m "feat: add token count runtime event"
```

## Task 6: Add Local Compaction Engine

**Files:**

- Create: `src/runtime/compaction.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/agent.rs`
- Test: `src/runtime/compaction.rs`

**Step 1: Write failing compaction tests**

Add tests for:

- compaction prompt includes prior conversation
- compaction output creates a summary message
- replacement history contains only the summary for pre-turn compaction
- compaction returns an error if the summarizer produces empty output

Expected API:

```rust
pub(crate) struct CompactionResult {
    pub(crate) summary: String,
    pub(crate) replacement_history: Vec<ConversationMessage>,
}

pub(crate) async fn compact_history(
    agent: &Agent,
    history: &[ConversationMessage],
) -> Result<CompactionResult>;
```

**Step 2: Run failing tests**

Run:

```bash
cargo test runtime::compaction --lib
```

Expected: FAIL because the module does not exist.

**Step 3: Implement local compaction**

Create a small prompt builder:

```rust
const DEFAULT_COMPACT_PROMPT: &str = "\
Summarize the conversation so far for a coding agent runtime. \
Preserve user goals, architectural decisions, files changed, commands run, \
open questions, and constraints. Omit irrelevant chatter.";
```

Build prompt history:

```text
system: DEFAULT_COMPACT_PROMPT
user: serialized prior conversation
```

Call:

```rust
let completion = agent.sample_assistant_turn(&prompt, &[]).await?;
```

Build replacement history:

```rust
vec![ConversationMessage::injected_system(format!(
    "Conversation summary so far:\n{}",
    summary
))]
```

Do not write rollout here. This module should only compute the result.

**Step 4: Verify**

Run:

```bash
cargo test runtime::compaction --lib
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/compaction.rs src/runtime/mod.rs src/runtime/agent.rs
git commit -m "feat: add local context compaction engine"
```

## Task 7: Wire Pre-Turn Auto Compaction

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/context.rs`
- Test: `src/runtime/thread_session/turn.rs`
- Test: `tests/thread_runtime.rs`

**Step 1: Write failing turn-loop tests**

Add tests for:

- when `active_context_tokens < limit`, no compaction happens
- when `active_context_tokens >= limit`, pre-turn compaction runs before new user message is recorded
- compaction appends `RolloutItem::Compacted`
- compaction records `RuntimeEventKind::CompactionWritten`
- replay after compaction restores replacement history and not old conversation

Use `MockLlm::new_completions` so the first completion returns the summary and the second completion returns the normal assistant response.

**Step 2: Run failing tests**

Run:

```bash
cargo test thread_session_pre_turn_compaction --lib
cargo test --test thread_runtime compact
```

Expected: FAIL because turn loop does not compact.

**Step 3: Implement pre-turn check**

In `handle_user_input_inner`, before applying current turn context and before recording the new user message:

```rust
if let Some(limit) = self.agent.config().resolved_auto_compact_token_limit() {
    if self.context_manager.active_context_tokens() >= limit {
        self.compact_before_turn(&turn_id).await?;
    }
}
```

Add a helper on `ThreadSession` or in `turn.rs`:

```rust
async fn compact_before_turn(&mut self, turn_id: &TurnId) -> Result<()>
```

It should:

1. Clone current `context_manager.for_prompt()`.
2. Call `runtime::compaction::compact_history`.
3. `context_manager.replace_history(replacement_history.clone(), None)`.
4. Sync snapshot.
5. Append `RolloutItem::Compacted(CompactedItem { message, replacement_history: Some(...) })`.
6. Record `RuntimeEventKind::CompactionWritten { summary }`.
7. Emit `RuntimeEventKind::TokenCount` using recomputed local estimate.

Use `None` as reference turn context for pre-turn compaction so the next regular turn reinjects full runtime context.

**Step 4: Verify**

Run:

```bash
cargo test thread_session_pre_turn_compaction --lib
cargo test --test thread_runtime compact
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/thread_session/turn.rs src/runtime/context.rs tests/thread_runtime.rs
git commit -m "feat: auto compact context before turns"
```

## Task 8: Record Token Usage After Model Calls

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/context.rs`
- Test: `src/runtime/thread_session/turn.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write failing tests**

Add tests for:

- `token_usage` returned by the LLM updates `ContextManager.token_info`
- a `TokenCount` event is emitted after assistant response
- token info survives cold replay if `TokenCount` is persisted
- if `token_usage` is `None`, runtime still emits no bogus usage and active budget falls back to local estimate

**Step 2: Run failing tests**

Run:

```bash
cargo test thread_session_records_token_usage --lib
cargo test --test app_server_boundary token_count
```

Expected: FAIL until turn loop records usage.

**Step 3: Update turn loop**

In `run_session_turn`, replace:

```rust
let turn = agent.sample_assistant_turn(&prompt, &tool_runtime.schemas()).await?;
```

with:

```rust
let completion = agent.sample_assistant_turn(&prompt, &tool_runtime.schemas()).await?;
let turn = completion.turn;
```

After `record_assistant_turn`, call:

```rust
if let Some(usage) = completion.token_usage.as_ref() {
    context_manager.update_token_info_from_usage(
        usage,
        agent.config().model_context_window,
    );
}
record_token_count_event(...)?;
```

Keep assistant content recording separate from token metadata.

**Step 4: Verify**

Run:

```bash
cargo test thread_session_records_token_usage --lib
cargo test --test app_server_boundary token_count
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/runtime/thread_session/turn.rs src/runtime/context.rs tests/app_server_boundary.rs
git commit -m "feat: record token usage from model calls"
```

## Task 9: Add Context-Window Error Detection and Retry Once

**Files:**

- Modify: `src/model/llm.rs`
- Modify: `src/runtime/thread_session/turn.rs`
- Test: `src/model/llm.rs`
- Test: `src/runtime/thread_session/turn.rs`

**Step 1: Write failing tests**

Add tests for:

- OpenAI-compatible HTTP errors that look like context-window failures are classified
- first context-window failure triggers `set_token_usage_full`
- runtime compacts and retries once
- second failure returns the error instead of looping forever

**Step 2: Run failing tests**

Run:

```bash
cargo test context_window --lib
```

Expected: FAIL because errors are plain `anyhow`.

**Step 3: Add minimal error classification**

Do not build a large error hierarchy. Add a helper:

```rust
pub fn is_context_window_error(err: &anyhow::Error) -> bool
```

In `OpenAiCompatibleLlm`, when status is not success, include the raw body in the error. The helper can match known substrings for the first version:

```text
context_length_exceeded
maximum context length
context window
too many tokens
```

In turn loop, on this error:

1. If model context window exists, call `context_manager.set_token_usage_full`.
2. Compact preserving the current user message.
3. Retry once.

Preserving the current user message can be implemented by adding a compaction mode:

```rust
enum CompactionMode {
    PreTurn,
    RetryPreserveLastUser,
}
```

For retry mode, replacement history should be:

```text
summary injected system message
last user message
```

**Step 4: Verify**

Run:

```bash
cargo test context_window --lib
cargo test --lib
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/model/llm.rs src/runtime/thread_session/turn.rs src/runtime/compaction.rs
git commit -m "feat: compact and retry on context window errors"
```

## Task 10: App-Server and Documentation Polish

**Files:**

- Modify: `docs/protocol/app-server-boundary-v2.md`
- Modify: `docs/demo/exagent-walkthrough.md`
- Modify: `README.md`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write or update boundary tests**

Verify:

- `events_replay` can return `TokenCount`
- `include_snapshot` includes latest compaction after compaction
- `ThreadView` remains compact and does not add noisy token items

**Step 2: Update docs**

Document:

```text
EXAGENT_MODEL_CONTEXT_WINDOW
EXAGENT_AUTO_COMPACT_TOKEN_LIMIT
TokenCount event
CompactionWritten event
rollout append-only logical compaction
```

Add a note:

```text
Compaction does not rewrite rollout.jsonl. It appends a Compacted checkpoint.
Replay uses the latest replacement_history to rebuild model-visible history.
```

**Step 3: Verify docs and tests**

Run:

```bash
cargo test --test app_server_boundary
cargo test --all
```

Expected: PASS.

**Step 4: Commit**

```bash
git add README.md docs/protocol/app-server-boundary-v2.md docs/demo/exagent-walkthrough.md tests/app_server_boundary.rs
git commit -m "docs: document compaction token budget"
```

## Final Verification

Run:

```bash
cargo fmt --check
cargo test --all
```

Expected:

```text
PASS
```

Then inspect a real rollout file from a test or demo run:

```bash
rg -n '"type":"compacted"|"type":"token_count"' .exagent/threads -g 'rollout.jsonl'
```

Expected:

```text
rollout.jsonl contains compacted checkpoints and token_count events when compaction/token accounting occurred.
```

## Non-Goals For This Pass

- Do not implement Codex remote compaction.
- Do not implement model downshift compaction.
- Do not implement exact tokenizer counting.
- Do not implement image token estimation.
- Do not physically rewrite `rollout.jsonl`.
- Do not move `ContextManager` into many files until it becomes hard to maintain.
- Do not mix `token_usage` into `AssistantTurn`.

## Suggested Implementation Order

```text
1. Token budget config
2. Token usage model types
3. LlmCompletion return type
4. ContextManager token accounting
5. TokenCount event
6. Local compaction engine
7. Pre-turn auto compaction
8. Token usage recording after model calls
9. Context-window retry once
10. Docs and app-server polish
```

This order keeps each patch small and keeps the durable rollout design stable throughout the work.
