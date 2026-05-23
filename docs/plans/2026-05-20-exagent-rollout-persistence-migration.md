# ExAgent Rollout Persistence Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace ExAgent's `snapshot.json + events.jsonl` durable state model with a Codex-style `rollout.jsonl` source of truth, and make `ThreadSession` own a stateful `ContextManager`.

**Status:** Implemented in the current working tree. Runtime load and cold read use `rollout.jsonl`; `snapshot.json + events.jsonl` are no longer runtime or migration inputs.

**Architecture:** Add a rollout schema/store first, then move runtime history ownership from `SessionSnapshot` into `ContextManager`, then switch new session persistence and resume to rollout. Keep only v2 compatibility path fields for snapshot/events; do not read or migrate those files.

**Tech Stack:** Rust, Tokio, serde JSONL, existing `ThreadSession`, `ThreadRuntime`, `RuntimeEvent`, `ConversationMessage`, `TurnContextItem`, `ToolCallRuntime`, app-server boundary tests.

## Reference Design

Read first:

- `docs/architecture/2026-05-20-exagent-rollout-persistence-architecture.md`
- `docs/architecture/2026-05-18-exagent-context-projection-layer.md`
- `src/runtime/context.rs`
- `src/runtime/thread_session/mod.rs`
- `src/runtime/thread_session/turn.rs`
- `src/runtime/thread_session/events.rs`
- `src/state/session.rs`
- `src/state/events.rs`
- `src/state/transcript.rs`
- `external-references/Codex/codex-rs/protocol/src/protocol.rs`
- `external-references/Codex/codex-rs/core/src/context_manager/history.rs`
- `external-references/Codex/codex-rs/core/src/state/session.rs`
- `external-references/Codex/codex-rs/core/src/session/rollout_reconstruction.rs`
- `external-references/Codex/codex-rs/rollout/src/recorder.rs`
- `external-references/Codex/codex-rs/rollout/src/policy.rs`

Target invariant:

```text
rollout.jsonl is the only durable source of truth for new sessions.
ThreadSession is the runtime owner.
ContextManager owns prompt-visible history and reference_turn_context.
```

## Implementation Strategy

Do not attempt this as one patch. Use four stages:

```text
P0: Add rollout schema/store beside existing transcript files.
P1: Upgrade ContextManager into a stateful history owner.
P2: Route ThreadSession writes through rollout + ContextManager.
P3: Switch resume/new-session persistence to rollout and retire snapshot/events for new sessions.
```

Each stage must keep the test suite green before moving on.

## Task 1: Add Rollout Schema

**Files:**

- Create: `src/state/rollout.rs`
- Modify: `src/state/mod.rs`
- Test: `src/state/rollout.rs`

**Step 1: Write failing serialization tests**

Add tests inside `src/state/rollout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::session::{AgentRole, TurnContextItem};
    use crate::types::{ConversationMessage, EventId, SessionId, TurnId};
    use std::path::PathBuf;

    #[test]
    fn rollout_item_serializes_with_snake_case_type_tag() {
        let item = RolloutItem::ResponseItem(ConversationMessage::user("hello"));
        let value = serde_json::to_value(item).expect("serialize rollout item");

        assert_eq!(value["type"], "response_item");
        assert_eq!(value["payload"]["content"], "hello");
    }

    #[test]
    fn rollout_item_can_store_session_meta_turn_context_and_compaction() {
        let meta = RolloutItem::SessionMeta(SessionMeta {
            thread_id: SessionId::new("thread_1"),
            root_thread_id: SessionId::new("thread_1"),
            parent_thread_id: None,
            spawned_by_turn_id: None,
            agent_role: AgentRole::Primary,
            workspace_root: PathBuf::from("/workspace"),
            initial_cwd: PathBuf::from("/workspace"),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        });
        let context = RolloutItem::TurnContext(TurnContextItem {
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: "mock".to_string(),
            policy_mode: crate::policy::PolicyMode::Off,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            current_utc_date: Some("2026-05-20".to_string()),
        });
        let compacted = RolloutItem::Compacted(CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(vec![ConversationMessage::assistant(Some("summary"), vec![])]),
        });

        serde_json::to_string(&meta).expect("serialize meta");
        serde_json::to_string(&context).expect("serialize context");
        serde_json::to_string(&compacted).expect("serialize compacted");
    }
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test state::rollout --lib
```

