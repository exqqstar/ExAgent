use std::path::{Path, PathBuf};

use crate::config::AgentConfig;
use crate::session::{ThreadSnapshot, TurnContextItem};
use crate::types::{ConversationMessage, MessageRole, TokenUsage, TokenUsageInfo};

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextManager {
    items: Vec<ConversationMessage>,
    history_version: u64,
    reference_turn_context: Option<TurnContextItem>,
    token_info: Option<TokenUsageInfo>,
}

#[derive(Debug, Clone)]
pub(crate) struct PromptContext {
    pub(crate) turn_context: TurnContextItem,
}

#[derive(Debug, Clone)]
pub(crate) struct TurnPaths {
    pub(crate) workspace_root: PathBuf,
    pub(crate) cwd: PathBuf,
}

impl PromptContext {
    pub(crate) fn for_turn(config: &AgentConfig, paths: TurnPaths) -> Self {
        Self {
            turn_context: TurnContextItem {
                workspace_root: paths.workspace_root,
                cwd: paths.cwd,
                model: config.model.clone(),
                policy_mode: config.policy_mode,
                command_timeout_secs: config.command_timeout_secs,
                max_output_bytes: config.max_output_bytes,
                thinking_mode: config.thinking_mode,
                current_utc_date: Some(current_utc_date()),
            },
        }
    }
}

impl ContextManager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_rollout_items(items: &[crate::state::rollout::RolloutItem]) -> Self {
        let mut manager = ContextManager::new();
        for item in items {
            match item {
                crate::state::rollout::RolloutItem::ResponseItem(message) => {
                    manager.record_items([message.clone()]);
                }
                crate::state::rollout::RolloutItem::TurnContext(context) => {
                    manager.set_reference_turn_context(Some(context.clone()));
                }
                crate::state::rollout::RolloutItem::Compacted(compacted) => {
                    if let Some(replacement_history) = &compacted.replacement_history {
                        manager.replace_history(replacement_history.clone(), None);
                    }
                }
                crate::state::rollout::RolloutItem::EventMsg(event) => {
                    if let crate::events::RuntimeEventKind::TokenCount { info } = &event.kind {
                        manager.set_token_info(info.clone());
                    }
                }
                crate::state::rollout::RolloutItem::ThreadMeta(_) => {}
            }
        }
        manager
    }

    pub(crate) fn sync_snapshot(&self, snapshot: &mut ThreadSnapshot) {
        snapshot.conversation = self.items.clone();
        snapshot.reference_turn_context = self.reference_turn_context.clone();
    }

    #[cfg(test)]
    pub(crate) fn raw_items(&self) -> &[ConversationMessage] {
        &self.items
    }

    pub(crate) fn record_items<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = ConversationMessage>,
    {
        let previous_len = self.items.len();
        self.items.extend(items);
        if self.items.len() != previous_len {
            self.history_version = self.history_version.saturating_add(1);
        }
    }

    pub(crate) fn replace_history(
        &mut self,
        items: Vec<ConversationMessage>,
        reference_turn_context: Option<TurnContextItem>,
    ) {
        self.items = items;
        self.reference_turn_context = reference_turn_context;
        self.token_info = None;
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

        if !messages.is_empty() {
            self.record_items(messages.clone());
        }
        self.reference_turn_context = Some(context.turn_context);
        messages
    }

    pub(crate) fn set_reference_turn_context(&mut self, context: Option<TurnContextItem>) {
        self.reference_turn_context = context;
    }

    #[cfg(test)]
    pub(crate) fn reference_turn_context(&self) -> Option<TurnContextItem> {
        self.reference_turn_context.clone()
    }

    pub(crate) fn for_prompt(&self) -> Vec<ConversationMessage> {
        self.items.clone()
    }

    pub(crate) fn token_info(&self) -> Option<TokenUsageInfo> {
        self.token_info.clone()
    }

    pub(crate) fn set_token_info(&mut self, info: Option<TokenUsageInfo>) {
        self.token_info = info;
    }

    pub(crate) fn update_token_info_from_usage(
        &mut self,
        usage: &TokenUsage,
        model_context_window: Option<i64>,
    ) {
        self.token_info =
            TokenUsageInfo::new_or_append(&self.token_info, Some(usage), model_context_window);
    }

    pub(crate) fn set_token_usage_full(&mut self, context_window: i64) {
        self.token_info = Some(TokenUsageInfo::full_context_window(context_window));
    }

    pub(crate) fn estimate_token_count(&self) -> i64 {
        self.items
            .iter()
            .map(estimate_message_tokens)
            .fold(0i64, i64::saturating_add)
    }

    pub(crate) fn active_context_tokens(&self) -> i64 {
        let Some(info) = &self.token_info else {
            return self.estimate_token_count();
        };

        let local_added = self
            .items_after_last_assistant_message()
            .iter()
            .map(estimate_message_tokens)
            .fold(0i64, i64::saturating_add);

        info.last_token_usage
            .total_tokens
            .saturating_add(local_added)
    }

    fn items_after_last_assistant_message(&self) -> &[ConversationMessage] {
        let start = self
            .items
            .iter()
            .rposition(|item| item.role == MessageRole::Assistant)
            .map_or(self.items.len(), |index| index.saturating_add(1));
        &self.items[start..]
    }
}

