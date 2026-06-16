use serde::{Deserialize, Serialize};

use crate::types::{EventId, ThreadId, TurnId};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Global,
    Project,
    Thread,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Thread => "thread",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceKind {
    Entry,
}

impl MemorySourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecallMode {
    AutoInject,
    ToolPull,
    DesktopInspect,
}

impl MemoryRecallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoInject => "auto_inject",
            Self::ToolPull => "tool_pull",
            Self::DesktopInspect => "desktop_inspect",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryKind {
    Architecture,
    Preference,
    Workflow,
    Bug,
    Fact,
}

impl MemoryEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
            Self::Preference => "preference",
            Self::Workflow => "workflow",
            Self::Bug => "bug",
            Self::Fact => "fact",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Candidate,
    Active,
    Superseded,
    Rejected,
    Archived,
    Deleted,
}

impl MemoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemoryPrivacyFlags {
    pub redacted_secret: bool,
    pub redacted_private_block: bool,
    pub sensitive_path: bool,
    pub output_truncated: bool,
    pub suspicious_injection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemoryCodeRef {
    pub path: String,
    pub line: Option<u32>,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemorySourceRef {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub event_id: Option<EventId>,
    pub tool_call_id: Option<String>,
    pub tool_invocation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntryRecord {
    pub id: String,
    pub scope: MemoryScope,
    pub project_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub kind: MemoryEntryKind,
    pub title: String,
    pub content: String,
    pub files: Vec<String>,
    pub code_refs: Vec<MemoryCodeRef>,
    pub concepts: Vec<String>,
    pub source_refs: Vec<MemorySourceRef>,
    pub confidence: f64,
    pub strength: i64,
    pub pinned: bool,
    pub status: MemoryStatus,
    pub inactive_reason: Option<String>,
    pub supersedes_id: Option<String>,
    pub privacy_flags: MemoryPrivacyFlags,
    pub created_by: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
    pub use_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySaveInput {
    pub scope: MemoryScope,
    pub kind: MemoryEntryKind,
    pub title: String,
    pub content: String,
    pub files: Vec<String>,
    pub concepts: Vec<String>,
    pub source_refs: Vec<MemorySourceRef>,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchQuery {
    pub scope: MemoryScope,
    pub project_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub query: String,
    pub mode: MemoryRecallMode,
    pub limit: usize,
    pub include_entries: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryRankSignals {
    pub text_rank: f64,
    pub scope_boost: f64,
    pub confidence_boost: f64,
    pub strength_boost: f64,
    pub recency_boost: f64,
    pub working_set_boost: f64,
    pub stale_penalty: f64,
    pub privacy_penalty: f64,
    pub final_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySearchHit {
    pub source_id: String,
    pub source: MemorySourceKind,
    pub scope: MemoryScope,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub files: Vec<String>,
    pub code_refs: Vec<MemoryCodeRef>,
    pub concepts: Vec<String>,
    pub source_refs: Vec<MemorySourceRef>,
    pub confidence: f64,
    pub stale: bool,
    pub quarantined: bool,
    pub pinned: bool,
    pub status: Option<MemoryStatus>,
    pub supersedes_id: Option<String>,
    pub use_count: i64,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub rank: MemoryRankSignals,
}