Expected:

```text
FAIL because src/state/rollout.rs does not exist or RolloutItem is undefined.
```

**Step 3: Implement minimal schema**

Create:

```rust
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::events::RuntimeEvent;
use crate::session::{AgentRole, TurnContextItem};
use crate::types::{ConversationMessage, SessionId, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMeta),
    ResponseItem(ConversationMessage),
    TurnContext(TurnContextItem),
    Compacted(CompactedItem),
    EventMsg(RuntimeEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMeta {
    pub thread_id: SessionId,
    pub root_thread_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by_turn_id: Option<TurnId>,
    pub agent_role: AgentRole,
    pub workspace_root: PathBuf,
    pub initial_cwd: PathBuf,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactedItem {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_history: Option<Vec<ConversationMessage>>,
}
```

Modify `src/state/mod.rs`:

```rust
pub mod rollout;
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test state::rollout --lib
```

Expected:

```text
PASS
```

## Task 2: Add RolloutStore JSONL IO

**Files:**

- Modify: `src/state/rollout.rs`
- Test: `src/state/rollout.rs`

**Step 1: Write failing IO tests**

Add:

```rust
#[tokio::test]
async fn rollout_store_appends_and_reads_jsonl_items() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    let store = RolloutStore::new(path.clone());

    store
        .append_items(&[
            RolloutItem::ResponseItem(ConversationMessage::user("first")),
            RolloutItem::ResponseItem(ConversationMessage::assistant(Some("second"), vec![])),
        ])
        .await
        .expect("append rollout items");

    let items = RolloutStore::read_items(&path).await.expect("read rollout");
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0],
        RolloutItem::ResponseItem(ConversationMessage::user("first"))
    );
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test state::rollout::tests::rollout_store_appends_and_reads_jsonl_items --lib
```

Expected:

```text
FAIL because RolloutStore does not exist.
```

**Step 3: Implement minimal store**

Add:

```rust
#[derive(Debug, Clone)]
pub struct RolloutStore {
    path: PathBuf,
}

impl RolloutStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        self.path.as_path()
    }

    pub async fn append_items(&self, items: &[RolloutItem]) -> std::io::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut text = String::new();
        for item in items {
            let line = serde_json::to_string(item).map_err(std::io::Error::other)?;
            text.push_str(&line);
            text.push('\n');
        }
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await
    }

    pub async fn read_items(path: &std::path::Path) -> std::io::Result<Vec<RolloutItem>> {
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut items = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let item = serde_json::from_str(line).map_err(std::io::Error::other)?;
            items.push(item);
        }
        Ok(items)
    }
}
```

**Step 4: Run test to verify pass**

Run:

```bash
cargo test state::rollout::tests::rollout_store_appends_and_reads_jsonl_items --lib
```

Expected:

```text
PASS
```

## Task 3: Add Rollout Event Persistence Policy

**Files:**

- Modify: `src/state/rollout.rs`
- Test: `src/state/rollout.rs`

**Step 1: Write failing policy tests**

Add:

```rust
#[test]
fn event_persistence_policy_filters_runtime_events() {
    let turn_started = RuntimeEvent {
        id: EventId::new("event_1"),
        session_id: SessionId::new("thread_1"),
        turn_id: Some(TurnId::new("turn_1")),
        kind: RuntimeEventKind::TurnStarted,
        timestamp_ms: 1,
        payload: None,
    };
    let exec_output = RuntimeEvent {
        kind: RuntimeEventKind::ExecOutput,
        ..turn_started.clone()
    };

    assert!(should_persist_rollout_item(&RolloutItem::EventMsg(turn_started)));
    assert!(!should_persist_rollout_item(&RolloutItem::EventMsg(exec_output)));
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test state::rollout::tests::event_persistence_policy_filters_runtime_events --lib
```

Expected:

```text
FAIL because should_persist_rollout_item does not exist.
```

