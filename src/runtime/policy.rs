use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use crate::session::ApprovalId;
use crate::types::{EventId, ThreadId};

static APPROVAL_COUNTER: AtomicU64 = AtomicU64::new(1);
static PENDING_APPROVAL_ORDER_COUNTER: AtomicU64 = AtomicU64::new(1);
static POLICY_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingApprovalSummary {
    pub thread_id: ThreadId,
    pub approval_id: ApprovalId,
    pub detail: PendingApprovalDetail,
    pub requested_at_ms: u64,
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    }

    pub async fn pending_count_for_thread(&self, thread_id: &ThreadId) -> usize {
        self.pending
            .lock()
            .await
            .values()
            .filter(|record| &record.approval.thread_id == thread_id)
            .count()
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