fn estimate_message_tokens(message: &ConversationMessage) -> i64 {
    let mut bytes = string_bytes(role_name(&message.role));
    bytes = bytes.saturating_add(string_bytes(&message.content));
    if let Some(tool_call_id) = &message.tool_call_id {
        bytes = bytes.saturating_add(string_bytes(tool_call_id));
    }
    for tool_call in &message.tool_calls {
        bytes = bytes.saturating_add(string_bytes(&tool_call.id));
        bytes = bytes.saturating_add(string_bytes(&tool_call.name));
        bytes = bytes.saturating_add(estimate_json_value_bytes(&tool_call.arguments));
    }
    if message.injected {
        bytes = bytes.saturating_add(string_bytes("injected"));
    }
    bytes_to_tokens(bytes)
}

fn bytes_to_tokens(bytes: i64) -> i64 {
    bytes.saturating_add(3) / 4
}

fn string_bytes(text: &str) -> i64 {
    i64::try_from(text.len()).unwrap_or(i64::MAX)
}

fn role_name(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn estimate_json_value_bytes(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Null => string_bytes("null"),
        serde_json::Value::Bool(value) => string_bytes(if *value { "true" } else { "false" }),
        serde_json::Value::Number(value) => string_bytes(&value.to_string()),
        serde_json::Value::String(value) => string_bytes(value),
        serde_json::Value::Array(values) => values
            .iter()
            .map(estimate_json_value_bytes)
            .fold(0i64, i64::saturating_add),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| string_bytes(key).saturating_add(estimate_json_value_bytes(value)))
            .fold(0i64, i64::saturating_add),
    }
}

fn build_initial_context_messages(context: &TurnContextItem) -> Vec<ConversationMessage> {
    vec![
        ConversationMessage::injected_system(format!(
            "Runtime context:\n\
             - Model: {}\n\
             - Policy mode: {}\n\
             - Command timeout: {}s\n\
             - Max command output: {} bytes\n\
             - Treat the workspace root as the project boundary for file and command operations.",
            context.model,
            context.policy_mode.as_str(),
            context.command_timeout_secs,
            context.max_output_bytes
        )),
        ConversationMessage::injected_system(format!(
            "Environment context:\n\
             - Workspace root: {}\n\
             - Current working directory: {}\n\
             - Current UTC date: {}",
            display_path(&context.workspace_root),
            display_path(&context.cwd),
            context.current_utc_date.as_deref().unwrap_or("unknown")
        )),
    ]
}

