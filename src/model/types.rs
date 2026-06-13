use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(ThreadId);
string_id!(TurnId);
string_id!(EventId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasoningBlock {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ReasoningSignature>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub redacted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSignature {
    OpenAiField { field: String },
    ReasoningDetails(Value),
    GeminiThoughtSignature(String),
    AnthropicSignature(String),
    AnthropicRedactedData(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
    ReviewRequired,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::ReviewRequired => "review_required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub content: String,
    pub meta: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ConversationContentPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<ReasoningBlock>,
}

impl AssistantTurn {
    pub fn into_completion(self) -> LlmCompletion {
        LlmCompletion {
            turn: self,
            token_usage: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageInfo {
    pub total_token_usage: TokenUsage,
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmCompletion {
    pub turn: AssistantTurn,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Auto,
    Low,
    High,
    Original,
}

impl Default for ImageDetail {
    fn default() -> Self {
        Self::High
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum InputModality {
    Text,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationContentPart {
    Text {
        text: String,
    },
    LocalImage {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    ImageUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

impl ConversationContentPart {
    pub fn is_image(&self) -> bool {
        matches!(self, Self::LocalImage { .. } | Self::ImageUrl { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Text {
        text: String,
    },
    LocalImage {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    ImageUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

impl UserInput {
    pub fn is_image(&self) -> bool {
        matches!(self, Self::LocalImage { .. } | Self::ImageUrl { .. })
    }

    fn into_content_part(self) -> ConversationContentPart {
        match self {
            Self::Text { text } => ConversationContentPart::Text { text },
            Self::LocalImage { path, detail } => {
                ConversationContentPart::LocalImage { path, detail }
            }
            Self::ImageUrl { url, detail } => ConversationContentPart::ImageUrl { url, detail },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ConversationContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<ReasoningBlock>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub injected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_source: Option<String>,
}

impl ConversationMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            parts: vec![],
            tool_call_id: None,
            tool_calls: vec![],
            reasoning: vec![],
            injected: false,
            internal_source: None,
        }
    }

    pub fn injected_system(content: impl Into<String>) -> Self {
        Self {
            injected: true,
            ..Self::system(content)
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            parts: vec![],
            tool_call_id: None,
            tool_calls: vec![],
            reasoning: vec![],
            injected: false,
            internal_source: None,
        }
    }

    pub fn injected_user_context(source: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            injected: true,
            internal_source: Some(source.into()),
            ..Self::user(content)
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::assistant_with_reasoning(content, vec![], tool_calls)
    }

    pub fn assistant_with_reasoning(
        content: Option<String>,
        reasoning: Vec<ReasoningBlock>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.unwrap_or_default(),
            parts: vec![],
            tool_call_id: None,
            tool_calls,
            reasoning,
            injected: false,
            internal_source: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool_with_parts(tool_call_id, content, vec![])
    }

    pub fn tool_with_parts(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        parts: Vec<ConversationContentPart>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            parts,
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: vec![],
            reasoning: vec![],
            injected: false,
            internal_source: None,
        }
    }

    pub fn user_parts(input: Vec<UserInput>) -> Self {
        let parts: Vec<ConversationContentPart> = input
            .into_iter()
            .map(UserInput::into_content_part)
            .collect();
        Self {
            role: MessageRole::User,
            content: text_preview_from_parts(&parts),
            parts,
            tool_call_id: None,
            tool_calls: vec![],
            reasoning: vec![],
            injected: false,
            internal_source: None,
        }
    }

    pub fn effective_parts(&self) -> Vec<ConversationContentPart> {
        if !self.parts.is_empty() {
            return self.parts.clone();
        }
        if self.content.is_empty() {
            return vec![];
        }
        vec![ConversationContentPart::Text {
            text: self.content.clone(),
        }]
    }

    pub fn sync_content_from_parts(&mut self) {
        self.content = text_preview_from_parts(&self.parts);
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn text_preview_from_parts(parts: &[ConversationContentPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ConversationContentPart::Text { text } => Some(text.as_str()),
            ConversationContentPart::LocalImage { .. }
            | ConversationContentPart::ImageUrl { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_add_assign_saturates_fields() {
        let mut usage = TokenUsage {
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
            reasoning_output_tokens: 1,
            total_tokens: 16,
        };
        usage.add_assign(&TokenUsage {
            input_tokens: 3,
            cached_input_tokens: 1,
            output_tokens: 7,
            reasoning_output_tokens: 2,
            total_tokens: 12,
        });

        assert_eq!(
            usage,
            TokenUsage {
                input_tokens: 13,
                cached_input_tokens: 3,
                output_tokens: 12,
                reasoning_output_tokens: 3,
                total_tokens: 28,
            }
        );
    }

    #[test]
    fn token_usage_info_new_or_append_tracks_total_and_last_usage() {
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

        let info = TokenUsageInfo::new_or_append(&None, Some(&first), Some(100_000))
            .expect("first token info");
        let info = TokenUsageInfo::new_or_append(&Some(info), Some(&second), None)
            .expect("second token info");

        assert_eq!(info.last_token_usage, second);
        assert_eq!(
            info.total_token_usage,
            TokenUsage {
                input_tokens: 30,
                cached_input_tokens: 5,
                output_tokens: 13,
                reasoning_output_tokens: 3,
                total_tokens: 46,
            }
        );
        assert_eq!(info.model_context_window, Some(100_000));
    }

    #[test]
    fn token_usage_info_can_mark_full_context_window() {
        let info = TokenUsageInfo::full_context_window(128_000);

        assert_eq!(info.total_token_usage.total_tokens, 128_000);
        assert_eq!(info.last_token_usage.total_tokens, 128_000);
        assert_eq!(info.model_context_window, Some(128_000));
    }

    #[test]
    fn assistant_turn_can_be_wrapped_as_completion_without_usage() {
        let turn = AssistantTurn {
            text: Some("hello".to_string()),
            tool_calls: vec![],
            reasoning: vec![],
        };

        let completion = turn.clone().into_completion();

        assert_eq!(completion.turn, turn);
        assert_eq!(completion.token_usage, None);
    }

    #[test]
    fn assistant_message_preserves_reasoning_metadata() {
        let message = ConversationMessage::assistant_with_reasoning(
            Some("answer".to_string()),
            vec![ReasoningBlock {
                text: "private reasoning".to_string(),
                signature: Some(ReasoningSignature::OpenAiField {
                    field: "reasoning_content".to_string(),
                }),
                redacted: false,
            }],
            vec![],
        );

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["reasoning"][0]["text"], "private reasoning");
        assert_eq!(
            value["reasoning"][0]["signature"]["open_ai_field"]["field"],
            "reasoning_content"
        );

        let round_trip: ConversationMessage = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip.reasoning[0].text, "private reasoning");
    }

    #[test]
    fn old_assistant_message_defaults_reasoning_to_empty() {
        let message: ConversationMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": "old answer",
            "tool_calls": []
        }))
        .unwrap();

        assert!(message.reasoning.is_empty());
    }

    #[test]
    fn injected_user_context_serializes_source() {
        let message = ConversationMessage::injected_user_context("goal_snapshot", "goal content");

        let value = serde_json::to_value(&message).unwrap();

        assert_eq!(value["role"], "user");
        assert_eq!(value["content"], "goal content");
        assert_eq!(value["injected"], true);
        assert_eq!(value["internal_source"], "goal_snapshot");
    }

    #[test]
    fn old_assistant_turn_defaults_reasoning_to_empty() {
        let turn: AssistantTurn = serde_json::from_value(serde_json::json!({
            "text": "old answer",
            "tool_calls": []
        }))
        .unwrap();

        assert!(turn.reasoning.is_empty());
    }

    #[test]
    fn old_tool_call_defaults_thought_signature_to_none() {
        let tool_call: ToolCall = serde_json::from_value(serde_json::json!({
            "id": "call_1",
            "name": "run_command",
            "arguments": {"cmd": "true"}
        }))
        .unwrap();

        assert_eq!(tool_call.thought_signature, None);
    }

    #[test]
    fn legacy_tool_result_deserializes_without_parts() {
        let result: ToolResult = serde_json::from_value(serde_json::json!({
            "tool_call_id": "call_1",
            "tool_name": "read_file",
            "status": "success",
            "content": "body",
            "meta": null
        }))
        .unwrap();

        assert!(result.parts.is_empty());
    }

    #[test]
    fn tool_result_parts_round_trip_and_empty_parts_serialize_compatibly() {
        let empty = ToolResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            status: ToolStatus::Success,
            content: "body".to_string(),
            meta: None,
            parts: Vec::new(),
        };

        let empty_value = serde_json::to_value(&empty).unwrap();
        assert!(empty_value.get("parts").is_none());

        let with_image = ToolResult {
            parts: vec![ConversationContentPart::LocalImage {
                path: std::path::PathBuf::from("/tmp/screen.png"),
                detail: Some(ImageDetail::High),
            }],
            ..empty
        };
        let value = serde_json::to_value(&with_image).unwrap();
        let round_trip: ToolResult = serde_json::from_value(value).unwrap();

        assert_eq!(round_trip.parts, with_image.parts);
    }

    #[test]
    fn tool_message_constructor_preserves_tool_result_parts() {
        let message = ConversationMessage::tool_with_parts(
            "call_1",
            "Viewed image",
            vec![ConversationContentPart::LocalImage {
                path: std::path::PathBuf::from("/tmp/screen.png"),
                detail: Some(ImageDetail::High),
            }],
        );

        assert_eq!(message.role, MessageRole::Tool);
        assert_eq!(message.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(message.content, "Viewed image");
        assert_eq!(message.parts.len(), 1);
    }

    #[test]
    fn legacy_conversation_message_deserializes_without_parts() {
        let value = serde_json::json!({
            "role": "user",
            "content": "legacy prompt"
        });

        let message: ConversationMessage = serde_json::from_value(value).unwrap();

        assert_eq!(message.role, MessageRole::User);
        assert_eq!(message.content, "legacy prompt");
        assert!(message.parts.is_empty());
        assert_eq!(
            message.effective_parts(),
            vec![ConversationContentPart::Text {
                text: "legacy prompt".to_string()
            }]
        );
    }

    #[test]
    fn user_input_parts_preserve_text_preview_and_images() {
        let message = ConversationMessage::user_parts(vec![
            UserInput::Text {
                text: "describe this".to_string(),
            },
            UserInput::LocalImage {
                path: std::path::PathBuf::from("/tmp/screen.png"),
                detail: Some(ImageDetail::High),
            },
        ]);

        assert_eq!(message.role, MessageRole::User);
        assert_eq!(message.content, "describe this");
        assert_eq!(
            message.parts,
            vec![
                ConversationContentPart::Text {
                    text: "describe this".to_string()
                },
                ConversationContentPart::LocalImage {
                    path: std::path::PathBuf::from("/tmp/screen.png"),
                    detail: Some(ImageDetail::High),
                },
            ]
        );
    }

    #[test]
    fn reasoning_signature_round_trips_provider_wire_shapes() {
        let cases = vec![
            (
                ReasoningSignature::OpenAiField {
                    field: "reasoning_content".to_string(),
                },
                serde_json::json!({
                    "open_ai_field": {
                        "field": "reasoning_content"
                    }
                }),
            ),
            (
                ReasoningSignature::ReasoningDetails(serde_json::json!({
                    "id": "rs_1",
                    "summary": [{"text": "detail"}]
                })),
                serde_json::json!({
                    "reasoning_details": {
                        "id": "rs_1",
                        "summary": [{"text": "detail"}]
                    }
                }),
            ),
            (
                ReasoningSignature::GeminiThoughtSignature("gemini-sig".to_string()),
                serde_json::json!({"gemini_thought_signature": "gemini-sig"}),
            ),
            (
                ReasoningSignature::AnthropicSignature("anthropic-sig".to_string()),
                serde_json::json!({"anthropic_signature": "anthropic-sig"}),
            ),
            (
                ReasoningSignature::AnthropicRedactedData("redacted-data".to_string()),
                serde_json::json!({"anthropic_redacted_data": "redacted-data"}),
            ),
        ];

        for (signature, expected_json) in cases {
            let value = serde_json::to_value(&signature).unwrap();
            assert_eq!(value, expected_json);
            let round_trip: ReasoningSignature = serde_json::from_value(value).unwrap();
            assert_eq!(round_trip, signature);
        }
    }
}
