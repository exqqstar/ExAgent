use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::session::ApprovalId;
use crate::types::{EventId, ThreadId};

static APPROVAL_COUNTER: AtomicU64 = AtomicU64::new(1);
static PENDING_APPROVAL_ORDER_COUNTER: AtomicU64 = AtomicU64::new(1);
static POLICY_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    Off,
    Advisory,
    Enforced,
}

impl PolicyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Advisory => "advisory",
            Self::Enforced => "enforced",
        }
    }
}

impl Default for PolicyMode {
    fn default() -> Self {
        Self::Off
    }
}

impl FromStr for PolicyMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "advisory" => Ok(Self::Advisory),
            "enforced" => Ok(Self::Enforced),
            other => Err(format!("unsupported policy mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingApprovalSummary {
    pub thread_id: ThreadId,
    pub approval_id: ApprovalId,
    pub detail: PendingApprovalDetail,
    pub requested_at_ms: u64,
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingApprovalDetail {
    Command {
        tool_name: String,
        command: String,
        cwd: PathBuf,
        timeout_secs: Option<u64>,
        persistent: bool,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct PendingCommandApproval {
    pub approval_id: ApprovalId,
    pub thread_id: ThreadId,
    pub tool_name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub timeout_secs: Option<u64>,
    pub persistent: bool,
    pub reason: String,
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct QuestionPrompt {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUserInputRequest {
    pub request_id: ApprovalId,
    pub thread_id: ThreadId,
    pub tool_name: String,
    pub questions: Vec<QuestionPrompt>,
    pub answers: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone)]
struct PendingCommandApprovalRecord {
    approval: PendingCommandApproval,
    requested_at_ms: u64,
    created_order: u64,
    checkpoint_id: Option<String>,
}

impl PendingCommandApprovalRecord {
    fn new(approval: PendingCommandApproval) -> Self {
        let checkpoint_id = approval.checkpoint_id.clone();
        Self {
            approval,
            requested_at_ms: current_time_ms(),
            created_order: next_pending_approval_order(),
            checkpoint_id,
        }
    }

    fn summary(&self) -> PendingApprovalSummary {
        PendingApprovalSummary {
            thread_id: self.approval.thread_id.clone(),
            approval_id: self.approval.approval_id.clone(),
            detail: PendingApprovalDetail::Command {
                tool_name: self.approval.tool_name.clone(),
                command: self.approval.command.clone(),
                cwd: self.approval.cwd.clone(),
                timeout_secs: self.approval.timeout_secs,
                persistent: self.approval.persistent,
                reason: self.approval.reason.clone(),
            },
            requested_at_ms: self.requested_at_ms,
            checkpoint_id: self.checkpoint_id.clone(),
        }
    }
}

#[derive(Clone, Default)]
pub struct PolicyManager {
    pending: Arc<Mutex<HashMap<String, PendingCommandApprovalRecord>>>,
    pending_user_input: Arc<Mutex<HashMap<String, PendingUserInputRequest>>>,
}

impl PolicyManager {
    pub fn classify_command(
        &self,
        mode: PolicyMode,
        command: &str,
    ) -> (PolicyDecision, Option<String>) {
        if let Some(reason) = hard_deny_reason(command) {
            return (PolicyDecision::Deny, Some(reason.to_string()));
        }

        if matches!(mode, PolicyMode::Enforced) {
            if let Some(reason) = review_required_reason(command) {
                return (PolicyDecision::ReviewRequired, Some(reason.to_string()));
            }
        }

        (PolicyDecision::Allow, None)
    }

    pub async fn create_command_approval(
        &self,
        thread_id: ThreadId,
        tool_name: &str,
        command: &str,
        cwd: PathBuf,
        timeout_secs: Option<u64>,
        persistent: bool,
        reason: String,
    ) -> PendingCommandApproval {
        let approval = PendingCommandApproval {
            approval_id: new_approval_id(),
            thread_id,
            tool_name: tool_name.to_string(),
            command: command.to_string(),
            cwd,
            timeout_secs,
            persistent,
            reason,
            checkpoint_id: None,
        };

        self.pending.lock().await.insert(
            approval.approval_id.as_str().to_string(),
            PendingCommandApprovalRecord::new(approval.clone()),
        );
        approval
    }

    pub async fn restore_command_approval(&self, approval: PendingCommandApproval) {
        let key = approval.approval_id.as_str().to_string();
        let mut pending = self.pending.lock().await;
        if let Some(record) = pending.get_mut(&key) {
            let checkpoint_id = approval
                .checkpoint_id
                .clone()
                .or_else(|| record.checkpoint_id.clone());
            record.approval = approval;
            record.approval.checkpoint_id = checkpoint_id.clone();
            record.checkpoint_id = checkpoint_id;
        } else {
            pending.insert(key, PendingCommandApprovalRecord::new(approval));
        }
    }

    pub async fn attach_checkpoint_id(
        &self,
        approval_id: &ApprovalId,
        checkpoint_id: String,
    ) -> bool {
        let mut pending = self.pending.lock().await;
        let Some(record) = pending.get_mut(approval_id.as_str()) else {
            return false;
        };
        record.checkpoint_id = Some(checkpoint_id.clone());
        record.approval.checkpoint_id = Some(checkpoint_id);
        true
    }

    pub async fn list_pending(&self) -> Vec<PendingApprovalSummary> {
        let mut pending = self
            .pending
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by_key(|record| record.created_order);
        pending.into_iter().map(|record| record.summary()).collect()
    }

    pub async fn take_pending_command(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<PendingCommandApproval, String> {
        self.pending
            .lock()
            .await
            .remove(approval_id.as_str())
            .map(|record| {
                let mut approval = record.approval;
                approval.checkpoint_id = record.checkpoint_id;
                approval
            })
            .ok_or_else(|| format!("unknown approval id: {}", approval_id.as_str()))
    }

    pub async fn cancel_pending_for_thread(&self, thread_id: &ThreadId) {
        self.pending
            .lock()
            .await
            .retain(|_, record| &record.approval.thread_id != thread_id);
        self.pending_user_input
            .lock()
            .await
            .retain(|_, request| &request.thread_id != thread_id);
    }

    pub async fn pending_count_for_thread(&self, thread_id: &ThreadId) -> usize {
        let command_count = self
            .pending
            .lock()
            .await
            .values()
            .filter(|record| &record.approval.thread_id == thread_id)
            .count();
        let user_input_count = self
            .pending_user_input
            .lock()
            .await
            .values()
            .filter(|request| &request.thread_id == thread_id)
            .count();
        command_count + user_input_count
    }

    pub async fn create_user_input_request(
        &self,
        thread_id: ThreadId,
        tool_name: &str,
        questions: Vec<QuestionPrompt>,
    ) -> PendingUserInputRequest {
        let request = PendingUserInputRequest {
            request_id: new_approval_id(),
            thread_id,
            tool_name: tool_name.to_string(),
            questions,
            answers: None,
        };
        self.pending_user_input
            .lock()
            .await
            .insert(request.request_id.as_str().to_string(), request.clone());
        request
    }

    pub async fn restore_user_input_request(&self, request: PendingUserInputRequest) {
        self.pending_user_input
            .lock()
            .await
            .insert(request.request_id.as_str().to_string(), request);
    }

    pub async fn submit_user_input_answers(
        &self,
        request_id: &ApprovalId,
        answers: Vec<Vec<String>>,
    ) -> Result<(), String> {
        let mut pending = self.pending_user_input.lock().await;
        let Some(request) = pending.get_mut(request_id.as_str()) else {
            return Err(format!(
                "unknown user input request id: {}",
                request_id.as_str()
            ));
        };
        request.answers = Some(answers);
        Ok(())
    }

    pub async fn take_pending_user_input(
        &self,
        request_id: &ApprovalId,
    ) -> Result<PendingUserInputRequest, String> {
        self.pending_user_input
            .lock()
            .await
            .remove(request_id.as_str())
            .ok_or_else(|| format!("unknown user input request id: {}", request_id.as_str()))
    }

    pub async fn has_pending_user_input_for_thread(&self, thread_id: &ThreadId) -> bool {
        self.pending_user_input
            .lock()
            .await
            .values()
            .any(|request| &request.thread_id == thread_id)
    }
}

pub fn new_policy_event_id() -> EventId {
    let next = POLICY_EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    EventId::new(format!("approval_evt_{next}"))
}

fn new_approval_id() -> ApprovalId {
    let next = APPROVAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    ApprovalId::new(format!("approval_{next}"))
}

fn next_pending_approval_order() -> u64 {
    PENDING_APPROVAL_ORDER_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn hard_deny_reason(command: &str) -> Option<&'static str> {
    let normalized = command.trim();
    if normalized.contains("rm -rf /") || normalized.contains("mkfs") {
        return Some("command matched a hard-deny policy pattern");
    }
    None
}

fn review_required_reason(command: &str) -> Option<&'static str> {
    const PATTERNS: [&str; 5] = [
        "rm -rf",
        "git reset --hard",
        "git checkout --",
        "shutdown",
        "reboot",
    ];
    if PATTERNS.iter().any(|pattern| command.contains(pattern)) {
        return Some("risky command matched approval policy");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{PolicyManager, QuestionOption, QuestionPrompt};
    use crate::types::ThreadId;

    fn question(text: &str) -> QuestionPrompt {
        QuestionPrompt {
            question: text.to_string(),
            header: Some("Choice".to_string()),
            options: vec![QuestionOption {
                label: "A".to_string(),
                description: Some("Option A".to_string()),
            }],
            multi_select: false,
        }
    }

    #[tokio::test]
    async fn user_input_request_create_submit_take_round_trips_answers() {
        let policy = PolicyManager::default();
        let thread_id = ThreadId::new("thread_question_policy");
        let request = policy
            .create_user_input_request(thread_id.clone(), "ask_user", vec![question("Pick one?")])
            .await;

        policy
            .submit_user_input_answers(&request.request_id, vec![vec!["A".to_string()]])
            .await
            .unwrap();
        let taken = policy
            .take_pending_user_input(&request.request_id)
            .await
            .unwrap();

        assert_eq!(taken.thread_id, thread_id);
        assert_eq!(taken.questions, vec![question("Pick one?")]);
        assert_eq!(taken.answers, Some(vec![vec!["A".to_string()]]));
    }

    #[tokio::test]
    async fn user_input_request_take_without_submit_represents_dismissal() {
        let policy = PolicyManager::default();
        let request = policy
            .create_user_input_request(
                ThreadId::new("thread_question_dismiss"),
                "ask_user",
                vec![question("Continue?")],
            )
            .await;

        let taken = policy
            .take_pending_user_input(&request.request_id)
            .await
            .unwrap();

        assert_eq!(taken.answers, None);
    }

    #[tokio::test]
    async fn cancel_pending_for_thread_clears_user_input_requests() {
        let policy = PolicyManager::default();
        let thread_id = ThreadId::new("thread_question_cancel");
        policy
            .create_user_input_request(thread_id.clone(), "ask_user", vec![question("Cancel?")])
            .await;
        assert_eq!(policy.pending_count_for_thread(&thread_id).await, 1);

        policy.cancel_pending_for_thread(&thread_id).await;

        assert_eq!(policy.pending_count_for_thread(&thread_id).await, 0);
    }
}
