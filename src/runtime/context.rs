use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{AgentConfig, ThinkingMode};
use crate::model::multimodal;
use crate::runtime::agent_profile::AgentType;
use crate::runtime::subagent::InterAgentCommunication;
use crate::runtime::turn_mode::TurnMode;
use crate::session::{ThreadSnapshot, TurnContextItem};
use crate::types::{
    ConversationMessage, InputModality, MessageRole, TokenUsage, TokenUsageInfo, TurnId,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextManager {
    items: Vec<ConversationMessage>,
    stable_internal_context: BTreeMap<String, ConversationMessage>,
    ephemeral_internal_context: BTreeMap<String, ConversationMessage>,
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

#[derive(Debug, Clone)]
pub(crate) struct AgentRuntimeProfileContext {
    pub(crate) agent_type: Option<AgentType>,
    pub(crate) agent_role: Option<String>,
    pub(crate) instructions: Option<String>,
    pub(crate) response_guidance: Option<String>,
}

impl PromptContext {
    pub(crate) fn for_turn(
        turn_id: TurnId,
        config: &AgentConfig,
        paths: TurnPaths,
        agent_profile: Option<AgentRuntimeProfileContext>,
        turn_mode: TurnMode,
    ) -> Self {
        let agent_type = agent_profile
            .as_ref()
            .and_then(|profile| profile.agent_type);
        let agent_profile_instructions = agent_profile
            .as_ref()
            .and_then(|profile| profile.instructions.clone());
        let agent_response_guidance = agent_profile
            .as_ref()
            .and_then(|profile| profile.response_guidance.clone());
        let agent_role = agent_profile
            .as_ref()
            .and_then(|profile| profile.agent_role.clone());
        Self {
            turn_context: TurnContextItem {
                turn_id,
                workspace_root: paths.workspace_root,
                cwd: paths.cwd,
                model: config.model.identity.clone(),
                policy_mode: config.policy_mode,
                permission_profile: config.permission_profile,
                command_timeout_secs: config.command_timeout_secs,
                max_output_bytes: config.max_output_bytes,
                turn_mode,
                agent_type,
                agent_profile_instructions,
                agent_response_guidance,
                agent_role,
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
                crate::state::rollout::RolloutItem::ResponseItem(response_item) => {
                    manager.record_items([response_item.message.clone()]);
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
                crate::state::rollout::RolloutItem::ThreadMeta(_)
                | crate::state::rollout::RolloutItem::WorkflowRun(_) => {}
            }
        }
        manager
    }

    pub(crate) fn sync_snapshot(&self, snapshot: &mut ThreadSnapshot) {
        snapshot.conversation = self.items.clone();
        snapshot.reference_turn_context = self.reference_turn_context.clone();
        snapshot.token_info = self.token_info.clone();
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

    pub(crate) fn upsert_ephemeral_internal_context(
        &mut self,
        source: impl Into<String>,
        mut message: ConversationMessage,
    ) {
        let source = source.into();
        message.internal_source = Some(source.clone());
        self.ephemeral_internal_context.insert(source, message);
    }

    pub(crate) fn upsert_stable_internal_context(
        &mut self,
        source: impl Into<String>,
        mut message: ConversationMessage,
    ) {
        let source = source.into();
        message.internal_source = Some(source.clone());
        self.stable_internal_context.insert(source, message);
    }

    pub(crate) fn clear_stable_internal_context(&mut self, source: &str) {
        self.stable_internal_context.remove(source);
    }

    pub(crate) fn clear_ephemeral_internal_context(&mut self, source: &str) {
        self.ephemeral_internal_context.remove(source);
    }

    pub(crate) fn clear_ephemeral_internal_context_prefix(&mut self, prefix: &str) {
        self.ephemeral_internal_context
            .retain(|source, _| !source.starts_with(prefix));
    }

    pub(crate) fn record_persistent_internal_context(
        &mut self,
        source: impl Into<String>,
        content: impl Into<String>,
    ) -> ConversationMessage {
        let message = ConversationMessage::injected_user_context(source, content);
        self.record_items([message.clone()]);
        message
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

    pub(crate) fn record_inter_agent_communications<I>(
        &mut self,
        mails: I,
    ) -> Vec<ConversationMessage>
    where
        I: IntoIterator<Item = InterAgentCommunication>,
    {
        let messages = mails
            .into_iter()
            .map(|mail| mail.to_conversation_message())
            .collect::<Vec<_>>();
        self.record_items(messages.clone());
        messages
    }

    pub(crate) fn set_reference_turn_context(&mut self, context: Option<TurnContextItem>) {
        self.reference_turn_context = context;
    }

    #[cfg(test)]
    pub(crate) fn reference_turn_context(&self) -> Option<TurnContextItem> {
        self.reference_turn_context.clone()
    }

    pub(crate) fn for_prompt(
        &self,
        input_modalities: &[InputModality],
    ) -> Vec<ConversationMessage> {
        let mut items = self
            .stable_internal_context
            .values()
            .cloned()
            .collect::<Vec<_>>();
        items.extend(self.items.clone());
        items.extend(self.ephemeral_internal_context.values().cloned());
        strip_images_when_unsupported(&mut items, input_modalities);
        items
    }

    pub(crate) fn for_compaction(
        &self,
        input_modalities: &[InputModality],
    ) -> Vec<ConversationMessage> {
        let mut items = self.items.clone();
        strip_images_when_unsupported(&mut items, input_modalities);
        items
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

fn strip_images_when_unsupported(
    messages: &mut [ConversationMessage],
    input_modalities: &[InputModality],
) {
    if !multimodal::supports_images(input_modalities) {
        multimodal::strip_images_from_messages(messages);
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
    let turn_mode_line = if context.turn_mode.is_default() {
        String::new()
    } else {
        format!("\n             - Turn mode: {}", context.turn_mode.as_str())
    };
    let agent_type_line = context
        .agent_type
        .map(|agent_type| format!("\n             - Agent type: {}", agent_type.as_str()))
        .unwrap_or_default();
    let role_line = context
        .agent_role
        .as_deref()
        .map(|role| format!("\n             - Agent role: {role}"))
        .unwrap_or_default();
    let profile_block = match (
        context.agent_profile_instructions.as_deref(),
        context.agent_response_guidance.as_deref(),
    ) {
        (Some(instructions), Some(response_guidance)) => format!(
            "\n\nAgent profile instructions:\n{instructions}\n\nResponse guidance:\n{response_guidance}"
        ),
        _ => String::new(),
    };
    let turn_mode_block = context
        .turn_mode
        .prompt_guidance()
        .map(|guidance| format!("\n\nTurn mode guidance:\n{guidance}"))
        .unwrap_or_default();
    let guidance_block = format!("{profile_block}{turn_mode_block}");
    vec![
        ConversationMessage::injected_system(format!(
            "Runtime context:\n\
             - Model: {}\n\
             - Thinking mode: {}\n\
             - Policy mode: {}\n\
             - Permission profile: {} ({})\n\
             - Command timeout: {}s\n\
             - Max command output: {} bytes{}{}{}{}\
             \n\
             - Treat the workspace root as the project boundary for file and command operations.",
            context.model,
            thinking_mode_label(context.thinking_mode),
            context.policy_mode.as_str(),
            context.permission_profile.as_str(),
            context.permission_profile.execution_boundary_summary(),
            context.command_timeout_secs,
            context.max_output_bytes,
            turn_mode_line,
            agent_type_line,
            role_line,
            guidance_block
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
        previous.model.display(),
        current.model.display(),
    );
    push_changed(
        &mut runtime_updates,
        "Thinking mode",
        thinking_mode_label(previous.thinking_mode),
        thinking_mode_label(current.thinking_mode),
    );
    push_changed(
        &mut runtime_updates,
        "Policy mode",
        previous.policy_mode.as_str(),
        current.policy_mode.as_str(),
    );
    push_changed(
        &mut runtime_updates,
        "Permission profile",
        previous.permission_profile.as_str(),
        current.permission_profile.as_str(),
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
    push_changed(
        &mut runtime_updates,
        "Turn mode",
        previous.turn_mode.as_str(),
        current.turn_mode.as_str(),
    );
    push_changed_opt(
        &mut runtime_updates,
        "Agent type",
        previous.agent_type.map(|agent_type| agent_type.as_str()),
        current.agent_type.map(|agent_type| agent_type.as_str()),
    );
    push_changed_opt(
        &mut runtime_updates,
        "Agent role",
        previous.agent_role.as_deref(),
        current.agent_role.as_deref(),
    );
    push_changed_opt(
        &mut runtime_updates,
        "Agent profile instructions",
        previous.agent_profile_instructions.as_deref(),
        current.agent_profile_instructions.as_deref(),
    );
    push_changed_opt(
        &mut runtime_updates,
        "Agent response guidance",
        previous.agent_response_guidance.as_deref(),
        current.agent_response_guidance.as_deref(),
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

fn thinking_mode_label(mode: Option<ThinkingMode>) -> &'static str {
    mode.map(ThinkingMode::label).unwrap_or("default")
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
    use crate::model::multimodal;
    use crate::policy::PolicyMode;
    use crate::resolved::ModelRef;
    use crate::runtime::agent_profile::{profile_for_type, AgentType};
    use crate::types::{
        ConversationContentPart, ImageDetail, InputModality, MessageRole, ThreadId, TokenUsage,
        UserInput,
    };

    fn test_config(workspace_root: &Path, cwd: &Path) -> AgentConfig {
        let mut model = AgentConfig::default().model;
        model.identity = ModelRef::new("openai", "test-model");
        AgentConfig {
            workspace_root: workspace_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            model,
            policy_mode: PolicyMode::Enforced,
            command_timeout_secs: 42,
            max_output_bytes: 1024,
            ..AgentConfig::default()
        }
    }

    fn all_input_modalities() -> &'static [InputModality] {
        &[InputModality::Text, InputModality::Image]
    }

    #[test]
    fn first_context_update_injects_full_runtime_and_environment_context() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        let messages = manager.apply_context_updates(PromptContext::for_turn(
            TurnId::new("turn_1"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            None,
            TurnMode::Default,
        ));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("Runtime context:"));
        assert!(messages[0].content.contains("Policy mode: enforced"));
        assert!(messages[0]
            .content
            .contains("Permission profile: full_access"));
        assert!(messages[0]
            .content
            .contains("filesystem sandbox: none; network sandbox: none; env isolation: none"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Environment context:"));
        assert!(messages[1].content.contains("Current working directory:"));
        assert!(messages[1].content.contains("Current UTC date:"));
        assert!(!messages[1].content.contains("Timezone:"));
        assert!(manager.reference_turn_context().is_some());
        assert_eq!(manager.raw_items().len(), 2);
    }

    #[test]
    fn initial_context_includes_agent_profile_guidance() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let profile = profile_for_type(Some(AgentType::Explorer));
        let prompt_context = PromptContext::for_turn(
            TurnId::new("turn_profile"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            Some(AgentRuntimeProfileContext {
                agent_type: Some(AgentType::Explorer),
                agent_role: Some("auth explorer".into()),
                instructions: Some(profile.instructions),
                response_guidance: Some(profile.response_guidance),
            }),
            TurnMode::Default,
        );

        let messages = build_initial_context_messages(&prompt_context.turn_context);
        let runtime_context = &messages[0].content;

        assert!(runtime_context.contains("Agent type: explorer"));
        assert!(runtime_context.contains("Agent role: auth explorer"));
        assert!(runtime_context.contains("Agent profile instructions:"));
        assert!(runtime_context.contains("You are an explorer agent."));
        assert!(runtime_context.contains("Response guidance:"));
        assert!(runtime_context.contains("Return relevant paths"));
    }

    #[test]
    fn initial_context_includes_plan_mode_guidance() {
        let profile = profile_for_type(Some(AgentType::Planner));
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_plan"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "gpt-5"),
            policy_mode: PolicyMode::Enforced,
            permission_profile: crate::config::PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 4096,
            turn_mode: TurnMode::Plan,
            agent_type: Some(AgentType::Planner),
            agent_profile_instructions: Some(profile.instructions),
            agent_response_guidance: Some(profile.response_guidance),
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-05".into()),
        };

        let messages = build_initial_context_messages(&context);
        let system_text = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(system_text.contains("Turn mode: plan"));
        assert!(system_text.contains("Agent type: planner"));
        assert!(system_text.contains("Agent profile instructions:"));
        assert!(system_text.contains("You are a planner agent."));
        assert!(system_text.contains("Do not edit"));
        assert!(system_text.contains("Turn mode guidance:"));
        assert!(system_text.contains("## Plan mode"));
        assert!(!system_text.contains("worker agents for scoped execution"));
    }

    #[test]
    fn initial_context_includes_thinking_mode() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let mut config = test_config(&workspace_root, &cwd);
        config.thinking_mode = Some(crate::config::ThinkingMode::Low);

        let prompt_context = PromptContext::for_turn(
            TurnId::new("turn_thinking"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            None,
            TurnMode::Default,
        );
        let messages = build_initial_context_messages(&prompt_context.turn_context);

        assert!(messages[0].content.contains("Thinking mode: low"));
    }

    #[test]
    fn unchanged_context_does_not_reinject_messages() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        manager.apply_context_updates(PromptContext::for_turn(
            TurnId::new("turn_1"),
            &config,
            TurnPaths {
                workspace_root: workspace_root.clone(),
                cwd: cwd.clone(),
            },
            None,
            TurnMode::Default,
        ));
        let messages = manager.apply_context_updates(PromptContext::for_turn(
            TurnId::new("turn_2"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            None,
            TurnMode::Default,
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
            TurnId::new("turn_1"),
            &config,
            TurnPaths {
                workspace_root: workspace_root.clone(),
                cwd,
            },
            None,
            TurnMode::Default,
        ));

        config.model.identity = ModelRef::new("openai", "next-model");
        config.policy_mode = PolicyMode::Advisory;
        let next_cwd = workspace_root.join("other");
        let messages = manager.apply_context_updates(PromptContext::for_turn(
            TurnId::new("turn_2"),
            &config,
            TurnPaths {
                workspace_root,
                cwd: next_cwd,
            },
            None,
            TurnMode::Default,
        ));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("Runtime context updated:"));
        assert!(messages[0]
            .content
            .contains("Model: openai:test-model -> openai:next-model"));
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
    fn context_update_reports_permission_profile_changes() {
        let previous = TurnContextItem {
            turn_id: TurnId::new("turn_permission"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "model-a"),
            policy_mode: crate::policy::PolicyMode::Off,
            permission_profile: crate::config::PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 8192,
            turn_mode: crate::runtime::turn_mode::TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-02".into()),
        };
        let current = TurnContextItem {
            permission_profile: crate::config::PermissionProfile::External,
            ..previous.clone()
        };

        let messages = build_context_update_messages(&previous, &current);

        assert!(messages.iter().any(|message| message
            .content
            .contains("Permission profile: full_access -> external")));
    }

    #[test]
    fn context_update_reports_agent_role_changes() {
        let previous = TurnContextItem {
            turn_id: TurnId::new("turn_agent_role"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "model-a"),
            policy_mode: crate::policy::PolicyMode::Off,
            permission_profile: crate::config::PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 8192,
            turn_mode: crate::runtime::turn_mode::TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-02".into()),
        };
        let current = TurnContextItem {
            agent_role: Some("code reviewer".into()),
            ..previous.clone()
        };

        let messages = build_context_update_messages(&previous, &current);

        assert!(messages.iter().any(|message| message
            .content
            .contains("Agent role: unknown -> code reviewer")));
    }

    #[test]
    fn context_update_reports_thinking_mode_changes() {
        let previous = TurnContextItem {
            turn_id: TurnId::new("turn_thinking_diff"),
            workspace_root: PathBuf::from("/workspace"),
            cwd: PathBuf::from("/workspace"),
            model: ModelRef::new("openai", "model-a"),
            policy_mode: crate::policy::PolicyMode::Off,
            permission_profile: crate::config::PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 8192,
            turn_mode: crate::runtime::turn_mode::TurnMode::Default,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: Some(crate::config::ThinkingMode::High),
            current_utc_date: Some("2026-06-02".into()),
        };
        let current = TurnContextItem {
            thinking_mode: Some(crate::config::ThinkingMode::Low),
            ..previous.clone()
        };

        let messages = build_context_update_messages(&previous, &current);

        assert!(messages
            .iter()
            .any(|message| message.content.contains("Thinking mode: high -> low")));
    }

    #[test]
    fn context_manager_owns_items_and_reference_context() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("app");
        let config = test_config(&workspace_root, &cwd);
        let mut manager = ContextManager::new();

        let context = PromptContext::for_turn(
            TurnId::new("turn_context_manager"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            None,
            TurnMode::Default,
        );

        let injected = manager.apply_context_updates(context);
        manager.record_items([ConversationMessage::user("hello")]);

        assert_eq!(injected.len(), 2);
        assert!(manager.reference_turn_context().is_some());
        assert_eq!(manager.raw_items().len(), 3);
        assert_eq!(
            manager.for_prompt(all_input_modalities())[2].content,
            "hello"
        );
    }

    #[test]
    fn for_prompt_strips_images_for_text_only_model_without_mutating_history() {
        let mut manager = ContextManager::new();
        manager.record_items([ConversationMessage::user_parts(vec![
            UserInput::Text {
                text: "look".to_string(),
            },
            UserInput::ImageUrl {
                url: "data:image/png;base64,AAA".to_string(),
                detail: Some(ImageDetail::High),
            },
        ])]);

        let text_only = manager.for_prompt(&[InputModality::Text]);
        let with_image = manager.for_prompt(&[InputModality::Text, InputModality::Image]);

        assert!(!multimodal::contains_image(&text_only[0].parts));
        assert!(text_only[0].parts.iter().any(|part| {
            matches!(
                part,
                ConversationContentPart::Text { text }
                    if text.contains(multimodal::IMAGE_OMITTED_PLACEHOLDER)
            )
        }));
        assert!(multimodal::contains_image(&with_image[0].parts));
        assert!(multimodal::contains_image(&manager.raw_items()[0].parts));
    }

    #[test]
    fn for_compaction_strips_images_for_text_only_model_without_mutating_history() {
        let mut manager = ContextManager::new();
        manager.record_items([ConversationMessage::user_parts(vec![
            UserInput::LocalImage {
                path: PathBuf::from("/tmp/screen.png"),
                detail: Some(ImageDetail::High),
            },
        ])]);

        let text_only = manager.for_compaction(&[InputModality::Text]);
        let with_image = manager.for_prompt(&[InputModality::Text, InputModality::Image]);

        assert!(!multimodal::contains_image(&text_only[0].parts));
        assert!(multimodal::contains_image(&with_image[0].parts));
        assert!(multimodal::contains_image(&manager.raw_items()[0].parts));
    }

    #[test]
    fn ephemeral_internal_context_is_prompt_only() {
        let mut manager = ContextManager::new();
        manager.record_items([ConversationMessage::user("visible")]);
        manager.upsert_ephemeral_internal_context(
            "goal_snapshot",
            ConversationMessage::injected_user_context("goal_snapshot", "internal"),
        );

        let prompt = manager.for_prompt(all_input_modalities());
        assert_eq!(prompt.len(), 2);
        assert_eq!(prompt[0].content, "visible");
        assert_eq!(prompt[1].content, "internal");
        assert_eq!(prompt[1].internal_source.as_deref(), Some("goal_snapshot"));

        assert_eq!(manager.raw_items().len(), 1);
        assert_eq!(manager.for_compaction(all_input_modalities()).len(), 1);
        assert_eq!(
            manager.for_compaction(all_input_modalities())[0].content,
            "visible"
        );

        let mut snapshot = crate::session::ThreadSnapshot::new_thread(
            ThreadId::new("thread_context"),
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace"),
        );
        manager.sync_snapshot(&mut snapshot);
        assert_eq!(snapshot.conversation.len(), 1);
        assert_eq!(snapshot.conversation[0].content, "visible");

        manager.clear_ephemeral_internal_context("goal_snapshot");
        assert_eq!(manager.for_prompt(all_input_modalities()).len(), 1);
    }

    #[test]
    fn stable_internal_context_is_prompt_only_and_precedes_history() {
        let mut manager = ContextManager::new();
        manager.record_items([ConversationMessage::user("visible")]);
        manager.upsert_stable_internal_context(
            "00_frozen_memory",
            ConversationMessage::injected_user_context("00_frozen_memory", "stable"),
        );
        manager.upsert_ephemeral_internal_context(
            "goal_snapshot",
            ConversationMessage::injected_user_context("goal_snapshot", "ephemeral"),
        );

        let prompt = manager.for_prompt(all_input_modalities());
        assert_eq!(prompt.len(), 3);
        assert_eq!(prompt[0].content, "stable");
        assert_eq!(
            prompt[0].internal_source.as_deref(),
            Some("00_frozen_memory")
        );
        assert_eq!(prompt[1].content, "visible");
        assert_eq!(prompt[2].content, "ephemeral");

        assert_eq!(manager.raw_items().len(), 1);
        assert_eq!(manager.for_compaction(all_input_modalities()).len(), 1);
        assert_eq!(
            manager.for_compaction(all_input_modalities())[0].content,
            "visible"
        );

        let mut snapshot = crate::session::ThreadSnapshot::new_thread(
            ThreadId::new("thread_context"),
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace"),
        );
        manager.sync_snapshot(&mut snapshot);
        assert_eq!(snapshot.conversation.len(), 1);
        assert_eq!(snapshot.conversation[0].content, "visible");

        manager.clear_stable_internal_context("00_frozen_memory");
        assert_eq!(manager.for_prompt(all_input_modalities()).len(), 2);
    }

    #[test]
    fn persistent_internal_context_is_recorded() {
        let mut manager = ContextManager::new();

        let message = manager.record_persistent_internal_context("goal_snapshot", "internal");

        assert_eq!(message.role, MessageRole::User);
        assert!(message.injected);
        assert_eq!(message.internal_source.as_deref(), Some("goal_snapshot"));
        assert_eq!(manager.raw_items(), &[message.clone()]);
        assert_eq!(manager.for_prompt(all_input_modalities()), vec![message]);
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
            crate::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("hello"),
            ),
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
            TurnId::new("turn_injected"),
            &config,
            TurnPaths {
                workspace_root,
                cwd,
            },
            None,
            TurnMode::Default,
        ));

        assert!(messages.iter().all(|message| message.injected));
        assert!(!ConversationMessage::user("hello").injected);
    }

    #[test]
    fn rollout_items_hydrate_context_manager_history_and_reference_context() {
        let workspace_root = PathBuf::from("/workspace");
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_context"),
            workspace_root: workspace_root.clone(),
            cwd: workspace_root.clone(),
            model: ModelRef::new("openai", "mock"),
            policy_mode: PolicyMode::Off,
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
        };
        let items = vec![
            crate::state::rollout::RolloutItem::TurnContext(context.clone()),
            crate::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("hello"),
            ),
            crate::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::assistant(Some("hi".to_string()), vec![]),
            ),
        ];

        let manager = ContextManager::from_rollout_items(&items);

        assert_eq!(manager.raw_items().len(), 2);
        assert_eq!(manager.reference_turn_context(), Some(context));
    }

    #[test]
    fn compacted_replacement_history_replaces_context_manager_items() {
        let items = vec![
            crate::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_1"),
                ConversationMessage::user("old"),
            ),
            crate::state::rollout::RolloutItem::Compacted(crate::state::rollout::CompactedItem {
                message: "summary".to_string(),
                replacement_history: Some(vec![ConversationMessage::assistant(
                    Some("summary".to_string()),
                    vec![],
                )]),
            }),
            crate::state::rollout::RolloutItem::response_item_for_turn(
                TurnId::new("turn_2"),
                ConversationMessage::user("new"),
            ),
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
            turn_id: TurnId::new("turn_context"),
            workspace_root: workspace_root.clone(),
            cwd: workspace_root,
            model: ModelRef::new("openai", "mock"),
            policy_mode: PolicyMode::Off,
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
