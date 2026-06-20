use std::collections::HashSet;

use crate::runtime::subagent::InterAgentCommunication;
use crate::state::rollout::{CompactedItem, ResponseItem, RolloutItem};
use crate::types::{ConversationMessage, MessageRole, TurnId};

pub(crate) const FORK_CONTEXT_TURN_ID: &str = "fork_context";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkTurns {
    None,
    All,
    Last(usize),
}

impl ForkTurns {
    pub fn label(self) -> String {
        match self {
            ForkTurns::None => "none".to_string(),
            ForkTurns::All => "all".to_string(),
            ForkTurns::Last(count) => count.to_string(),
        }
    }
}

pub fn build_fork_history(parent_items: &[RolloutItem], fork_turns: ForkTurns) -> Vec<RolloutItem> {
    match fork_turns {
        ForkTurns::None => Vec::new(),
        ForkTurns::All => {
            let mut forked = parent_items
                .iter()
                .filter_map(|item| filter_item(item, true, None, fork_turns))
                .collect::<Vec<_>>();
            strip_incomplete_tool_call_groups(&mut forked);
            forked
        }
        ForkTurns::Last(count) => {
            if count == 0 {
                return Vec::new();
            }
            let selected_turn_ids = last_boundary_turn_ids(parent_items, count);
            if selected_turn_ids.is_empty() {
                return Vec::new();
            }
            let mut forked = parent_items
                .iter()
                .filter_map(|item| filter_item(item, false, Some(&selected_turn_ids), fork_turns))
                .collect::<Vec<_>>();
            strip_incomplete_tool_call_groups(&mut forked);
            forked
        }
    }
}

/// Prefix fork for user-facing thread forking. Unlike `build_fork_history`,
/// this preserves original turn ids and `TurnContext` items so the forked
/// thread's transcript replays identically to the parent up to the fork point.
pub fn build_thread_fork_history(
    parent_items: &[RolloutItem],
    fork_point_turn_id: &TurnId,
) -> anyhow::Result<Vec<RolloutItem>> {
    let mut boundary_index = None;
    let mut found_response = false;

    for (index, item) in parent_items.iter().enumerate() {
        match item {
            RolloutItem::ResponseItem(response) if &response.turn_id == fork_point_turn_id => {
                found_response = true;
                boundary_index = Some(index);
            }
            RolloutItem::TurnContext(context) if &context.turn_id == fork_point_turn_id => {
                boundary_index = Some(index);
            }
            _ => {}
        }
    }

    if !found_response {
        return Err(anyhow::anyhow!(
            "fork point turn id {} was not found in parent history",
            fork_point_turn_id.as_str()
        ));
    }

    let boundary_index = boundary_index.expect("found response must set fork boundary");
    let mut forked = parent_items[..=boundary_index]
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ThreadMeta(_) | RolloutItem::EventMsg(_) | RolloutItem::WorkflowRun(_) => {
                None
            }
            RolloutItem::ResponseItem(_)
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_) => Some(item.clone()),
        })
        .collect::<Vec<_>>();
    strip_incomplete_tool_call_groups(&mut forked);
    Ok(forked)
}

fn filter_item(
    item: &RolloutItem,
    include_turn_context: bool,
    selected_turn_ids: Option<&HashSet<TurnId>>,
    fork_turns: ForkTurns,
) -> Option<RolloutItem> {
    match item {
        RolloutItem::ThreadMeta(_) | RolloutItem::EventMsg(_) | RolloutItem::WorkflowRun(_) => None,
        RolloutItem::TurnContext(_) => None,
        RolloutItem::Compacted(compacted) => Some(RolloutItem::Compacted(filter_compacted(
            compacted, fork_turns,
        ))),
        RolloutItem::ResponseItem(response) => {
            if let Some(selected_turn_ids) = selected_turn_ids {
                if !selected_turn_ids.contains(&response.turn_id) {
                    return None;
                }
            }
            if !include_turn_context && is_runtime_context_message(&response.message) {
                return None;
            }
            Some(RolloutItem::ResponseItem(ResponseItem::for_turn(
                TurnId::new(FORK_CONTEXT_TURN_ID),
                response.message.clone(),
            )))
        }
    }
}

fn filter_compacted(compacted: &CompactedItem, fork_turns: ForkTurns) -> CompactedItem {
    let replacement_history = compacted.replacement_history.as_ref().map(|history| {
        let mut history = match fork_turns {
            ForkTurns::None => Vec::new(),
            ForkTurns::All => history.clone(),
            ForkTurns::Last(count) => truncate_messages_to_last_boundaries(history, count),
        };
        history.retain(|message| !is_runtime_context_message(message));
        history = strip_incomplete_tool_messages(history);
        history
    });
    CompactedItem {
        message: compacted.message.clone(),
        replacement_history,
    }
}

