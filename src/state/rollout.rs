use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use tokio::io::AsyncWriteExt;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::session::{ThreadSnapshot, TurnContextItem};
use crate::types::{ConversationMessage, ThreadId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    ThreadMeta(ThreadMeta),
    ResponseItem(ConversationMessage),
    TurnContext(TurnContextItem),
    Compacted(CompactedItem),
    EventMsg(RuntimeEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadMeta {
    pub thread_id: ThreadId,
    pub workspace_root: PathBuf,
    pub initial_cwd: PathBuf,
    pub created_at: String,
}

pub(crate) fn thread_meta_from_snapshot(snapshot: &ThreadSnapshot) -> ThreadMeta {
    ThreadMeta {
        thread_id: snapshot.thread_id.clone(),
        workspace_root: snapshot.workspace_root.clone(),
        initial_cwd: snapshot.cwd.clone(),
        created_at: current_utc_timestamp(),
    }
}

pub fn snapshot_from_rollout_items(
    requested_thread_id: &ThreadId,
    items: &[RolloutItem],
) -> anyhow::Result<ThreadSnapshot> {
    let meta = items
        .iter()
        .find_map(|item| match item {
            RolloutItem::ThreadMeta(meta) => Some(meta),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("rollout is missing ThreadMeta"))?;
    if &meta.thread_id != requested_thread_id {
        return Err(anyhow::anyhow!(
            "rollout thread id {} does not match requested thread id {}",
            meta.thread_id.as_str(),
            requested_thread_id.as_str()
        ));
    }

    let mut conversation = Vec::new();
    let mut reference_turn_context = None;
    let mut latest_compaction = None;
    for item in items {
        match item {
            RolloutItem::ResponseItem(message) => conversation.push(message.clone()),
            RolloutItem::TurnContext(context) => reference_turn_context = Some(context.clone()),
            RolloutItem::Compacted(compacted) => {
                latest_compaction = Some(crate::session::CompactionSummary {
                    summary: compacted.message.clone(),
                    source_event_ids: vec![],
                });
                if let Some(replacement_history) = &compacted.replacement_history {
                    conversation = replacement_history.clone();
                }
            }
            RolloutItem::ThreadMeta(_) | RolloutItem::EventMsg(_) => {}
        }
    }

    let snapshot = ThreadSnapshot {
        thread_id: meta.thread_id.clone(),
        workspace_root: meta.workspace_root.clone(),
        cwd: meta.initial_cwd.clone(),
        reference_turn_context,
        conversation,
        open_exec_sessions: vec![],
        latest_compaction,
        pending_approvals: vec![],
    };
    Ok(snapshot)
}

pub fn events_from_rollout_items(items: &[RolloutItem]) -> Vec<RuntimeEvent> {
    items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::EventMsg(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactedItem {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_history: Option<Vec<ConversationMessage>>,
}

pub fn should_persist_rollout_item(item: &RolloutItem) -> bool {
    match item {
        RolloutItem::ThreadMeta(_)
        | RolloutItem::ResponseItem(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::Compacted(_) => true,
        RolloutItem::EventMsg(event) => should_persist_event(event),
    }
}

fn should_persist_event(event: &RuntimeEvent) -> bool {
    matches!(
        &event.kind,
        RuntimeEventKind::TurnStarted
            | RuntimeEventKind::TurnCompleted
            | RuntimeEventKind::TurnInterrupted
            | RuntimeEventKind::RuntimeError { .. }
            | RuntimeEventKind::ApprovalRequested { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::TokenCount { .. }
    )
}

#[derive(Debug, Clone)]
pub struct RolloutStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RolloutPaths {
    pub thread_dir: PathBuf,
    pub rollout_path: PathBuf,
}

pub fn rollout_paths(workspace_root: &std::path::Path, thread_id: &ThreadId) -> RolloutPaths {
    let thread_dir = workspace_root
        .join(".exagent")
        .join("threads")
        .join(thread_id.as_str());
    RolloutPaths {
        rollout_path: thread_dir.join("rollout.jsonl"),
        thread_dir,
    }
}

fn current_utc_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

impl RolloutStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        self.path.as_path()
    }

    pub async fn append_items(&self, items: &[RolloutItem]) -> std::io::Result<()> {
        let items = items
            .iter()
            .filter(|item| should_persist_rollout_item(item))
            .collect::<Vec<_>>();
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

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await
    }

    pub fn append_items_blocking(&self, items: &[RolloutItem]) -> std::io::Result<()> {
        let items = items
            .iter()
            .filter(|item| should_persist_rollout_item(item))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        for item in items {
            writeln!(
                file,
                "{}",
                serde_json::to_string(item).map_err(std::io::Error::other)?
            )?;
        }
        file.flush()
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

    pub fn read_items_blocking(path: &std::path::Path) -> std::io::Result<Vec<RolloutItem>> {
        let text = match std::fs::read_to_string(path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::resolved::ModelRef;
    use crate::session::{ApprovalId, TurnContextItem};
    use crate::types::{
        ConversationMessage, EventId, ThreadId, TokenUsage, TokenUsageInfo, TurnId,
    };
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
        let meta = RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: ThreadId::new("thread_1"),
            workspace_root: PathBuf::from("/workspace"),
            initial_cwd: PathBuf::from("/workspace"),
            created_at: "2026-05-20T00:00:00Z".to_string(),
        });
        let context = RolloutItem::TurnContext(TurnContextItem {
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "mock"),
            policy_mode: crate::policy::PolicyMode::Off,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            thinking_mode: None,
            current_utc_date: Some("2026-05-20".to_string()),
        });
        let compacted = RolloutItem::Compacted(CompactedItem {
            message: "summary".to_string(),
            replacement_history: Some(vec![ConversationMessage::assistant(
                Some("summary".to_string()),
                vec![],
            )]),
        });
        let event = RolloutItem::EventMsg(RuntimeEvent {
            event_id: EventId::new("evt_1"),
            thread_id: ThreadId::new("thread_1"),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::TurnStarted,
        });

        serde_json::to_string(&meta).expect("serialize meta");
        serde_json::to_string(&context).expect("serialize context");
        serde_json::to_string(&compacted).expect("serialize compacted");
        serde_json::to_string(&event).expect("serialize event");
    }

    #[tokio::test]
    async fn rollout_store_appends_and_reads_jsonl_items() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let store = RolloutStore::new(path.clone());

        store
            .append_items(&[
                RolloutItem::ResponseItem(ConversationMessage::user("first")),
                RolloutItem::ResponseItem(ConversationMessage::assistant(
                    Some("second".to_string()),
                    vec![],
                )),
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

    #[test]
    fn event_persistence_policy_filters_runtime_events() {
        let turn_started = RuntimeEvent {
            event_id: EventId::new("event_1"),
            thread_id: ThreadId::new("thread_1"),
            turn_id: Some(TurnId::new("turn_1")),
            kind: RuntimeEventKind::TurnStarted,
        };
        let exec_output = RuntimeEvent {
            kind: RuntimeEventKind::ExecOutput {
                exec_session_id: crate::session::ExecSessionId::new("exec_1"),
                stream: crate::events::ExecOutputStream::Stdout,
                chunk: "streaming chunk".to_string(),
            },
            ..turn_started.clone()
        };
        let token_count = RuntimeEvent {
            kind: RuntimeEventKind::TokenCount {
                info: Some(TokenUsageInfo {
                    total_token_usage: TokenUsage {
                        total_tokens: 100,
                        ..TokenUsage::default()
                    },
                    last_token_usage: TokenUsage {
                        total_tokens: 25,
                        ..TokenUsage::default()
                    },
                    model_context_window: Some(1_000),
                }),
            },
            ..turn_started.clone()
        };

        assert!(should_persist_rollout_item(&RolloutItem::EventMsg(
            turn_started
        )));
        assert!(should_persist_rollout_item(&RolloutItem::EventMsg(
            token_count
        )));
        assert!(!should_persist_rollout_item(&RolloutItem::EventMsg(
            exec_output
        )));
    }

    #[test]
    fn rollout_snapshot_does_not_restore_live_only_runtime_state() {
        let thread_id = ThreadId::new("session_overlay_cold");
        let workspace_root = PathBuf::from("/tmp/exagent-overlay");
        let snapshot = crate::session::ThreadSnapshot::new_thread(
            thread_id.clone(),
            workspace_root.clone(),
            workspace_root.clone(),
        );

        let items = vec![
            RolloutItem::ThreadMeta(thread_meta_from_snapshot(&snapshot)),
            RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: thread_id.clone(),
                turn_id: Some(TurnId::new("turn_1")),
                kind: RuntimeEventKind::ApprovalRequested {
                    approval_id: ApprovalId::new("approval_1"),
                    tool_name: "run_command".to_string(),
                    reason: "approval required".to_string(),
                },
            }),
        ];

        let rebuilt = snapshot_from_rollout_items(&thread_id, &items).expect("rebuild snapshot");

        assert!(rebuilt.pending_approvals.is_empty());
        assert!(rebuilt.open_exec_sessions.is_empty());
    }

    #[test]
    fn rollout_path_uses_thread_directory_and_rollout_jsonl() {
        let workspace_root = PathBuf::from("/workspace");
        let thread_id = ThreadId::new("thread_1");

        let paths = rollout_paths(&workspace_root, &thread_id);

        assert!(paths.thread_dir.ends_with("thread_1"));
        assert!(paths.rollout_path.ends_with("rollout.jsonl"));
    }
}