**Step 3: Implement policy**

Add:

```rust
pub fn should_persist_rollout_item(item: &RolloutItem) -> bool {
    match item {
        RolloutItem::SessionMeta(_)
        | RolloutItem::ResponseItem(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::Compacted(_) => true,
        RolloutItem::EventMsg(event) => should_persist_event(event),
    }
}

fn should_persist_event(event: &RuntimeEvent) -> bool {
    matches!(
        event.kind,
        RuntimeEventKind::TurnStarted
            | RuntimeEventKind::TurnCompleted
            | RuntimeEventKind::TurnInterrupted
            | RuntimeEventKind::RuntimeError
            | RuntimeEventKind::ApprovalRequested
            | RuntimeEventKind::ApprovalResolved
    )
}
```

Update `append_items` to filter items before writing:

```rust
for item in items.iter().filter(|item| should_persist_rollout_item(item)) {
    ...
}
```

**Step 4: Run focused tests**

Run:

```bash
cargo test state::rollout --lib
```

Expected:

```text
PASS
```

## Task 4: Make ContextManager Stateful

**Files:**

- Modify: `src/runtime/context.rs`
- Test: `src/runtime/context.rs`

**Step 1: Write failing stateful ContextManager tests**

Add:

```rust
#[test]
fn context_manager_owns_items_and_reference_context() {
    let workspace_root = PathBuf::from("/workspace");
    let cwd = workspace_root.join("app");
    let config = test_config(&workspace_root, &cwd);
    let mut manager = ContextManager::new();

    let context = PromptContext::for_turn(
        &config,
        TurnPaths {
            workspace_root: workspace_root.clone(),
            cwd: cwd.clone(),
        },
    );

    let injected = manager.apply_context_updates(context);
    manager.record_items([ConversationMessage::user("hello")]);

    assert_eq!(injected.len(), 2);
    assert!(manager.reference_turn_context().is_some());
    assert_eq!(manager.raw_items().len(), 3);
    assert_eq!(manager.for_prompt()[2].content, "hello");
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test runtime::context::tests::context_manager_owns_items_and_reference_context --lib
```

Expected:

```text
FAIL because ContextManager::new and instance methods do not exist.
```

**Step 3: Implement stateful ContextManager**

Refactor `ContextManager` from:

```rust
pub(crate) struct ContextManager;
```

to:

```rust
#[derive(Debug, Clone, Default)]
pub(crate) struct ContextManager {
    items: Vec<ConversationMessage>,
    history_version: u64,
    reference_turn_context: Option<TurnContextItem>,
}
```

Add methods:

```rust
impl ContextManager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn raw_items(&self) -> &[ConversationMessage] {
        &self.items
    }

    pub(crate) fn record_items<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = ConversationMessage>,
    {
        self.items.extend(items);
    }

    pub(crate) fn replace_history(
        &mut self,
        items: Vec<ConversationMessage>,
        reference_turn_context: Option<TurnContextItem>,
    ) {
        self.items = items;
        self.reference_turn_context = reference_turn_context;
        self.history_version = self.history_version.saturating_add(1);
    }

    pub(crate) fn apply_context_updates(
        &mut self,
        context: PromptContext,
    ) -> Vec<ConversationMessage> {
        let messages = match self.reference_turn_context.as_ref() {
            Some(previous) => build_context_update_messages(previous, &context.turn_context),
            None => build_initial_context_messages(&context.turn_context),
        };
        self.items.extend(messages.clone());
        self.reference_turn_context = Some(context.turn_context);
        messages
    }

    pub(crate) fn set_reference_turn_context(&mut self, context: Option<TurnContextItem>) {
        self.reference_turn_context = context;
    }

    pub(crate) fn reference_turn_context(&self) -> Option<TurnContextItem> {
        self.reference_turn_context.clone()
    }

    pub(crate) fn for_prompt(&self) -> Vec<ConversationMessage> {
        self.items.clone()
    }
}
```

Keep the old snapshot-based helpers temporarily as compatibility wrappers only if needed by tests. Prefer updating call sites in later tasks.

**Step 4: Run context tests**

