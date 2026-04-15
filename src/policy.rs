use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex;

use crate::session::ApprovalId;
use crate::types::{EventId, SessionId};

static APPROVAL_COUNTER: AtomicU64 = AtomicU64::new(1);
static POLICY_EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyMode {
    Off,
    Advisory,
    Enforced,
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

#[derive(Debug, Clone)]
pub struct PendingCommandApproval {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub tool_name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub timeout_secs: Option<u64>,
    pub persistent: bool,
    pub reason: String,
}

#[derive(Clone, Default)]
pub struct PolicyManager {
    pending: Arc<Mutex<HashMap<String, PendingCommandApproval>>>,
}

impl PolicyManager {
    pub fn classify_command(&self, mode: PolicyMode, command: &str) -> (PolicyDecision, Option<String>) {
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
        session_id: SessionId,
        tool_name: &str,
        command: &str,
        cwd: PathBuf,
        timeout_secs: Option<u64>,
        persistent: bool,
        reason: String,
    ) -> PendingCommandApproval {
        let approval = PendingCommandApproval {
            approval_id: new_approval_id(),
            session_id,
            tool_name: tool_name.to_string(),
            command: command.to_string(),
            cwd,
            timeout_secs,
            persistent,
            reason,
        };

        self.pending
            .lock()
            .await
            .insert(approval.approval_id.as_str().to_string(), approval.clone());
        approval
    }

    pub async fn take_pending_command(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<PendingCommandApproval, String> {
        self.pending
            .lock()
            .await
            .remove(approval_id.as_str())
            .ok_or_else(|| format!("unknown approval id: {}", approval_id.as_str()))
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

fn hard_deny_reason(command: &str) -> Option<&'static str> {
    let normalized = command.trim();
    if normalized.contains("rm -rf /") || normalized.contains("mkfs") {
        return Some("command matched a hard-deny policy pattern");
    }
    None
}

fn review_required_reason(command: &str) -> Option<&'static str> {
    const PATTERNS: [&str; 5] = ["rm -rf", "git reset --hard", "git checkout --", "shutdown", "reboot"];
    if PATTERNS.iter().any(|pattern| command.contains(pattern)) {
        return Some("risky command matched approval policy");
    }
    None
}