fn build_context_update_messages(
    previous: &TurnContextItem,
    current: &TurnContextItem,
) -> Vec<ConversationMessage> {
    let mut runtime_updates = Vec::new();
    push_changed(
        &mut runtime_updates,
        "Model",
        previous.model.as_str(),
        current.model.as_str(),
    );
    push_changed(
        &mut runtime_updates,
        "Policy mode",
        previous.policy_mode.as_str(),
        current.policy_mode.as_str(),
    );
    push_changed(
        &mut runtime_updates,
        "Command timeout",
        format!("{}s", previous.command_timeout_secs),
        format!("{}s", current.command_timeout_secs),
    );
    push_changed(
        &mut runtime_updates,
        "Max command output",
        format!("{} bytes", previous.max_output_bytes),
        format!("{} bytes", current.max_output_bytes),
    );

    let mut environment_updates = Vec::new();
    push_changed(
        &mut environment_updates,
        "Workspace root",
        display_path(&previous.workspace_root),
        display_path(&current.workspace_root),
    );
    push_changed(
        &mut environment_updates,
        "Current working directory",
        display_path(&previous.cwd),
        display_path(&current.cwd),
    );
    push_changed_opt(
        &mut environment_updates,
        "Current UTC date",
        previous.current_utc_date.as_deref(),
        current.current_utc_date.as_deref(),
    );

    let mut messages = Vec::new();
    if !runtime_updates.is_empty() {
        messages.push(ConversationMessage::injected_system(format!(
            "Runtime context updated:\n{}",
            runtime_updates.join("\n")
        )));
    }
    if !environment_updates.is_empty() {
        messages.push(ConversationMessage::injected_system(format!(
            "Environment context updated:\n{}",
            environment_updates.join("\n")
        )));
    }
    messages
}

fn push_changed(
    updates: &mut Vec<String>,
    label: &str,
    previous: impl AsRef<str>,
    current: impl AsRef<str>,
) {
    let previous = previous.as_ref();
    let current = current.as_ref();
    if previous != current {
        updates.push(format!("- {label}: {previous} -> {current}"));
    }
}

