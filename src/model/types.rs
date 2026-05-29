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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub injected: bool,
}

impl ConversationMessage {
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

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: vec![],
            injected: false,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.unwrap_or_default(),
            tool_call_id: None,
            tool_calls,
            injected: false,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: vec![],
            injected: false,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
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
        };

        let completion = turn.clone().into_completion();

        assert_eq!(completion.turn, turn);
        assert_eq!(completion.token_usage, None);
    }
}