fn last_boundary_turn_ids(parent_items: &[RolloutItem], count: usize) -> HashSet<TurnId> {
    let mut ordered = Vec::<TurnId>::new();
    for item in parent_items {
        let RolloutItem::ResponseItem(response) = item else {
            continue;
        };
        if !is_fork_turn_boundary(&response.message) {
            continue;
        }
        let turn_id = response.turn_id.clone();
        if ordered.last() != Some(&turn_id) {
            ordered.push(turn_id);
        }
    }
    ordered
        .into_iter()
        .rev()
        .take(count)
        .collect::<HashSet<_>>()
}

fn truncate_messages_to_last_boundaries(
    messages: &[ConversationMessage],
    count: usize,
) -> Vec<ConversationMessage> {
    if count == 0 {
        return Vec::new();
    }
    let boundary_indexes = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| is_fork_turn_boundary(message).then_some(index))
        .collect::<Vec<_>>();
    let Some(start_index) = boundary_indexes
        .len()
        .checked_sub(count)
        .and_then(|index| boundary_indexes.get(index))
        .copied()
    else {
        return Vec::new();
    };
    messages[start_index..].to_vec()
}

fn is_fork_turn_boundary(message: &ConversationMessage) -> bool {
    is_real_user_message(message) || is_triggering_mailbox_message(message)
}

fn is_real_user_message(message: &ConversationMessage) -> bool {
    message.role == MessageRole::User && !message.injected
}

fn is_triggering_mailbox_message(message: &ConversationMessage) -> bool {
    InterAgentCommunication::from_conversation_message(message)
        .is_some_and(|mail| mail.trigger_turn)
}

fn is_runtime_context_message(message: &ConversationMessage) -> bool {
    message.injected && InterAgentCommunication::from_conversation_message(message).is_none()
}

fn strip_incomplete_tool_call_groups(items: &mut Vec<RolloutItem>) {
    let mut sanitized = Vec::with_capacity(items.len());
    let mut response_group = Vec::new();

    for item in std::mem::take(items) {
        match item {
            RolloutItem::ResponseItem(response) => response_group.push(response),
            non_response => {
                sanitized.extend(
                    strip_incomplete_tool_response_items(std::mem::take(&mut response_group))
                        .into_iter()
                        .map(RolloutItem::ResponseItem),
                );
                sanitized.push(non_response);
            }
        }
    }
    sanitized.extend(
        strip_incomplete_tool_response_items(response_group)
            .into_iter()
            .map(RolloutItem::ResponseItem),
    );
    *items = sanitized;
}

fn strip_incomplete_tool_response_items(items: Vec<ResponseItem>) -> Vec<ResponseItem> {
    let mut sanitized = Vec::with_capacity(items.len());
    let mut index = 0;
    while index < items.len() {
        let message = &items[index].message;
        if has_tool_call(message) {
            if let Some(end) = complete_tool_group_end_for_responses(&items, index) {
                sanitized.extend(items[index..end].iter().cloned());
                index = end;
            } else {
                index = skip_following_tool_responses(&items, index + 1);
            }
            continue;
        }
        if message.role == MessageRole::Tool {
            index += 1;
            continue;
        }
        sanitized.push(items[index].clone());
        index += 1;
    }
    sanitized
}

fn strip_incomplete_tool_messages(messages: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
    let mut sanitized = Vec::with_capacity(messages.len());
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if has_tool_call(message) {
            if let Some(end) = complete_tool_group_end_for_messages(&messages, index) {
                sanitized.extend(messages[index..end].iter().cloned());
                index = end;
            } else {
                index = skip_following_tool_messages(&messages, index + 1);
            }
            continue;
        }
        if message.role == MessageRole::Tool {
            index += 1;
            continue;
        }
        sanitized.push(message.clone());
        index += 1;
    }
    sanitized
}

fn complete_tool_group_end_for_responses(items: &[ResponseItem], index: usize) -> Option<usize> {
    complete_tool_group_end(index, items.len(), |message_index| {
        &items[message_index].message
    })
}

fn complete_tool_group_end_for_messages(
    messages: &[ConversationMessage],
    index: usize,
) -> Option<usize> {
    complete_tool_group_end(index, messages.len(), |message_index| {
        &messages[message_index]
    })
}