fn push_changed_opt(
    updates: &mut Vec<String>,
    label: &str,
    previous: Option<&str>,
    current: Option<&str>,
) {
    let previous = previous.unwrap_or("unknown");
    let current = current.unwrap_or("unknown");
    push_changed(updates, label, previous, current);
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn current_utc_date() -> String {
    let now = time::OffsetDateTime::now_utc();
    let date = now.date();
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyMode;
    use crate::types::{MessageRole, TokenUsage};

    fn test_config(workspace_root: &Path, cwd: &Path) -> AgentConfig {
        AgentConfig {
            workspace_root: workspace_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            model: "test-model".to_string(),
            policy_mode: PolicyMode::Enforced,
            command_timeout_secs: 42,
            max_output_bytes: 1024,
            ..AgentConfig::default()
        }
    }

    #[test]
    fn first_context_update_injects_full_runtime_and_environment_context() {
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

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("Runtime context:"));
        assert!(messages[0].content.contains("Policy mode: enforced"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Environment context:"));
        assert!(messages[1].content.contains("Current working directory:"));
        assert!(messages[1].content.contains("Current UTC date:"));
        assert!(!messages[1].content.contains("Timezone:"));
        assert!(manager.reference_turn_context().is_some());
        assert_eq!(manager.raw_items().len(), 2);
    }

    #[test]
    fn unchanged_context_does_not_reinject_messages() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        manager.apply_context_updates(PromptContext::for_turn(
            &config,
            TurnPaths {
                workspace_root: workspace_root.clone(),
                cwd: cwd.clone(),
            },
        ));
        let messages = manager.apply_context_updates(PromptContext::for_turn(
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
        ));

        assert!(messages.is_empty());
        assert_eq!(manager.raw_items().len(), 2);
    }

    #[test]
    fn changed_context_injects_only_diffs() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let mut config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        manager.apply_context_updates(PromptContext::for_turn(
            &config,
            TurnPaths {
                workspace_root: workspace_root.clone(),
                cwd,
            },
        ));

        config.model = "next-model".to_string();
        config.policy_mode = PolicyMode::Advisory;
        let next_cwd = workspace_root.join("other");
        let messages = manager.apply_context_updates(PromptContext::for_turn(
            &config,
            TurnPaths {
                workspace_root,
                cwd: next_cwd,
            },
        ));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("Runtime context updated:"));
        assert!(messages[0]
            .content
            .contains("Model: test-model -> next-model"));
        assert!(messages[0]
            .content
            .contains("Policy mode: enforced -> advisory"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Environment context updated:"));
        assert!(messages[1]
            .content
            .contains("Current working directory: /workspace/app -> /workspace/other"));
    }

    #[test]
    fn context_manager_owns_items_and_reference_context() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        let context = PromptContext::for_turn(
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
        );

        let injected = manager.apply_context_updates(context);
        manager.record_items([ConversationMessage::user("hello")]);

        assert_eq!(injected.len(), 2);
        assert!(manager.reference_turn_context().is_some());
        assert_eq!(manager.raw_items().len(), 3);
        assert_eq!(manager.for_prompt()[2].content, "hello");
    }

    #[test]
    fn token_estimate_counts_message_fields_without_serializing_full_message() {
        let mut manager = ContextManager::new();
        let messages = vec![
            ConversationMessage::user("hello"),
            ConversationMessage::assistant(Some("hi".to_string()), vec![]),
        ];

        manager.record_items(messages);

        assert_eq!(
            manager.estimate_token_count(),
            bytes_to_tokens(string_bytes("user").saturating_add(string_bytes("hello")))
                .saturating_add(bytes_to_tokens(
                    string_bytes("assistant").saturating_add(string_bytes("hi"))
                ))
        );
        assert!(manager.estimate_token_count() > 0);
    }

    #[test]
    fn token_info_updates_from_model_usage() {
        let mut manager = ContextManager::new();
        let first = TokenUsage {
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 16,
        };
        let second = TokenUsage {
            input_tokens: 20,
            cached_input_tokens: 3,
            output_tokens: 8,
            reasoning_output_tokens: 2,
            total_tokens: 30,
        };

        manager.update_token_info_from_usage(&first, Some(100_000));
        manager.update_token_info_from_usage(&second, None);

        let info = manager.token_info().expect("token info");
        assert_eq!(info.last_token_usage, second);
        assert_eq!(info.total_token_usage.total_tokens, 46);
        assert_eq!(info.model_context_window, Some(100_000));
    }

    #[test]
    fn active_context_tokens_adds_local_items_after_last_assistant() {
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
        let tool_message = ConversationMessage::tool("call_1", "large output");
        let tool_tokens = estimate_message_tokens(&tool_message);

        manager.record_items([tool_message]);

        assert_eq!(manager.active_context_tokens(), 100 + tool_tokens);
    }

    #[test]
    fn active_context_tokens_falls_back_to_local_estimate_without_api_usage() {
        let mut manager = ContextManager::new();
        manager.record_items([
            ConversationMessage::user("hello"),
            ConversationMessage::assistant(Some("hi".to_string()), vec![]),
        ]);

        assert_eq!(
            manager.active_context_tokens(),
            manager.estimate_token_count()
        );
    }

    #[test]
    fn token_usage_can_be_marked_full_context_window() {
        let mut manager = ContextManager::new();

        manager.set_token_usage_full(128_000);

        let info = manager.token_info().expect("token info");
        assert_eq!(info.total_token_usage.total_tokens, 128_000);
        assert_eq!(info.last_token_usage.total_tokens, 128_000);
        assert_eq!(info.model_context_window, Some(128_000));
    }

    #[test]
    fn replacing_history_clears_stale_token_info() {
        let mut manager = ContextManager::new();
        manager.update_token_info_from_usage(
            &TokenUsage {
                total_tokens: 100,
                ..TokenUsage::default()
            },
            Some(1_000),
        );

        manager.replace_history(vec![ConversationMessage::user("summary")], None);

        assert_eq!(manager.token_info(), None);
        assert_eq!(
            manager.active_context_tokens(),
            manager.estimate_token_count()
        );
    }

    #[test]
    fn from_rollout_items_restores_latest_token_count_info() {
        let info = TokenUsageInfo {
            total_token_usage: TokenUsage {
                total_tokens: 120,
                ..TokenUsage::default()
            },
            last_token_usage: TokenUsage {
                total_tokens: 80,
                ..TokenUsage::default()
            },
            model_context_window: Some(1_000),
        };

        let manager = ContextManager::from_rollout_items(&[
            crate::state::rollout::RolloutItem::ResponseItem(ConversationMessage::user("hello")),
            crate::state::rollout::RolloutItem::EventMsg(crate::events::RuntimeEvent {
                event_id: crate::types::EventId::new("evt_1"),
                thread_id: crate::types::ThreadId::new("thread_1"),
                turn_id: Some(crate::types::TurnId::new("turn_1")),
                kind: crate::events::RuntimeEventKind::TokenCount {
                    info: Some(info.clone()),
                },
            }),
        ]);

        assert_eq!(manager.token_info(), Some(info));
    }

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

    #[test]
    fn rollout_items_hydrate_context_manager_history_and_reference_context() {
        let workspace_root = PathBuf::from("/workspace");
        let context = TurnContextItem {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root.clone(),
            model: "mock".to_string(),
            policy_mode: PolicyMode::Off,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            thinking_mode: None,
            current_utc_date: Some("2026-05-20".to_string()),
        };
        let items = vec![
            crate::state::rollout::RolloutItem::TurnContext(context.clone()),
            crate::state::rollout::RolloutItem::ResponseItem(ConversationMessage::user("hello")),
            crate::state::rollout::RolloutItem::ResponseItem(ConversationMessage::assistant(
                Some("hi".to_string()),
                vec![],
            )),
        ];

        let manager = ContextManager::from_rollout_items(&items);

        assert_eq!(manager.raw_items().len(), 2);
        assert_eq!(manager.reference_turn_context(), Some(context));
    }

    #[test]
    fn compacted_replacement_history_replaces_context_manager_items() {
        let items = vec![
            crate::state::rollout::RolloutItem::ResponseItem(ConversationMessage::user("old")),
            crate::state::rollout::RolloutItem::Compacted(crate::state::rollout::CompactedItem {
                message: "summary".to_string(),
                replacement_history: Some(vec![ConversationMessage::assistant(
                    Some("summary".to_string()),
                    vec![],
                )]),
            }),
            crate::state::rollout::RolloutItem::ResponseItem(ConversationMessage::user("new")),
        ];

        let manager = ContextManager::from_rollout_items(&items);
        let contents = manager
            .raw_items()
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>();

        assert_eq!(contents, vec!["summary", "new"]);
    }

    #[test]
    fn compacted_replacement_history_clears_reference_context() {
        let workspace_root = PathBuf::from("/workspace");
        let context = TurnContextItem {
            workspace_root: workspace_root.clone(),
            cwd: workspace_root,
            model: "mock".to_string(),
            policy_mode: PolicyMode::Off,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            thinking_mode: None,
            current_utc_date: Some("2026-05-20".to_string()),
        };
        let items = vec![
            crate::state::rollout::RolloutItem::TurnContext(context.clone()),
            crate::state::rollout::RolloutItem::Compacted(crate::state::rollout::CompactedItem {
                message: "summary".to_string(),
                replacement_history: Some(vec![ConversationMessage::assistant(
                    Some("summary".to_string()),
                    vec![],
                )]),
            }),
        ];

        let manager = ContextManager::from_rollout_items(&items);

        assert_eq!(manager.reference_turn_context(), None);
    }
}