Run:

```bash
cargo test runtime::context::tests --lib
```

Expected:

```text
PASS
```

## Task 5: Add Injected Message Metadata

**Files:**

- Modify: `src/model/types.rs`
- Modify: `src/runtime/context.rs`
- Test: `src/runtime/context.rs`

**Step 1: Write failing injected metadata test**

Add:

```rust
#[test]
fn context_messages_are_marked_injected() {
    let workspace_root = PathBuf::from("/workspace");
    let cwd = workspace_root.join("app");
    let config = test_config(&workspace_root, &cwd);
    let mut manager = ContextManager::new();

    let messages = manager.apply_context_updates(PromptContext::for_turn(
        &config,
        TurnPaths {
            workspace_root,
            cwd,
        },
    ));

    assert!(messages.iter().all(|message| message.injected));
    assert!(!ConversationMessage::user("hello").injected);
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test runtime::context::tests::context_messages_are_marked_injected --lib
```

Expected:

```text
FAIL because ConversationMessage.injected does not exist.
```

**Step 3: Add field and constructors**

Modify `ConversationMessage`:

```rust
#[serde(default, skip_serializing_if = "std::ops::Not::not")]
pub injected: bool,
```

Update constructors:

```rust
pub fn system(content: impl Into<String>) -> Self {
    Self {
        role: MessageRole::System,
        content: content.into(),
        tool_call_id: None,
        tool_calls: vec![],
        injected: false,
    }
}

pub fn injected_system(content: impl Into<String>) -> Self {
    Self {
        injected: true,
        ..Self::system(content)
    }
}
```

Update context message builders to use `ConversationMessage::injected_system(...)`.

**Step 4: Run tests**

Run:

```bash
cargo test runtime::context::tests model --lib
```

Expected:

```text
PASS
```

## Task 6: Introduce ThreadSession Rollout Load Helpers

**Files:**

- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/state/rollout.rs`
- Test: `src/runtime/thread_session/mod.rs`

**Step 1: Write failing reconstruction test**

Add a test that builds rollout items and hydrates a `ContextManager`:

```rust
#[test]
fn rollout_items_hydrate_context_manager_history_and_reference_context() {
    let workspace_root = PathBuf::from("/workspace");
    let context = TurnContextItem {
        workspace_root: workspace_root.clone(),
        cwd: workspace_root.clone(),
        model: "mock".to_string(),
        policy_mode: crate::policy::PolicyMode::Off,
        command_timeout_secs: 30,
        max_output_bytes: 1024,
        current_utc_date: Some("2026-05-20".to_string()),
    };
    let items = vec![
        RolloutItem::TurnContext(context.clone()),
        RolloutItem::ResponseItem(ConversationMessage::user("hello")),
        RolloutItem::ResponseItem(ConversationMessage::assistant(Some("hi"), vec![])),
    ];

    let manager = ContextManager::from_rollout_items(&items);

    assert_eq!(manager.raw_items().len(), 2);
    assert_eq!(manager.reference_turn_context(), Some(context));
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test rollout_items_hydrate_context_manager_history_and_reference_context --lib
```

Expected:

```text
FAIL because from_rollout_items does not exist.
```

**Step 3: Implement minimal forward replay**

In `runtime/context.rs`:

```rust
pub(crate) fn from_rollout_items(items: &[RolloutItem]) -> Self {
    let mut manager = ContextManager::new();
    for item in items {
        match item {
            RolloutItem::ResponseItem(message) => {
                manager.record_items([message.clone()]);
            }
            RolloutItem::TurnContext(context) => {
                manager.set_reference_turn_context(Some(context.clone()));
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    manager.replace_history(replacement_history.clone(), None);
                }
            }
            RolloutItem::SessionMeta(_) | RolloutItem::EventMsg(_) => {}
        }
    }
    manager
}
```

**Step 4: Run focused tests**

Run:

```bash
cargo test rollout_items_hydrate_context_manager_history_and_reference_context --lib
```

Expected:

```text
PASS
```

## Task 7: Add Rollout Path Layout And Legacy Transcript Boundary

**Files:**

- Modify: `src/state/rollout.rs`
- Modify: `src/state/transcript.rs`
- Test: `src/state/rollout.rs`

**Step 1: Write failing path test**

Add:

```rust
#[test]
fn rollout_path_uses_thread_directory_and_rollout_jsonl() {
    let workspace_root = PathBuf::from("/workspace");
    let thread_id = SessionId::new("thread_1");

    let paths = rollout_paths(&workspace_root, &thread_id);

    assert!(paths.thread_dir.ends_with("thread_1"));
    assert!(paths.rollout_path.ends_with("rollout.jsonl"));
}
```

**Step 2: Run test**

Run:

```bash
cargo test rollout_path_uses_thread_directory_and_rollout_jsonl --lib
```

Expected:

```text
FAIL because rollout_paths does not exist.
```

**Step 3: Implement path helper**

Add:

```rust
#[derive(Debug, Clone)]
pub struct RolloutPaths {
    pub thread_dir: PathBuf,
    pub rollout_path: PathBuf,
}