fn complete_tool_group_end<'a>(
    index: usize,
    len: usize,
    message_at: impl Fn(usize) -> &'a ConversationMessage,
) -> Option<usize> {
    let assistant = message_at(index);
    let expected = assistant
        .tool_calls
        .iter()
        .map(|call| call.id.clone())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut cursor = index + 1;

    while cursor < len {
        let message = message_at(cursor);
        if message.role != MessageRole::Tool {
            break;
        }
        let Some(tool_call_id) = message.tool_call_id.as_ref() else {
            return None;
        };
        if !expected.contains(tool_call_id) || !seen.insert(tool_call_id.clone()) {
            return None;
        }
        cursor += 1;
        if seen.len() == expected.len() {
            return Some(cursor);
        }
    }
    None
}

fn skip_following_tool_responses(items: &[ResponseItem], mut index: usize) -> usize {
    while index < items.len() && items[index].message.role == MessageRole::Tool {
        index += 1;
    }
    index
}

fn skip_following_tool_messages(messages: &[ConversationMessage], mut index: usize) -> usize {
    while index < messages.len() && messages[index].role == MessageRole::Tool {
        index += 1;
    }
    index
}

fn has_tool_call(message: &ConversationMessage) -> bool {
    message.role == MessageRole::Assistant && !message.tool_calls.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::PermissionProfile;
    use crate::events::{RuntimeEvent, RuntimeEventKind};
    use crate::policy::PolicyMode;
    use crate::resolved::ModelRef;
    use crate::runtime::subagent::InterAgentCommunication;
    use crate::runtime::turn_mode::TurnMode;
    use crate::session::TurnContextItem;
    use crate::state::rollout::{ResponseItem, ThreadMeta};
    use crate::types::{ConversationMessage, EventId, ThreadId, ToolCall, TurnId};

    #[test]
    fn fork_history_all_filters_parent_meta_events_and_trailing_spawn_call() {
        let parent_items = vec![
            RolloutItem::ThreadMeta(parent_meta()),
            RolloutItem::TurnContext(parent_turn_context("turn_99")),
            response("turn_1", ConversationMessage::user("parent fact")),
            response(
                "turn_1",
                ConversationMessage::assistant(Some("parent answer".into()), vec![]),
            ),
            RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: ThreadId::new("thread_parent"),
                turn_id: Some(TurnId::new("turn_1")),
                kind: RuntimeEventKind::TurnCompleted,
            }),
            response("turn_2", ConversationMessage::user("fork child")),
            response(
                "turn_2",
                ConversationMessage::assistant(
                    None,
                    vec![ToolCall {
                        id: "call_spawn".into(),
                        name: "spawn_agent".into(),
                        arguments: serde_json::json!({
                            "task_name": "review",
                            "message": "review this",
                            "fork_turns": "all"
                        }),
                        thought_signature: None,
                    }],
                ),
            ),
        ];

        let forked = build_fork_history(&parent_items, ForkTurns::All);
        assert!(forked.iter().all(|item| {
            !matches!(
                item,
                RolloutItem::ThreadMeta(_) | RolloutItem::EventMsg(_) | RolloutItem::TurnContext(_)
            )
        }));
        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("parent fact")));
        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("parent answer")));
        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("fork child")));
        assert!(forked.iter().all(|item| match item {
            RolloutItem::ResponseItem(response) =>
                response.turn_id == TurnId::new(FORK_CONTEXT_TURN_ID),
            _ => true,
        }));
        assert!(!forked.iter().any(|item| match item {
            RolloutItem::ResponseItem(response) => {
                response
                    .message
                    .tool_calls
                    .iter()
                    .any(|call| call.name == "spawn_agent")
            }
            _ => false,
        }));
    }

    #[test]
    fn fork_history_all_drops_partially_completed_tool_call_group() {
        let parent_items = vec![
            response("turn_1", ConversationMessage::user("spawn three children")),
            response(
                "turn_1",
                ConversationMessage::assistant(
                    Some("spawning".into()),
                    vec![
                        ToolCall {
                            id: "call_explorer".into(),
                            name: "spawn_agent".into(),
                            arguments: serde_json::json!({"task_name": "explorer"}),
                            thought_signature: None,
                        },
                        ToolCall {
                            id: "call_reviewer".into(),
                            name: "spawn_agent".into(),
                            arguments: serde_json::json!({"task_name": "reviewer"}),
                            thought_signature: None,
                        },
                        ToolCall {
                            id: "call_planner".into(),
                            name: "spawn_agent".into(),
                            arguments: serde_json::json!({"task_name": "planner"}),
                            thought_signature: None,
                        },
                    ],
                ),
            ),
            response(
                "turn_1",
                ConversationMessage::tool("call_explorer", "explorer spawned"),
            ),
        ];

        let forked = build_fork_history(&parent_items, ForkTurns::All);

        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("spawn three children")));
        assert!(!forked.iter().any(|item| match item {
            RolloutItem::ResponseItem(response) =>
                response.message.role == MessageRole::Assistant
                    && !response.message.tool_calls.is_empty(),
            _ => false,
        }));
        assert!(!forked.iter().any(|item| match item {
            RolloutItem::ResponseItem(response) => response.message.role == MessageRole::Tool,
            _ => false,
        }));
    }

    #[test]
    fn fork_history_last_n_counts_user_turns_and_triggering_mailbox_envelopes() {
        let parent = ThreadId::new("thread_parent");
        let child = ThreadId::new("thread_child");
        let parent_items = vec![
            RolloutItem::ThreadMeta(parent_meta()),
            response("turn_1", ConversationMessage::user("old user turn")),
            response(
                "turn_1",
                ConversationMessage::assistant(Some("old answer".into()), vec![]),
            ),
            response(
                "turn_2",
                InterAgentCommunication {
                    author_thread_id: parent.clone(),
                    author_path: "/root".into(),
                    recipient_thread_id: child.clone(),
                    recipient_path: "/root/review".into(),
                    other_recipients: Vec::new(),
                    content: "triggering followup".into(),
                    trigger_turn: true,
                    source_turn_id: Some(TurnId::new("turn_1")),
                    created_at: "2026-06-04T00:00:00Z".into(),
                }
                .to_conversation_message(),
            ),
            response(
                "turn_2",
                ConversationMessage::assistant(Some("followup answer".into()), vec![]),
            ),
            response(
                "turn_3",
                InterAgentCommunication {
                    author_thread_id: parent,
                    author_path: "/root".into(),
                    recipient_thread_id: child,
                    recipient_path: "/root/review".into(),
                    other_recipients: Vec::new(),
                    content: "send only".into(),
                    trigger_turn: false,
                    source_turn_id: Some(TurnId::new("turn_2")),
                    created_at: "2026-06-04T00:00:01Z".into(),
                }
                .to_conversation_message(),
            ),
            response(
                "turn_3",
                ConversationMessage::assistant(Some("send-only answer".into()), vec![]),
            ),
        ];

        let forked = build_fork_history(&parent_items, ForkTurns::Last(1));
        assert!(!forked
            .iter()
            .any(|item| response_content(item) == Some("old user turn")));
        assert!(forked.iter().any(|item| response_content(item)
            .is_some_and(|content| content.contains("triggering followup"))));
        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("followup answer")));
        assert!(!forked.iter().any(
            |item| response_content(item).is_some_and(|content| content.contains("send only"))
        ));
        assert!(!forked
            .iter()
            .any(|item| response_content(item) == Some("send-only answer")));
    }

    #[test]
    fn thread_fork_history_keeps_items_up_to_and_including_fork_turn() {
        let compacted = RolloutItem::Compacted(CompactedItem {
            message: "summary before fork".into(),
            replacement_history: Some(vec![ConversationMessage::user("replacement")]),
        });
        let turn_1_context = RolloutItem::TurnContext(parent_turn_context("turn_1"));
        let turn_2_context = RolloutItem::TurnContext(parent_turn_context("turn_2"));
        let parent_items = vec![
            RolloutItem::ThreadMeta(parent_meta()),
            turn_1_context.clone(),
            response("turn_1", ConversationMessage::user("first user")),
            response(
                "turn_1",
                ConversationMessage::assistant(Some("first answer".into()), vec![]),
            ),
            compacted.clone(),
            RolloutItem::EventMsg(RuntimeEvent {
                event_id: EventId::new("evt_1"),
                thread_id: ThreadId::new("thread_parent"),
                turn_id: Some(TurnId::new("turn_1")),
                kind: RuntimeEventKind::TurnCompleted,
            }),
            turn_2_context.clone(),
            response("turn_2", ConversationMessage::user("second user")),
            response(
                "turn_2",
                ConversationMessage::assistant(Some("second answer".into()), vec![]),
            ),
            RolloutItem::TurnContext(parent_turn_context("turn_3")),
            response("turn_3", ConversationMessage::user("third user")),
            response(
                "turn_3",
                ConversationMessage::assistant(Some("third answer".into()), vec![]),
            ),
        ];

        let forked = build_thread_fork_history(&parent_items, &TurnId::new("turn_2"))
            .expect("thread fork history should build");

        assert_eq!(
            forked,
            vec![
                turn_1_context,
                response("turn_1", ConversationMessage::user("first user")),
                response(
                    "turn_1",
                    ConversationMessage::assistant(Some("first answer".into()), vec![]),
                ),
                compacted,
                turn_2_context,
                response("turn_2", ConversationMessage::user("second user")),
                response(
                    "turn_2",
                    ConversationMessage::assistant(Some("second answer".into()), vec![]),
                ),
            ]
        );
        assert_eq!(
            response_turn_ids(&forked),
            vec![
                TurnId::new("turn_1"),
                TurnId::new("turn_1"),
                TurnId::new("turn_2"),
                TurnId::new("turn_2"),
            ]
        );
        assert!(forked.iter().all(|item| {
            !matches!(item, RolloutItem::ThreadMeta(_) | RolloutItem::EventMsg(_))
        }));
        assert!(!forked
            .iter()
            .any(|item| response_content(item) == Some("third user")));
        assert!(!forked
            .iter()
            .any(|item| response_content(item) == Some("third answer")));
    }

    #[test]
    fn thread_fork_history_rejects_unknown_turn_id() {
        let parent_items = vec![
            response("turn_1", ConversationMessage::user("first user")),
            response(
                "turn_1",
                ConversationMessage::assistant(Some("first answer".into()), vec![]),
            ),
        ];

        let error = build_thread_fork_history(&parent_items, &TurnId::new("turn_missing"))
            .expect_err("unknown turn id should be rejected");

        assert!(error.to_string().contains("turn_missing"));
    }

    #[test]
    fn thread_fork_history_strips_incomplete_tool_call_groups_at_boundary() {
        let parent_items = vec![
            response("turn_1", ConversationMessage::user("first user")),
            response(
                "turn_1",
                ConversationMessage::assistant(Some("first answer".into()), vec![]),
            ),
            response("turn_2", ConversationMessage::user("second user")),
            response(
                "turn_2",
                ConversationMessage::assistant(
                    Some("starting tool".into()),
                    vec![ToolCall {
                        id: "call_pending".into(),
                        name: "shell".into(),
                        arguments: serde_json::json!({"cmd": "pwd"}),
                        thought_signature: None,
                    }],
                ),
            ),
            response("turn_3", ConversationMessage::user("third user")),
            response(
                "turn_3",
                ConversationMessage::tool("call_pending", "late tool result"),
            ),
        ];

        let forked = build_thread_fork_history(&parent_items, &TurnId::new("turn_2"))
            .expect("thread fork history should build");

        assert!(forked
            .iter()
            .any(|item| response_content(item) == Some("second user")));
        assert!(!forked.iter().any(|item| match item {
            RolloutItem::ResponseItem(response) =>
                response.message.role == MessageRole::Assistant
                    && !response.message.tool_calls.is_empty(),
            _ => false,
        }));
        assert!(!forked.iter().any(|item| match item {
            RolloutItem::ResponseItem(response) => response.message.role == MessageRole::Tool,
            _ => false,
        }));
        assert!(!forked
            .iter()
            .any(|item| response_content(item) == Some("third user")));
    }

    fn response(turn_id: &str, message: ConversationMessage) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::for_turn(TurnId::new(turn_id), message))
    }

    fn response_content(item: &RolloutItem) -> Option<&str> {
        match item {
            RolloutItem::ResponseItem(response) => Some(response.message.content.as_str()),
            _ => None,
        }
    }

    fn response_turn_ids(items: &[RolloutItem]) -> Vec<TurnId> {
        items
            .iter()
            .filter_map(|item| match item {
                RolloutItem::ResponseItem(response) => Some(response.turn_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn parent_turn_context(turn_id: &str) -> TurnContextItem {
        TurnContextItem {
            turn_id: TurnId::new(turn_id),
            workspace_root: std::path::PathBuf::from("/tmp/exagent"),
            cwd: std::path::PathBuf::from("/tmp/exagent"),
            model: ModelRef::new("openai", "mock"),
            policy_mode: PolicyMode::Off,
            permission_profile: PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            turn_mode: TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-05".to_string()),
        }
    }

    fn parent_meta() -> ThreadMeta {
        ThreadMeta {
            thread_id: ThreadId::new("thread_parent"),
            workspace_root: std::path::PathBuf::from("/tmp/exagent"),
            initial_cwd: std::path::PathBuf::from("/tmp/exagent"),
            permission_profile: crate::config::PermissionProfile::FullAccess,
            thread_source: crate::session::ThreadSource::User,
            lineage: None,
            created_at: "2026-06-04T00:00:00Z".into(),
        }
    }
}
