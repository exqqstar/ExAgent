use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use tokio::io::AsyncWriteExt;

use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::session::{ThreadLineage, ThreadSnapshot, ThreadSource, TurnContextItem};
use crate::types::{ConversationMessage, ThreadId, TurnId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    ThreadMeta(ThreadMeta),
    ResponseItem(ResponseItem),
    TurnContext(TurnContextItem),
    Compacted(CompactedItem),
    EventMsg(RuntimeEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseItem {
    pub turn_id: TurnId,
    #[serde(flatten)]
    pub message: ConversationMessage,
}

impl ResponseItem {
    pub fn for_turn(turn_id: TurnId, message: ConversationMessage) -> Self {
        Self { turn_id, message }
    }
}

impl RolloutItem {
    pub fn response_item_for_turn(turn_id: TurnId, message: ConversationMessage) -> Self {
        Self::ResponseItem(ResponseItem::for_turn(turn_id, message))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadMeta {
    pub thread_id: ThreadId,
    pub workspace_root: PathBuf,
    pub initial_cwd: PathBuf,
    #[serde(default)]
    pub permission_profile: crate::config::PermissionProfile,
    #[serde(default)]
    pub thread_source: ThreadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<ThreadLineage>,
    pub created_at: String,
}

pub(crate) fn thread_meta_from_snapshot(snapshot: &ThreadSnapshot) -> ThreadMeta {
    ThreadMeta {
        thread_id: snapshot.thread_id.clone(),
        workspace_root: snapshot.workspace_root.clone(),
        initial_cwd: snapshot.cwd.clone(),
        permission_profile: snapshot.permission_profile,
        thread_source: snapshot.thread_source.clone(),
        lineage: snapshot.lineage.clone(),
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
    let mut token_info = None;
    for item in items {
        match item {
            RolloutItem::ResponseItem(response_item) => {
                conversation.push(response_item.message.clone())
            }
            RolloutItem::TurnContext(context) => reference_turn_context = Some(context.clone()),
            RolloutItem::Compacted(compacted) => {
                latest_compaction = Some(crate::session::CompactionSummary {
                    summary: compacted.message.clone(),
                    source_event_ids: vec![],
                });
                if let Some(replacement_history) = &compacted.replacement_history {
                    conversation = replacement_history.clone();
                    token_info = None;
                }
            }
            RolloutItem::EventMsg(event) => {
                if let RuntimeEventKind::TokenCount { info } = &event.kind {
                    token_info = info.clone();
                }
            }
            RolloutItem::ThreadMeta(_) => {}
        }
    }

    let snapshot = ThreadSnapshot {
        thread_id: meta.thread_id.clone(),
        workspace_root: meta.workspace_root.clone(),
        cwd: meta.initial_cwd.clone(),
        permission_profile: meta.permission_profile,
        thread_source: meta.thread_source.clone(),
        lineage: meta.lineage.clone(),
        reference_turn_context,
        conversation,
        open_exec_sessions: vec![],
        latest_compaction,
        token_info,
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

pub fn response_items_from_rollout_items(items: &[RolloutItem]) -> Vec<ResponseItem> {
    items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(response_item) => Some(response_item.clone()),
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
            | RuntimeEventKind::Reasoning { .. }
            | RuntimeEventKind::RuntimeError { .. }
            | RuntimeEventKind::ToolResult { .. }
            | RuntimeEventKind::ToolInvocationStarted { .. }
            | RuntimeEventKind::ToolInvocationWaitingApproval { .. }
            | RuntimeEventKind::ToolInvocationCompleted { .. }
            | RuntimeEventKind::ToolInvocationFailed { .. }
            | RuntimeEventKind::ToolInvocationCancelled { .. }
            | RuntimeEventKind::ApprovalRequested { .. }
            | RuntimeEventKind::ApprovalDecision { .. }
            | RuntimeEventKind::SubagentSpawned { .. }
            | RuntimeEventKind::SubagentClosed { .. }
            | RuntimeEventKind::InterAgentMessageSent { .. }
            | RuntimeEventKind::TokenCount { .. }
            | RuntimeEventKind::ThreadGoalTurnStarted { .. }
            | RuntimeEventKind::ThreadGoalToolCompleted { .. }
            | RuntimeEventKind::ThreadGoalReport { .. }
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
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn rollout_item_serializes_with_snake_case_type_tag() {
        let item = RolloutItem::response_item_for_turn(
            TurnId::new("turn_1"),
            ConversationMessage::user("hello"),
        );
        let value = serde_json::to_value(item).expect("serialize rollout item");

        assert_eq!(value["type"], "response_item");
        assert_eq!(value["payload"]["turn_id"], "turn_1");
        assert_eq!(value["payload"]["content"], "hello");
    }

    #[test]
    fn response_item_serializes_required_turn_id() {
        let item = RolloutItem::response_item_for_turn(
            TurnId::new("turn_2"),
            ConversationMessage::user("hello"),
        );
        let value = serde_json::to_value(item).expect("serialize rollout item");

        assert_eq!(value["type"], "response_item");
        assert_eq!(value["payload"]["turn_id"], "turn_2");
        assert_eq!(value["payload"]["role"], "user");
        assert_eq!(value["payload"]["content"], "hello");
    }

    #[test]
    fn response_item_requires_turn_id() {
        let result = serde_json::from_value::<RolloutItem>(json!({
            "type": "response_item",
            "payload": {
                "role": "assistant",
                "content": "untagged answer"
            }
        }));

        assert!(result.is_err());
    }

    #[test]
    fn rollout_item_can_store_session_meta_turn_context_and_compaction() {
        let meta = RolloutItem::ThreadMeta(ThreadMeta {
            thread_id: ThreadId::new("thread_1"),
            workspace_root: PathBuf::from("/workspace"),
            initial_cwd: PathBuf::from("/workspace"),
            permission_profile: crate::config::PermissionProfile::FullAccess,
            thread_source: Default::default(),
            lineage: None,
            created_at: "2026-05-20T00:00:00Z".to_string(),
        });
        let context = RolloutItem::TurnContext(TurnContextItem {
            turn_id: TurnId::new("turn_1"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "mock"),
            policy_mode: crate::policy::PolicyMode::Off,
            permission_profile: crate::config::PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            turn_mode: crate::runtime::turn_mode::TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
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
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_1"),
                    ConversationMessage::user("first"),
                ),
                RolloutItem::response_item_for_turn(
                    TurnId::new("turn_2"),
                    ConversationMessage::assistant(Some("second".to_string()), vec![]),
                ),
            ])
            .await
            .expect("append rollout items");

        let items = RolloutStore::read_items(&path).await.expect("read rollout");
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("first")
            )
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
                sequence: 1,
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
    fn goal_report_events_are_persisted_for_thread_history() {
        let thread_id = ThreadId::new("thread_goal_report_rollout");
        assert!(should_persist_rollout_item(&RolloutItem::EventMsg(
            RuntimeEvent {
                event_id: EventId::new("evt_goal_report"),
                thread_id,
                turn_id: Some(TurnId::new("turn_goal_report")),
                kind: RuntimeEventKind::ThreadGoalReport {
                    report: crate::app_server::protocol::ThreadGoalReport {
                        goal_id: "goal_1".to_string(),
                        objective: "ship report".to_string(),
                        final_status: crate::app_server::protocol::ThreadGoalStatus::Complete,
                        turns_run: 2,
                        tokens_used: 120,
                        token_budget: Some(200),
                        time_used_seconds: 30,
                        changed_files: vec!["src/runtime/goal/runtime.rs".to_string()],
                        pending_approvals_count: 1,
                        summary: "The goal completed.".to_string(),
                    },
                },
            }
        )));
    }

    #[test]
    fn compacted_replacement_history_clears_stale_token_info_from_snapshot() {
        let thread_id = ThreadId::new("thread_compacted_tokens");
        let workspace_root = PathBuf::from("/tmp/compacted-tokens");
        let snapshot = crate::session::ThreadSnapshot::new_thread(
            thread_id.clone(),
            workspace_root.clone(),
            workspace_root,
        );

        let rebuilt = snapshot_from_rollout_items(
            &thread_id,
            &[
                RolloutItem::ThreadMeta(thread_meta_from_snapshot(&snapshot)),
                RolloutItem::EventMsg(RuntimeEvent {
                    event_id: EventId::new("evt_token"),
                    thread_id: thread_id.clone(),
                    turn_id: Some(TurnId::new("turn_1")),
                    kind: RuntimeEventKind::TokenCount {
                        info: Some(TokenUsageInfo {
                            total_token_usage: TokenUsage {
                                total_tokens: 500,
                                ..TokenUsage::default()
                            },
                            last_token_usage: TokenUsage {
                                total_tokens: 500,
                                ..TokenUsage::default()
                            },
                            model_context_window: Some(1_000),
                        }),
                    },
                }),
                RolloutItem::Compacted(CompactedItem {
                    message: "summary".to_string(),
                    replacement_history: Some(vec![ConversationMessage::assistant(
                        Some("summary".to_string()),
                        vec![],
                    )]),
                }),
            ],
        )
        .expect("rebuild snapshot");

        assert_eq!(rebuilt.token_info, None);
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
                    checkpoint_id: None,
                    permission_profile: crate::config::PermissionProfile::FullAccess,
                    filesystem_sandbox: crate::config::default_boundary_none(),
                    network_sandbox: crate::config::default_boundary_none(),
                    env_isolation: crate::config::default_boundary_none(),
                    command: None,
                },
            }),
        ];

        let rebuilt = snapshot_from_rollout_items(&thread_id, &items).expect("rebuild snapshot");

        assert!(rebuilt.pending_approvals.is_empty());
        assert!(rebuilt.open_exec_sessions.is_empty());
    }

    #[test]
    fn thread_meta_round_trips_permission_profile() {
        let snapshot = crate::session::ThreadSnapshot::new_thread_with_permission_profile(
            ThreadId::new("thread_profile_rollout"),
            PathBuf::from("/tmp/profile-workspace"),
            PathBuf::from("/tmp/profile-workspace"),
            crate::config::PermissionProfile::FullAccess,
        );

        let meta = thread_meta_from_snapshot(&snapshot);
        assert_eq!(
            meta.permission_profile,
            crate::config::PermissionProfile::FullAccess
        );

        let rebuilt =
            snapshot_from_rollout_items(&snapshot.thread_id, &[RolloutItem::ThreadMeta(meta)])
                .expect("rebuild snapshot from rollout");

        assert_eq!(
            rebuilt.permission_profile,
            crate::config::PermissionProfile::FullAccess
        );
    }

    #[test]
    fn old_thread_meta_without_permission_profile_rebuilds_full_access_snapshot() {
        let thread_id = ThreadId::new("thread_old_profile_rollout");
        let item: RolloutItem = serde_json::from_value(json!({
            "type": "thread_meta",
            "payload": {
                "thread_id": "thread_old_profile_rollout",
                "workspace_root": "/tmp/old-profile-workspace",
                "initial_cwd": "/tmp/old-profile-workspace",
                "created_at": "2026-05-20T00:00:00Z"
            }
        }))
        .expect("deserialize old thread meta");

        let rebuilt =
            snapshot_from_rollout_items(&thread_id, &[item]).expect("rebuild old rollout");

        assert_eq!(
            rebuilt.permission_profile,
            crate::config::PermissionProfile::FullAccess
        );
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