pub fn rollout_paths(workspace_root: &std::path::Path, thread_id: &SessionId) -> RolloutPaths {
    let thread_dir = workspace_root
        .join(".exagent")
        .join("threads")
        .join(thread_id.as_str());
    RolloutPaths {
        rollout_path: thread_dir.join("rollout.jsonl"),
        thread_dir,
    }
}
```

Do not delete `transcript.rs` yet. Keep only JSON helpers and compatibility path construction there.

**Step 4: Run tests**

Run:

```bash
cargo test state::rollout --lib
```

Expected:

```text
PASS
```

## Task 8: Start New Threads With SessionMeta Rollout

**Files:**

- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/app_server/thread_manager.rs`
- Test: `tests/thread_runtime.rs` or `tests/app_server_boundary.rs`

**Step 1: Write failing integration test**

Add a test that starts a thread and asserts `rollout.jsonl` exists with `SessionMeta`:

```rust
#[tokio::test]
async fn thread_start_writes_rollout_session_meta() {
    let dir = tempdir().unwrap();
    let thread_id = SessionId::new("thread_rollout_start");
    let config = AgentConfig {
        workspace_root: dir.path().to_path_buf(),
        cwd: dir.path().to_path_buf(),
        ..AgentConfig::default()
    };

    let _session = ThreadSession::new(ThreadSessionOptions::new(
        thread_id.clone(),
        config.clone(),
        agent_factory(vec![]),
    ))
    .expect("create session");

    let paths = crate::state::rollout::rollout_paths(&config.workspace_root, &thread_id);
    let items = RolloutStore::read_items(&paths.rollout_path).await.expect("read rollout");

    assert!(matches!(items.first(), Some(RolloutItem::SessionMeta(_))));
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test thread_start_writes_rollout_session_meta --lib
```

Expected:

```text
FAIL because ThreadSession does not write rollout.
```

**Step 3: Implement minimal write**

Add `RolloutStore` to `ThreadSession` construction. On new thread creation:

```text
construct SessionMeta from thread id and config
append RolloutItem::SessionMeta
```

Keep old snapshot writing in place for this task. Do not switch resume yet.

**Step 4: Run focused tests**

Run:

```bash
cargo test thread_start_writes_rollout_session_meta --lib
```

Expected:

```text
PASS
```

## Task 9: Route Turn Conversation Writes Through ContextManager And Rollout

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Test: `src/runtime/thread_session/turn.rs`

**Step 1: Write failing turn rollout test**

Add a test proving a turn writes context, user, assistant into rollout and ContextManager:

```rust
#[tokio::test]
async fn thread_turn_records_rollout_items_and_context_history() {
    // Arrange a ThreadSession with RecordingLlm returning one assistant message.
    // Act: handle_user_input(...)
    // Assert:
    // - rollout contains TurnContext
    // - rollout contains ResponseItem user
    // - rollout contains ResponseItem assistant
    // - session context raw_items contains injected context + user + assistant
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test thread_turn_records_rollout_items_and_context_history --lib
```

Expected:

```text
FAIL because turn path still writes snapshot as authority.
```

**Step 3: Implement turn writes**

