use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::{AgentConfig, ThinkingMode};
use crate::resolved::ModelRef;
use crate::runtime::agent_profile::AgentType;
use crate::session::{ThreadLineage, ThreadSnapshot};
use crate::state::fork_history::ForkTurns;
use crate::types::{ConversationMessage, ThreadId, TurnId};

const ROOT_AGENT_PATH: &str = "/root";
// Backstop only. By policy, only the root agent can spawn (worker subagents use
// basic collaboration and have no spawn_agent tool), so realized depth is 1.
// This cap is a defense-in-depth limit, not the primary control; see ADR-0041.
const MAX_SUBAGENT_DEPTH: u32 = 2;
const INTER_AGENT_COMMUNICATION_TYPE: &str = "inter_agent_communication";

#[derive(Debug, Clone)]
pub struct SpawnAgentRequest {
    pub parent_thread_id: ThreadId,
    pub config: AgentConfig,
    pub task_name: String,
    pub message: String,
    pub agent_type: AgentType,
    pub agent_role: Option<String>,
    pub fork_turns: ForkTurns,
    pub model: Option<ModelRef>,
    pub thinking_mode: Option<ThinkingMode>,
}

#[derive(Debug, Clone)]
pub struct SpawnAgentResponse {
    pub thread_id: ThreadId,
    pub parent_thread_id: ThreadId,
    pub root_thread_id: ThreadId,
    pub turn_id: TurnId,
    pub task_name: String,
    pub message_preview: String,
    pub depth: u32,
}

#[derive(Debug, Clone)]
pub struct SpawnCleanChildRequest {
    pub config: AgentConfig,
    pub message: String,
    pub lineage: ThreadLineage,
    pub fork_turns: ForkTurns,
    pub model: Option<ModelRef>,
    pub thinking_mode: Option<ThinkingMode>,
}

#[derive(Debug, Clone)]
pub struct CloseAgentRequest {
    pub parent_thread_id: ThreadId,
    pub config: AgentConfig,
    pub agent_path: String,
}

