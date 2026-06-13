use std::path::PathBuf;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::config::{PermissionProfile, ThinkingMode};
use crate::policy::{PolicyMode, QuestionPrompt};
use crate::resolved::ModelRef;
use crate::runtime::agent_profile::AgentType;
use crate::runtime::turn_mode::TurnMode;
use crate::types::{ConversationMessage, EventId, ThreadId, TokenUsageInfo, TurnId};

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

string_id!(ExecSessionId);
string_id!(ApprovalId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecSessionStatus {
    Running,
    Exited,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSource {
    #[default]
    User,
    Subagent,
    Fork,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadLineage {
    pub parent_thread_id: ThreadId,
    pub root_thread_id: ThreadId,
    pub depth: u32,
    pub agent_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_id: Option<ThreadId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecSessionRef {
    pub exec_session_id: ExecSessionId,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ExecSessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionSummary {
    pub summary: String,
    pub source_event_ids: Vec<EventId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    pub requested_event_id: EventId,
    pub tool_name: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub permission_profile: PermissionProfile,
    #[serde(default = "crate::config::default_boundary_none")]
    pub filesystem_sandbox: String,
    #[serde(default = "crate::config::default_boundary_none")]
    pub network_sandbox: String,
    #[serde(default = "crate::config::default_boundary_none")]
    pub env_isolation: String,
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingUserInput {
    pub request_id: ApprovalId,
    pub requested_event_id: EventId,
    pub tool_name: String,
    pub questions: Vec<QuestionPrompt>,
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSnapshot {
    pub thread_id: ThreadId,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    #[serde(default)]
    pub permission_profile: PermissionProfile,
    #[serde(default)]
    pub thread_source: ThreadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<ThreadLineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_turn_context: Option<TurnContextItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation: Vec<ConversationMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_exec_sessions: Vec<ExecSessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<CompactionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_info: Option<TokenUsageInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approvals: Vec<PendingApproval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_user_inputs: Vec<PendingUserInput>,
}

impl ThreadSnapshot {
    pub fn new_thread(thread_id: ThreadId, workspace_root: PathBuf, cwd: PathBuf) -> Self {
        Self::new_thread_with_permission_profile(
            thread_id,
            workspace_root,
            cwd,
            PermissionProfile::FullAccess,
        )
    }

    pub fn new_thread_with_permission_profile(
        thread_id: ThreadId,
        workspace_root: PathBuf,
        cwd: PathBuf,
        permission_profile: PermissionProfile,
    ) -> Self {
        Self::new_thread_with_options(
            thread_id,
            workspace_root,
            cwd,
            permission_profile,
            ThreadSource::User,
            None,
        )
    }

    pub fn new_thread_with_options(
        thread_id: ThreadId,
        workspace_root: PathBuf,
        cwd: PathBuf,
        permission_profile: PermissionProfile,
        thread_source: ThreadSource,
        lineage: Option<ThreadLineage>,
    ) -> Self {
        Self {
            thread_id,
            workspace_root,
            cwd,
            permission_profile,
            thread_source,
            lineage,
            reference_turn_context: None,
            conversation: vec![],
            open_exec_sessions: vec![],
            latest_compaction: None,
            token_info: None,
            pending_approvals: vec![],
            pending_user_inputs: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnContextItem {
    pub turn_id: TurnId,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    #[serde(deserialize_with = "deserialize_model_ref")]
    pub model: ModelRef,
    pub policy_mode: PolicyMode,
    #[serde(default)]
    pub permission_profile: PermissionProfile,
    pub command_timeout_secs: u64,
    pub max_output_bytes: usize,
    #[serde(default, skip_serializing_if = "TurnMode::is_default")]
    pub turn_mode: TurnMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_response_guidance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<ThinkingMode>,
    #[serde(
        default,
        alias = "current_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_utc_date: Option<String>,
}

fn deserialize_model_ref<'de, D>(deserializer: D) -> Result<ModelRef, D::Error>
where
    D: de::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ModelRefWire {
        Current(ModelRef),
        LegacyString(String),
    }

    match ModelRefWire::deserialize(deserializer)? {
        ModelRefWire::Current(model_ref) => Ok(model_ref),
        ModelRefWire::LegacyString(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(de::Error::custom("turn context model cannot be empty"));
            }
            if let Some((provider_id, model_id)) = value.split_once(':') {
                let provider_id = provider_id.trim();
                let model_id = model_id.trim();
                if provider_id.is_empty() || model_id.is_empty() {
                    return Err(de::Error::custom("turn context model ref cannot be empty"));
                }
                Ok(ModelRef::new(provider_id, model_id))
            } else {
                Ok(ModelRef::new("openai", value))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent_profile::AgentType;
    use serde_json::json;

    #[test]
    fn snapshot_deserializes_without_reference_turn_context() {
        let snapshot: ThreadSnapshot = serde_json::from_value(json!({
            "thread_id": "thread_old",
            "workspace_root": "/tmp/workspace",
            "cwd": "/tmp/workspace"
        }))
        .expect("deserialize thread snapshot");

        assert_eq!(snapshot.thread_id, ThreadId::new("thread_old"));
        assert!(snapshot.reference_turn_context.is_none());
        assert!(snapshot.conversation.is_empty());
    }

    #[test]
    fn snapshot_deserializes_without_permission_profile() {
        let snapshot: ThreadSnapshot = serde_json::from_value(json!({
            "thread_id": "thread_old_profile",
            "workspace_root": "/tmp/workspace",
            "cwd": "/tmp/workspace"
        }))
        .expect("deserialize thread snapshot");

        assert_eq!(snapshot.permission_profile, PermissionProfile::FullAccess);
    }

    #[test]
    fn thread_lineage_defaults_missing_agent_type() {
        let lineage: ThreadLineage = serde_json::from_value(json!({
            "parent_thread_id": "thread_parent",
            "root_thread_id": "thread_root",
            "depth": 1,
            "agent_path": "root/child",
            "agent_role": "reviewer"
        }))
        .expect("deserialize thread lineage");

        assert_eq!(lineage.agent_type, None);
        assert_eq!(lineage.agent_role.as_deref(), Some("reviewer"));
    }

    #[test]
    fn turn_context_serializes_agent_type_when_present() {
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_agent_type"),
            workspace_root: PathBuf::from("/tmp/workspace"),
            cwd: PathBuf::from("/tmp/workspace/app"),
            model: ModelRef::new("openai", "mock"),
            policy_mode: PolicyMode::Off,
            permission_profile: PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            turn_mode: TurnMode::Default,
            agent_type: Some(AgentType::Explorer),
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-05".to_string()),
        };

        let value = serde_json::to_value(context).expect("serialize turn context");

        assert_eq!(value["agent_type"], "explorer");
    }

    #[test]
    fn turn_context_serializes_plan_mode_when_present() {
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_plan_mode"),
            workspace_root: PathBuf::from("/tmp/workspace"),
            cwd: PathBuf::from("/tmp/workspace/app"),
            model: ModelRef::new("openai", "mock"),
            policy_mode: PolicyMode::Off,
            permission_profile: PermissionProfile::FullAccess,
            command_timeout_secs: 30,
            max_output_bytes: 1024,
            turn_mode: TurnMode::Plan,
            agent_type: None,
            agent_profile_instructions: None,
            agent_response_guidance: None,
            agent_role: None,
            thinking_mode: None,
            current_utc_date: Some("2026-06-05".to_string()),
        };

        let value = serde_json::to_value(context).expect("serialize turn context");

        assert_eq!(value["turn_mode"], "plan");
    }

    #[test]
    fn turn_context_omits_default_turn_mode() {
        let context = TurnContextItem {
            turn_id: TurnId::new("turn_default_mode"),
            workspace_root: PathBuf::from("/tmp/workspace"),
            cwd: PathBuf::from("/tmp/workspace/app"),
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
        };

        let value = serde_json::to_value(context).expect("serialize turn context");

        assert!(value.get("turn_mode").is_none());
    }

    #[test]
    fn turn_context_defaults_missing_profile_guidance() {
        let context: TurnContextItem = serde_json::from_value(json!({
            "turn_id": "turn_legacy_guidance",
            "workspace_root": "/tmp/workspace",
            "cwd": "/tmp/workspace/app",
            "model": "openai:mock",
            "policy_mode": "off",
            "command_timeout_secs": 30,
            "max_output_bytes": 1024,
            "agent_role": "legacy metadata"
        }))
        .expect("deserialize turn context");

        assert_eq!(context.agent_type, None);
        assert_eq!(context.turn_mode, TurnMode::Default);
        assert_eq!(context.agent_profile_instructions, None);
        assert_eq!(context.agent_response_guidance, None);
        assert_eq!(context.agent_role.as_deref(), Some("legacy metadata"));
    }
}