In `handle_user_input_inner`:

```text
build TurnContextItem
context.apply_context_updates(...)
append RolloutItem::TurnContext
record user ConversationMessage in ContextManager
append RolloutItem::ResponseItem(user)
record selected RuntimeEvent as RolloutItem::EventMsg
```

In assistant/tool record helpers:

```text
record in ContextManager
append RolloutItem::ResponseItem
apply tool effects to runtime fields
persist selected RuntimeEvent separately
```

Keep old snapshot checkpoint for this task if needed, but tests must assert rollout correctness.

**Step 4: Run focused tests**

Run:

```bash
cargo test runtime::thread_session::turn --lib
```

Expected:

```text
PASS
```

## Task 10: Switch Sampling To Stateful ContextManager

**Files:**

- Modify: `src/runtime/thread_session/turn.rs`
- Modify: `src/runtime/context.rs`
- Test: `src/runtime/thread_session/turn.rs`

**Step 1: Write failing prompt-source guard**

Add or update architecture guard:

```rust
#[test]
fn turn_loop_does_not_sample_from_session_snapshot_conversation() {
    let source = std::fs::read_to_string("src/runtime/thread_session/turn.rs").unwrap();
    assert!(!source.contains("snapshot.conversation.clone()"));
    assert!(!source.contains("ContextManager::for_prompt(snapshot)"));
}
```

**Step 2: Run guard**

Run:

```bash
cargo test --test architecture_guards turn_loop_does_not_sample_from_session_snapshot_conversation
```

Expected:

```text
FAIL until turn loop uses session-owned ContextManager.
```

**Step 3: Update sampling**

Change sampling to:

```rust
let prompt = self.context.for_prompt();
```

or, if borrow boundaries require helper methods:

```rust
let prompt = self.prompt_for_sampling();
```

Do not derive prompt from `SessionSnapshot`.

**Step 4: Run tests**

Run:

```bash
cargo test runtime::thread_session::turn --lib
cargo test --test architecture_guards
```

Expected:

```text
PASS
```

## Task 11: Implement Rollout Resume Path

**Files:**

- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/state/rollout.rs`
- Test: `tests/thread_runtime.rs`
- Test: `tests/app_server_boundary.rs`

**Step 1: Write failing resume test**

Add an integration test:

```rust
#[tokio::test]
async fn thread_resume_reconstructs_context_from_rollout_without_snapshot() {
    // Create rollout.jsonl with:
    // - SessionMeta
    // - TurnContext
    // - ResponseItem user
    // - ResponseItem assistant
    // Ensure snapshot.json does not exist.
    // Resume thread through ThreadManager or ThreadSession.
    // Assert live view/history has user + assistant and reference_turn_context.
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test thread_resume_reconstructs_context_from_rollout_without_snapshot
```

Expected:

```text
FAIL because resume still depends on snapshot.
```

**Step 3: Implement rollout resume**

Thread resume should:

```text
find rollout path
read RolloutItem list
read SessionMeta
hydrate ContextManager from rollout
hydrate runtime fields from selected EventMsg / effects where supported
construct ThreadSession
```

Do not keep a snapshot fallback. Rollout is the only runtime load source.

**Step 4: Run integration tests**

Run:

```bash
cargo test thread_resume_reconstructs_context_from_rollout_without_snapshot
cargo test thread_runtime --test thread_runtime
cargo test app_server_boundary --test app_server_boundary
```

Expected:

```text
PASS
```

## Task 12: Remove Legacy Snapshot/Event Migration

**Files:**

- Modify: `src/state/rollout.rs`
- Modify: `src/state/transcript.rs`
- Test: `src/state/rollout.rs`

This task was superseded by the review decision that old sessions do not need
to be migrated. Runtime load and cold read should reject snapshot-only state
instead of synthesizing rollout from it.

**Step 1: Write failing no-fallback test**

```rust
#[test]
fn legacy_snapshot_only_thread_is_not_loaded_as_runtime_state() {
    // Write only .exagent/sessions/<thread_id>/snapshot.json.
    // Attempt ThreadSession::new.
    // Assert it returns an error because rollout is missing.
}
```

**Step 2: Remove migration helpers**

```text
delete migrate_legacy_transcript_to_rollout
delete read_session_snapshot/read_session_events/replay_session/append_runtime_event
keep only JSON helpers and compatibility path construction in transcript.rs
```

**Step 3: Run tests**

Run:

```bash
cargo test --test thread_runtime legacy_snapshot_only_thread_is_not_loaded_as_runtime_state
```

Expected:

```text
PASS
```

## Task 13: Stop Writing Snapshot/Event Files For New Sessions

**Files:**

- Modify: `src/runtime/thread_session/events.rs`
- Modify: `src/runtime/thread_session/mod.rs`
- Modify: `src/state/transcript.rs`
- Test: `tests/thread_runtime.rs`

**Step 1: Write failing no-snapshot test**

Add:

```rust
#[tokio::test]
async fn new_thread_writes_rollout_without_snapshot_or_events_files() {
    // Start a new thread and run one turn.
    // Assert rollout.jsonl exists.
    // Assert snapshot.json does not exist.
    // Assert events.jsonl does not exist.
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test new_thread_writes_rollout_without_snapshot_or_events_files
```

Expected:

```text
FAIL because ThreadEventRecorder still writes snapshot/events.
```

**Step 3: Remove new-session snapshot/event writes**

Update `ThreadEventRecorder` responsibilities:

```text
assign event ids
update live event buffer
broadcast events
persist selected event as RolloutItem::EventMsg
do not write snapshot.json
do not append events.jsonl
```

Keep `state::transcript` as migration-only.

**Step 4: Run tests**

Run:

```bash
cargo test thread_runtime --test thread_runtime
cargo test app_server_boundary --test app_server_boundary
cargo test api_server --test api_server
```

Expected:

```text
PASS
```

## Task 14: Add Compacted Replacement History Support

**Files:**

- Modify: `src/runtime/context.rs`
- Modify: `src/state/rollout.rs`
- Test: `src/runtime/context.rs`

**Step 1: Write failing compaction replay test**

Add:

```rust
#[test]
fn compacted_replacement_history_replaces_context_manager_items() {
    let items = vec![
        RolloutItem::ResponseItem(ConversationMessage::user("old")),
        RolloutItem::Compacted(CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(vec![ConversationMessage::assistant(Some("summary"), vec![])]),
        }),
        RolloutItem::ResponseItem(ConversationMessage::user("new")),
    ];

    let manager = ContextManager::from_rollout_items(&items);
    let contents = manager
        .raw_items()
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(contents, vec!["summary", "new"]);
}
```

**Step 2: Run test**

Run:

```bash
cargo test compacted_replacement_history_replaces_context_manager_items --lib
```

Expected:

```text
FAIL if compaction replay does not replace history.
```

**Step 3: Implement replay behavior**

Ensure `RolloutItem::Compacted` with `replacement_history: Some(...)` calls:

```rust
manager.replace_history(replacement_history.clone(), manager.reference_turn_context());
```

Leave automatic compaction creation for a future plan.

**Step 4: Run tests**

Run:

```bash
cargo test compacted_replacement_history_replaces_context_manager_items --lib
```

Expected:

```text
PASS
```

## Task 15: Update App Server Derived Views

**Files:**

- Modify: `src/app_server/thread_manager.rs`
- Modify: `src/app_server/protocol.rs`
- Test: `tests/app_server_boundary.rs`
- Test: `tests/api_server.rs`

**Step 1: Write failing derived-view test**

Add/update tests to assert:

```text
thread_read returns live view derived from ThreadSession ContextManager
events_replay returns selected persisted EventMsg from rollout
include_snapshot no longer exposes durable snapshot internals
```

**Step 2: Run tests**

Run:

```bash
cargo test app_server_boundary --test app_server_boundary
cargo test api_server --test api_server
```

Expected:

```text
FAIL until app_server reads rollout/live runtime views.
```

**Step 3: Update app-server reads**

Rules:

```text
loaded runtime:
  prefer ThreadRuntime live view

not loaded:
  read rollout
  reconstruct derived view

legacy only:
  return not found / unsupported unless rollout exists
```

**Step 4: Run boundary tests**

Run:

```bash
cargo test app_server_boundary --test app_server_boundary
cargo test api_server --test api_server
```

Expected:

```text
PASS
```

## Task 16: Architecture Guards

**Files:**

- Modify: `tests/architecture_guards.rs`

**Step 1: Add guards**

Add:

```rust
#[test]
fn new_runtime_does_not_write_snapshot_or_events_jsonl() {
    for path in [
        "src/runtime/thread_session/events.rs",
        "src/runtime/thread_session/turn.rs",
        "src/runtime/thread_session/mod.rs",
    ] {
        let source = std::fs::read_to_string(path).expect("read source");
        assert!(!source.contains("snapshot_path"));
        assert!(!source.contains("events_path"));
        assert!(!source.contains("write_json(&paths.snapshot_path"));
    }
}

#[test]
fn context_manager_is_stateful_history_owner() {
    let source = std::fs::read_to_string("src/runtime/context.rs").expect("read context");
    assert!(source.contains("items: Vec<ConversationMessage>"));
    assert!(source.contains("reference_turn_context: Option<TurnContextItem>"));
}

#[test]
fn rollout_schema_has_codex_style_top_level_items() {
    let source = std::fs::read_to_string("src/state/rollout.rs").expect("read rollout");
    for expected in ["SessionMeta", "ResponseItem", "TurnContext", "Compacted", "EventMsg"] {
        assert!(source.contains(expected), "missing {expected}");
    }
}
```

**Step 2: Run guards**

Run:

```bash
cargo test --test architecture_guards
```

Expected:

```text
PASS
```

## Task 17: Documentation Update

**Files:**

- Modify: `docs/architecture/2026-05-20-exagent-rollout-persistence-architecture.md`
- Modify: `docs/architecture/2026-05-18-exagent-context-projection-layer.md`
- Modify: `docs/protocol/app-server-boundary-v2.md`

**Step 1: Update docs**

Document:

```text
rollout.jsonl is source of truth
snapshot/events are compatibility-only protocol paths
ContextManager is stateful
selected EventMsg persistence policy
resume/replay behavior
```

**Step 2: Search for stale claims**

Run:

```bash
rg -n "snapshot\\.json|events\\.jsonl|SessionSnapshot|ThreadEventRecorder|source of truth|reference_turn_context" docs src -g '*.md' -g '*.rs'
```

Expected:

```text
Only compatibility-field or historical-plan references remain.
```

## Task 18: Final Verification

**Files:**

- All changed files

**Step 1: Format**

Run:

```bash
cargo fmt -- --check
```

Expected:

```text
exit 0
```

**Step 2: Check diff whitespace**

Run:

```bash
git diff --check
```

Expected:

```text
exit 0
```

**Step 3: Full tests**

Run:

```bash
cargo test
```

Expected:

```text
all tests pass
```

**Step 4: Manual invariant check**

Run:

```bash
rg -n "snapshot\\.json|events\\.jsonl|write_json\\(|append_event|snapshot_path|events_path" src/runtime src/app_server src/state -g '*.rs'
```

Expected:

```text
No new-session runtime path writes snapshot.json or events.jsonl.
Legacy migration code may still reference old paths.
```

## Rollout Risk Notes

High-risk areas:

```text
ThreadManager resume path
app-server include_snapshot compatibility
approval pending state reconstruction
exec session live references
event replay expectations
tests that assert conversation_message_count
```

Keep changes staged. If a task reveals that runtime-only fields cannot be reconstructed from rollout yet, add the minimal `RolloutItem::EventMsg` or state-bearing rollout item required by that field. Do not reintroduce snapshot authority.

## Execution Handoff

Plan complete and saved to `docs/plans/2026-05-20-exagent-rollout-persistence-migration.md`.

Recommended execution mode:

```text
Use a dedicated implementation pass with superpowers:executing-plans.
Do not run this migration in parallel with unrelated runtime refactors.
```

Implementation should begin with Task 1 and stop after each stage if architecture guards or app-server boundary tests expose hidden coupling.