#[derive(Debug, Clone)]
pub struct CloseAgentsRequest {
    pub config: AgentConfig,
    pub parent_thread_id: ThreadId,
    pub root_thread_id: ThreadId,
    pub targets: Vec<CloseAgentTarget>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CloseAgentTarget {
    pub thread_id: ThreadId,
    pub agent_path: String,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CloseAgentResponse {
    pub parent_thread_id: ThreadId,
    pub root_thread_id: ThreadId,
    pub closed_agents: Vec<CloseAgentTarget>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InterAgentCommunication {
    pub author_thread_id: ThreadId,
    pub author_path: String,
    pub recipient_thread_id: ThreadId,
    pub recipient_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_recipients: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub trigger_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<TurnId>,
    pub created_at: String,
}

impl InterAgentCommunication {
    pub fn to_conversation_message(&self) -> ConversationMessage {
        let envelope = InterAgentCommunicationEnvelope::from(self);
        let content = serde_json::to_string(&envelope)
            .unwrap_or_else(|_| format!("{{\"type\":\"{INTER_AGENT_COMMUNICATION_TYPE}\"}}"));
        ConversationMessage::injected_system(content)
    }

    pub fn from_conversation_message(message: &ConversationMessage) -> Option<Self> {
        let envelope =
            serde_json::from_str::<InterAgentCommunicationEnvelope>(&message.content).ok()?;
        if envelope.kind != INTER_AGENT_COMMUNICATION_TYPE {
            return None;
        }
        Some(envelope.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InterAgentCommunicationEnvelope {
    #[serde(rename = "type")]
    kind: String,
    author_thread_id: ThreadId,
    author_path: String,
    recipient_thread_id: ThreadId,
    recipient_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    other_recipients: Vec<String>,
    content: String,
    #[serde(default)]
    trigger_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_turn_id: Option<TurnId>,
    created_at: String,
}

impl From<&InterAgentCommunication> for InterAgentCommunicationEnvelope {
    fn from(mail: &InterAgentCommunication) -> Self {
        Self {
            kind: INTER_AGENT_COMMUNICATION_TYPE.to_string(),
            author_thread_id: mail.author_thread_id.clone(),
            author_path: mail.author_path.clone(),
            recipient_thread_id: mail.recipient_thread_id.clone(),
            recipient_path: mail.recipient_path.clone(),
            other_recipients: mail.other_recipients.clone(),
            content: mail.content.clone(),
            trigger_turn: mail.trigger_turn,
            source_turn_id: mail.source_turn_id.clone(),
            created_at: mail.created_at.clone(),
        }
    }
}

impl From<InterAgentCommunicationEnvelope> for InterAgentCommunication {
    fn from(envelope: InterAgentCommunicationEnvelope) -> Self {
        Self {
            author_thread_id: envelope.author_thread_id,
            author_path: envelope.author_path,
            recipient_thread_id: envelope.recipient_thread_id,
            recipient_path: envelope.recipient_path,
            other_recipients: envelope.other_recipients,
            content: envelope.content,
            trigger_turn: envelope.trigger_turn,
            source_turn_id: envelope.source_turn_id,
            created_at: envelope.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub author_thread_id: ThreadId,
    pub config: AgentConfig,
    pub recipient_path: String,
    pub message: String,
    pub source_turn_id: Option<TurnId>,
    pub followup: bool,
}

#[derive(Debug, Clone)]
pub struct DeliverInterAgentMessageRequest {
    pub config: AgentConfig,
    pub control: Arc<AgentControl>,
    pub mail: InterAgentCommunication,
    pub followup: bool,
}

#[derive(Debug, Clone)]
pub struct SendMessageResponse {
    pub mail: InterAgentCommunication,
    pub followup: bool,
    pub started_turn_id: Option<TurnId>,
    pub target_busy: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Spawning,
    Running,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnTerminalStatus {
    Completed,
    Failed,
    Interrupted,
}

pub fn terminal_completion_content(
    agent_path: &str,
    turn_id: &TurnId,
    status: AgentTurnTerminalStatus,
    message: &str,
) -> String {
    serde_json::json!({
        "type": "subagent_turn_completed",
        "agent_path": agent_path,
        "turn_id": turn_id.as_str(),
        "status": status,
        "message": message,
    })
    .to_string()
}

pub fn parent_agent_path(agent_path: &str) -> Option<String> {
    let trimmed = agent_path.trim_end_matches('/');
    let (parent, child) = trimmed.rsplit_once('/')?;
    if child.is_empty() || parent.is_empty() {
        return None;
    }
    Some(parent.to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListedAgent {
    pub thread_id: Option<ThreadId>,
    pub root_thread_id: ThreadId,
    pub depth: u32,
    pub agent_path: String,
    pub status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_message: Option<String>,
}

#[async_trait]
pub trait SubagentLifecycle: Send + Sync {
    async fn spawn_clean_child(
        &self,
        request: SpawnCleanChildRequest,
        control: Arc<AgentControl>,
    ) -> Result<SpawnAgentResponse>;

    async fn close_agents(&self, request: CloseAgentsRequest) -> Result<CloseAgentResponse>;

    async fn deliver_inter_agent_message(
        &self,
        request: DeliverInterAgentMessageRequest,
    ) -> Result<SendMessageResponse>;
}

#[derive(Debug, Clone)]
struct AgentMetadata {
    thread_id: Option<ThreadId>,
    root_thread_id: ThreadId,
    depth: u32,
    agent_path: String,
    agent_type: Option<AgentType>,
    agent_role: Option<String>,
    agent_nickname: Option<String>,
    last_task_message: Option<String>,
}

#[derive(Debug)]
struct AgentRegistry {
    agents_by_path: HashMap<String, AgentMetadata>,
    paths_by_thread: HashMap<ThreadId, String>,
}

#[derive(Debug)]
pub struct AgentControl {
    root_thread_id: ThreadId,
    lifecycle: Weak<dyn SubagentLifecycle>,
    registry: Mutex<AgentRegistry>,
}

impl AgentControl {
    pub fn new_root(root_thread_id: ThreadId, lifecycle: Weak<dyn SubagentLifecycle>) -> Arc<Self> {
        let mut agents_by_path = HashMap::new();
        let mut paths_by_thread = HashMap::new();
        agents_by_path.insert(
            ROOT_AGENT_PATH.to_string(),
            AgentMetadata {
                thread_id: Some(root_thread_id.clone()),
                root_thread_id: root_thread_id.clone(),
                depth: 0,
                agent_path: ROOT_AGENT_PATH.to_string(),
                agent_type: None,
                agent_role: None,
                agent_nickname: None,
                last_task_message: None,
            },
        );
        paths_by_thread.insert(root_thread_id.clone(), ROOT_AGENT_PATH.to_string());
        Arc::new(Self {
            root_thread_id,
            lifecycle,
            registry: Mutex::new(AgentRegistry {
                agents_by_path,
                paths_by_thread,
            }),
        })
    }

    pub fn register_thread_from_snapshot(&self, snapshot: &ThreadSnapshot) {
        let metadata = metadata_from_snapshot(snapshot, &self.root_thread_id);
        let mut registry = self.registry.lock().expect("agent registry poisoned");
        registry
            .paths_by_thread
            .insert(snapshot.thread_id.clone(), metadata.agent_path.clone());
        registry
            .agents_by_path
            .entry(metadata.agent_path.clone())
            .or_insert(metadata);
    }

    pub fn list_agents(&self) -> Vec<ListedAgent> {
        let registry = self.registry.lock().expect("agent registry poisoned");
        let mut agents = registry
            .agents_by_path
            .values()
            .filter(|metadata| metadata.root_thread_id == self.root_thread_id)
            .map(|metadata| ListedAgent {
                thread_id: metadata.thread_id.clone(),
                root_thread_id: metadata.root_thread_id.clone(),
                depth: metadata.depth,
                agent_path: metadata.agent_path.clone(),
                status: if metadata.thread_id.is_some() {
                    AgentStatus::Running
                } else {
                    AgentStatus::Spawning
                },
                agent_type: metadata.agent_type,
                agent_role: metadata.agent_role.clone(),
                agent_nickname: metadata.agent_nickname.clone(),
                last_task_message: metadata.last_task_message.clone(),
            })
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| left.agent_path.cmp(&right.agent_path));
        agents
    }

    pub async fn spawn_agent(
        self: &Arc<Self>,
        request: SpawnAgentRequest,
    ) -> Result<SpawnAgentResponse> {
        let task_name = normalize_task_name(&request.task_name)?;
        let reservation = self.reserve_child_path(
            &request.parent_thread_id,
            &task_name,
            request.agent_role.clone(),
            request.agent_type,
            &request.message,
        )?;
        let lifecycle = self
            .lifecycle
            .upgrade()
            .ok_or_else(|| anyhow!("subagent lifecycle is unavailable"))?;
        let lineage = ThreadLineage {
            parent_thread_id: request.parent_thread_id.clone(),
            root_thread_id: reservation.root_thread_id.clone(),
            depth: reservation.depth,
            agent_path: reservation.agent_path.clone(),
            agent_type: Some(request.agent_type),
            agent_role: request.agent_role,
            agent_nickname: None,
            forked_from_id: match request.fork_turns {
                ForkTurns::None => None,
                ForkTurns::All | ForkTurns::Last(_) => Some(request.parent_thread_id.clone()),
            },
        };
        let result = lifecycle
            .spawn_clean_child(
                SpawnCleanChildRequest {
                    config: request.config,
                    message: request.message,
                    lineage,
                    fork_turns: request.fork_turns,
                    model: request.model,
                    thinking_mode: request.thinking_mode,
                },
                Arc::clone(self),
            )
            .await;

        match result {
            Ok(response) => {
                self.commit_reservation(&reservation.agent_path, &response.thread_id);
                Ok(response)
            }
            Err(err) => {
                self.release_reservation(&reservation.agent_path);
                Err(err)
            }
        }
    }

    pub async fn close_agent(
        self: &Arc<Self>,
        request: CloseAgentRequest,
    ) -> Result<CloseAgentResponse> {
        let target_path = normalize_agent_path(&request.agent_path)?;
        if target_path == ROOT_AGENT_PATH {
            return Err(anyhow!("root agent cannot be closed"));
        }
        let (root_thread_id, targets) =
            self.close_targets_for_path(&request.parent_thread_id, &target_path)?;
        let lifecycle = self
            .lifecycle
            .upgrade()
            .ok_or_else(|| anyhow!("subagent lifecycle is unavailable"))?;
        let response = lifecycle
            .close_agents(CloseAgentsRequest {
                config: request.config,
                parent_thread_id: request.parent_thread_id.clone(),
                root_thread_id,
                targets: targets.clone(),
            })
            .await?;
        self.release_closed_targets(&targets);
        Ok(response)
    }

    pub async fn send_message(
        self: &Arc<Self>,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse> {
        let mail = self.inter_agent_mail_from_request(&request)?;
        let lifecycle = self
            .lifecycle
            .upgrade()
            .ok_or_else(|| anyhow!("subagent lifecycle is unavailable"))?;
        lifecycle
            .deliver_inter_agent_message(DeliverInterAgentMessageRequest {
                config: request.config,
                control: Arc::clone(self),
                mail,
                followup: request.followup,
            })
            .await
    }

    fn reserve_child_path(
        &self,
        parent_thread_id: &ThreadId,
        task_name: &str,
        agent_role: Option<String>,
        agent_type: AgentType,
        message: &str,
    ) -> Result<SpawnReservation> {
        let mut registry = self.registry.lock().expect("agent registry poisoned");
        let parent_path = registry
            .paths_by_thread
            .get(parent_thread_id)
            .cloned()
            .ok_or_else(|| anyhow!("parent thread is not registered for subagent spawning"))?;
        let parent = registry
            .agents_by_path
            .get(&parent_path)
            .cloned()
            .ok_or_else(|| anyhow!("parent agent metadata is missing"))?;
        let depth = parent.depth.saturating_add(1);
        if depth > MAX_SUBAGENT_DEPTH {
            return Err(anyhow!(
                "subagent depth limit reached; max depth is {MAX_SUBAGENT_DEPTH}"
            ));
        }
        let agent_path = join_agent_path(&parent.agent_path, task_name);
        if registry.agents_by_path.contains_key(&agent_path) {
            return Err(anyhow!("agent path `{agent_path}` already exists"));
        }
        registry.agents_by_path.insert(
            agent_path.clone(),
            AgentMetadata {
                thread_id: None,
                root_thread_id: parent.root_thread_id.clone(),
                depth,
                agent_path: agent_path.clone(),
                agent_type: Some(agent_type),
                agent_role,
                agent_nickname: None,
                last_task_message: Some(message_preview(message)),
            },
        );
        Ok(SpawnReservation {
            agent_path,
            root_thread_id: parent.root_thread_id,
            depth,
        })
    }

    fn commit_reservation(&self, agent_path: &str, child_thread_id: &ThreadId) {
        let mut registry = self.registry.lock().expect("agent registry poisoned");
        if let Some(metadata) = registry.agents_by_path.get_mut(agent_path) {
            metadata.thread_id = Some(child_thread_id.clone());
        }
        registry
            .paths_by_thread
            .insert(child_thread_id.clone(), agent_path.to_string());
    }

    fn release_reservation(&self, agent_path: &str) {
        let mut registry = self.registry.lock().expect("agent registry poisoned");
        if registry
            .agents_by_path
            .get(agent_path)
            .is_some_and(|metadata| metadata.thread_id.is_none())
        {
            registry.agents_by_path.remove(agent_path);
        }
    }

    fn close_targets_for_path(
        &self,
        parent_thread_id: &ThreadId,
        target_path: &str,
    ) -> Result<(ThreadId, Vec<CloseAgentTarget>)> {
        let registry = self.registry.lock().expect("agent registry poisoned");
        let parent_path = registry
            .paths_by_thread
            .get(parent_thread_id)
            .ok_or_else(|| anyhow!("parent thread is not registered for subagent closing"))?;
        let parent = registry
            .agents_by_path
            .get(parent_path)
            .ok_or_else(|| anyhow!("parent agent metadata is missing"))?;
        let Some(target) = registry.agents_by_path.get(target_path) else {
            return Err(anyhow!("agent path `{target_path}` does not exist"));
        };
        if target.root_thread_id != parent.root_thread_id {
            return Err(anyhow!(
                "agent path `{target_path}` is outside the current root tree"
            ));
        }
        let mut targets = registry
            .agents_by_path
            .values()
            .filter(|metadata| {
                metadata.root_thread_id == parent.root_thread_id
                    && is_path_or_descendant(&metadata.agent_path, target_path)
            })
            .filter_map(|metadata| {
                Some(CloseAgentTarget {
                    thread_id: metadata.thread_id.clone()?,
                    agent_path: metadata.agent_path.clone(),
                    depth: metadata.depth,
                })
            })
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| {
            right
                .depth
                .cmp(&left.depth)
                .then_with(|| left.agent_path.cmp(&right.agent_path))
        });
        if targets.is_empty() {
            return Err(anyhow!("agent path `{target_path}` has no running thread"));
        }
        if targets
            .iter()
            .any(|target| target.thread_id == *parent_thread_id)
        {
            return Err(anyhow!(
                "agent path `{target_path}` includes the current agent and cannot be closed from this turn"
            ));
        }
        Ok((parent.root_thread_id.clone(), targets))
    }

    fn release_closed_targets(&self, targets: &[CloseAgentTarget]) {
        let mut registry = self.registry.lock().expect("agent registry poisoned");
        for target in targets {
            registry.agents_by_path.remove(&target.agent_path);
            registry.paths_by_thread.remove(&target.thread_id);
        }
    }

    fn inter_agent_mail_from_request(
        &self,
        request: &SendMessageRequest,
    ) -> Result<InterAgentCommunication> {
        if request.message.trim().is_empty() {
            return Err(anyhow!("message must not be empty"));
        }
        let recipient_path = normalize_agent_path(&request.recipient_path)?;
        let registry = self.registry.lock().expect("agent registry poisoned");
        let author_path = registry
            .paths_by_thread
            .get(&request.author_thread_id)
            .cloned()
            .ok_or_else(|| anyhow!("author thread is not registered for subagent messaging"))?;
        let author = registry
            .agents_by_path
            .get(&author_path)
            .ok_or_else(|| anyhow!("author agent metadata is missing"))?;
        let recipient = registry
            .agents_by_path
            .get(&recipient_path)
            .ok_or_else(|| anyhow!("recipient agent path `{recipient_path}` does not exist"))?;
        if recipient.root_thread_id != author.root_thread_id {
            return Err(anyhow!(
                "recipient agent path `{recipient_path}` is outside the current root tree"
            ));
        }
        let recipient_thread_id = recipient
            .thread_id
            .clone()
            .ok_or_else(|| anyhow!("recipient agent `{recipient_path}` has no running thread"))?;
        Ok(InterAgentCommunication {
            author_thread_id: request.author_thread_id.clone(),
            author_path,
            recipient_thread_id,
            recipient_path,
            other_recipients: Vec::new(),
            content: request.message.trim().to_string(),
            trigger_turn: request.followup,
            source_turn_id: request.source_turn_id.clone(),
            created_at: current_utc_timestamp(),
        })
    }
}

struct SpawnReservation {
    agent_path: String,
    root_thread_id: ThreadId,
    depth: u32,
}

fn metadata_from_snapshot(
    snapshot: &ThreadSnapshot,
    fallback_root_thread_id: &ThreadId,
) -> AgentMetadata {
    match snapshot.lineage.as_ref() {
        Some(lineage) => AgentMetadata {
            thread_id: Some(snapshot.thread_id.clone()),
            root_thread_id: lineage.root_thread_id.clone(),
            depth: lineage.depth,
            agent_path: lineage.agent_path.clone(),
            agent_type: lineage.agent_type,
            agent_role: lineage.agent_role.clone(),
            agent_nickname: lineage.agent_nickname.clone(),
            last_task_message: None,
        },
        None => AgentMetadata {
            thread_id: Some(snapshot.thread_id.clone()),
            root_thread_id: fallback_root_thread_id.clone(),
            depth: 0,
            agent_path: ROOT_AGENT_PATH.to_string(),
            agent_type: None,
            agent_role: None,
            agent_nickname: None,
            last_task_message: None,
        },
    }
}

fn normalize_task_name(task_name: &str) -> Result<String> {
    let task_name = task_name.trim();
    if task_name.is_empty() {
        return Err(anyhow!("task_name must not be empty"));
    }
    if task_name.contains('/') || task_name.contains('\\') {
        return Err(anyhow!("task_name must not contain path separators"));
    }
    Ok(task_name.to_string())
}

fn normalize_agent_path(agent_path: &str) -> Result<String> {
    let agent_path = agent_path.trim();
    if agent_path.is_empty() {
        return Err(anyhow!("agent_path must not be empty"));
    }
    if !agent_path.starts_with('/') {
        return Err(anyhow!("agent_path must be an absolute agent path"));
    }
    if agent_path.contains('\\') {
        return Err(anyhow!("agent_path must not contain backslashes"));
    }
    let normalized = agent_path.trim_end_matches('/');
    if normalized.is_empty() {
        return Err(anyhow!("agent_path must be an absolute agent path"));
    }
    Ok(normalized.to_string())
}

fn join_agent_path(parent_path: &str, task_name: &str) -> String {
    format!("{}/{}", parent_path.trim_end_matches('/'), task_name)
}

fn is_path_or_descendant(agent_path: &str, target_path: &str) -> bool {
    agent_path == target_path || agent_path.starts_with(&format!("{target_path}/"))
}

pub fn message_preview(message: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut chars = message.trim().chars();
    let mut preview = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageRole;

    #[test]
    fn task_name_rejects_path_separators() {
        assert!(normalize_task_name("research/api").is_err());
        assert!(normalize_task_name("research\\api").is_err());
    }

    #[test]
    fn preview_truncates_long_messages() {
        let message = "a".repeat(240);
        let preview = message_preview(&message);
        assert_eq!(preview.chars().count(), 203);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn inter_agent_communication_round_trips_as_structured_envelope() {
        let mail = InterAgentCommunication {
            author_thread_id: ThreadId::new("thread_author"),
            author_path: "/root/research".to_string(),
            recipient_thread_id: ThreadId::new("thread_recipient"),
            recipient_path: "/root/validator".to_string(),
            other_recipients: Vec::new(),
            content: "Please verify the patch.".to_string(),
            trigger_turn: true,
            source_turn_id: Some(TurnId::new("turn_parent_1")),
            created_at: "2026-06-04T00:00:00Z".to_string(),
        };

        let message = mail.to_conversation_message();

        assert_eq!(message.role, MessageRole::System);
        assert!(message.injected);
        let envelope: serde_json::Value =
            serde_json::from_str(&message.content).expect("structured envelope json");
        assert_eq!(envelope["type"], "inter_agent_communication");
        assert_eq!(envelope["author_path"], "/root/research");
        assert_eq!(envelope["recipient_path"], "/root/validator");
        assert_eq!(envelope["trigger_turn"], true);
        assert_eq!(envelope["source_turn_id"], "turn_parent_1");

        let parsed = InterAgentCommunication::from_conversation_message(&message)
            .expect("parse structured envelope");
        assert_eq!(parsed, mail);
    }

    #[test]
    fn registry_rejects_duplicate_child_path() {
        let lifecycle = Arc::new(NoopLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_root"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let first = control
            .reserve_child_path(
                &ThreadId::new("thread_root"),
                "research",
                None,
                AgentType::Worker,
                "research task",
            )
            .expect("first reservation");
        let duplicate = control.reserve_child_path(
            &ThreadId::new("thread_root"),
            "research",
            None,
            AgentType::Worker,
            "research task",
        );
        assert!(duplicate.is_err());
        control.release_reservation(&first.agent_path);
    }

    #[test]
    fn registry_lists_agents_scoped_to_each_root_tree() {
        let lifecycle = Arc::new(NoopLifecycle);
        let lifecycle: Arc<dyn SubagentLifecycle> = lifecycle;
        let root_a =
            AgentControl::new_root(ThreadId::new("thread_root_a"), Arc::downgrade(&lifecycle));
        let root_b =
            AgentControl::new_root(ThreadId::new("thread_root_b"), Arc::downgrade(&lifecycle));

        let child_a = root_a
            .reserve_child_path(
                &ThreadId::new("thread_root_a"),
                "research",
                None,
                AgentType::Explorer,
                "root a task",
            )
            .expect("reserve child a");
        root_a.commit_reservation(&child_a.agent_path, &ThreadId::new("thread_child_a"));
        let child_b = root_b
            .reserve_child_path(
                &ThreadId::new("thread_root_b"),
                "research",
                None,
                AgentType::Worker,
                "root b task",
            )
            .expect("reserve child b");
        root_b.commit_reservation(&child_b.agent_path, &ThreadId::new("thread_child_b"));

        let listed_a = root_a.list_agents();
        assert_eq!(listed_a.len(), 2);
        assert!(listed_a.iter().any(|agent| {
            agent.agent_path == "/root/research"
                && agent.thread_id.as_ref() == Some(&ThreadId::new("thread_child_a"))
                && agent.status == AgentStatus::Running
                && agent.agent_type == Some(AgentType::Explorer)
                && agent.last_task_message.as_deref() == Some("root a task")
        }));
        assert!(listed_a
            .iter()
            .any(|agent| agent.agent_path == ROOT_AGENT_PATH && agent.agent_type.is_none()));
        assert!(!listed_a
            .iter()
            .any(|agent| agent.thread_id.as_ref() == Some(&ThreadId::new("thread_child_b"))));

        let listed_b = root_b.list_agents();
        assert_eq!(listed_b.len(), 2);
        assert!(listed_b.iter().any(|agent| {
            agent.agent_path == "/root/research"
                && agent.thread_id.as_ref() == Some(&ThreadId::new("thread_child_b"))
                && agent.agent_type == Some(AgentType::Worker)
                && agent.last_task_message.as_deref() == Some("root b task")
        }));
    }

    #[test]
    fn registry_release_closed_child_path_allows_reuse() {
        let lifecycle = Arc::new(NoopLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_root"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let first = control
            .reserve_child_path(
                &ThreadId::new("thread_root"),
                "research",
                None,
                AgentType::Worker,
                "first task",
            )
            .expect("first reservation");
        control.commit_reservation(&first.agent_path, &ThreadId::new("thread_child_first"));
        let (_root_thread_id, targets) = control
            .close_targets_for_path(&ThreadId::new("thread_root"), "/root/research")
            .expect("close targets");

        control.release_closed_targets(&targets);
        let second = control.reserve_child_path(
            &ThreadId::new("thread_root"),
            "research",
            None,
            AgentType::Worker,
            "second task",
        );

        assert!(second.is_ok());
    }

    #[test]
    fn registry_closes_target_subtree() {
        let lifecycle = Arc::new(NoopLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_root"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let research = control
            .reserve_child_path(
                &ThreadId::new("thread_root"),
                "research",
                None,
                AgentType::Worker,
                "research task",
            )
            .expect("research reservation");
        control.commit_reservation(&research.agent_path, &ThreadId::new("thread_research"));
        let audit = control
            .reserve_child_path(
                &ThreadId::new("thread_research"),
                "audit",
                None,
                AgentType::Worker,
                "audit task",
            )
            .expect("audit reservation");
        control.commit_reservation(&audit.agent_path, &ThreadId::new("thread_audit"));

        let (_root_thread_id, targets) = control
            .close_targets_for_path(&ThreadId::new("thread_root"), "/root/research")
            .expect("subtree targets");

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].agent_path, "/root/research/audit");
        assert_eq!(targets[1].agent_path, "/root/research");
    }

    #[test]
    fn registry_rejects_closing_callers_own_subtree() {
        let lifecycle = Arc::new(NoopLifecycle);
        let control = AgentControl::new_root(
            ThreadId::new("thread_root"),
            Arc::downgrade(&(lifecycle as Arc<dyn SubagentLifecycle>)),
        );
        let research = control
            .reserve_child_path(
                &ThreadId::new("thread_root"),
                "research",
                None,
                AgentType::Worker,
                "research task",
            )
            .expect("research reservation");
        control.commit_reservation(&research.agent_path, &ThreadId::new("thread_research"));
        let audit = control
            .reserve_child_path(
                &ThreadId::new("thread_research"),
                "audit",
                None,
                AgentType::Worker,
                "audit task",
            )
            .expect("audit reservation");
        control.commit_reservation(&audit.agent_path, &ThreadId::new("thread_audit"));

        let own_path =
            control.close_targets_for_path(&ThreadId::new("thread_research"), "/root/research");
        assert!(own_path.is_err());

        let ancestor_path =
            control.close_targets_for_path(&ThreadId::new("thread_audit"), "/root/research");
        assert!(ancestor_path.is_err());
    }

    #[test]
    fn parent_agent_path_returns_direct_parent() {
        assert_eq!(parent_agent_path("/root/planner").as_deref(), Some("/root"));
        assert_eq!(
            parent_agent_path("/root/research/audit").as_deref(),
            Some("/root/research")
        );
        assert_eq!(parent_agent_path("/root"), None);
    }

    struct NoopLifecycle;

    #[async_trait]
    impl SubagentLifecycle for NoopLifecycle {
        async fn spawn_clean_child(
            &self,
            _request: SpawnCleanChildRequest,
            _control: Arc<AgentControl>,
        ) -> Result<SpawnAgentResponse> {
            unreachable!("unit test does not call lifecycle")
        }

        async fn close_agents(&self, _request: CloseAgentsRequest) -> Result<CloseAgentResponse> {
            unreachable!("unit test does not call lifecycle")
        }

        async fn deliver_inter_agent_message(
            &self,
            _request: DeliverInterAgentMessageRequest,
        ) -> Result<SendMessageResponse> {
            unreachable!("unit test does not call lifecycle")
        }
    }
}
