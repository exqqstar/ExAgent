use crate::registry::ToolContext;
use crate::runtime::memory::MemoryToolApi;
use crate::state::memory::{
    MemoryEntryKind, MemoryEntryRecord, MemoryRecallMode, MemoryScope, MemorySearchHit,
    MemorySearchQuery, MemorySourceRef, MemoryStatus,
};
use crate::tools::ToolOutcome;
use crate::types::{ThreadId, ToolCall, ToolResult, ToolStatus};

pub(crate) const MEMORY_TOOL_ACTOR: &str = "model";

#[derive(Debug, Clone, Copy, PartialEq, Eq, schemars::JsonSchema, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryScopeArg {
    Global,
    Project,
}

impl From<MemoryScopeArg> for MemoryScope {
    fn from(scope: MemoryScopeArg) -> Self {
        match scope {
            MemoryScopeArg::Global => Self::Global,
            MemoryScopeArg::Project => Self::Project,
        }
    }
}

#[derive(Debug, Clone, Copy, schemars::JsonSchema, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemoryEntryKindArg {
    Architecture,
    Preference,
    Workflow,
    Bug,
    Fact,
}

impl From<MemoryEntryKindArg> for MemoryEntryKind {
    fn from(kind: MemoryEntryKindArg) -> Self {
        match kind {
            MemoryEntryKindArg::Architecture => Self::Architecture,
            MemoryEntryKindArg::Preference => Self::Preference,
            MemoryEntryKindArg::Workflow => Self::Workflow,
            MemoryEntryKindArg::Bug => Self::Bug,
            MemoryEntryKindArg::Fact => Self::Fact,
        }
    }
}

pub(crate) struct DerivedMemoryScope {
    pub(crate) scope: MemoryScope,
    pub(crate) project_id: Option<String>,
    pub(crate) thread_id: Option<ThreadId>,
}

pub(crate) fn memory_api(ctx: &ToolContext) -> Result<&MemoryToolApi, String> {
    ctx.memory_api
        .as_deref()
        .ok_or_else(|| "memory API unavailable".to_string())
}

pub(crate) async fn derive_scope(
    ctx: &ToolContext,
    scope: MemoryScope,
) -> Result<DerivedMemoryScope, String> {
    let api = memory_api(ctx)?;
    let project_id = api
        .runtime()
        .resolve_project_id_cached(&ctx.config.workspace_root)
        .await
        .ok()
        .flatten();

    match scope {
        MemoryScope::Global => Ok(DerivedMemoryScope {
            scope,
            project_id: None,
            thread_id: None,
        }),
        MemoryScope::Project => {
            let Some(project_id) = project_id else {
                return Err("project memory scope requires a known workspace project".to_string());
            };
            Ok(DerivedMemoryScope {
                scope,
                project_id: Some(project_id),
                thread_id: None,
            })
        }
        MemoryScope::Thread => {
            let Some(project_id) = project_id else {
                return Err("thread memory scope requires a known workspace project".to_string());
            };
            let Some(thread_id) = ctx.thread_id.clone() else {
                return Err("thread memory scope requires thread context".to_string());
            };
            Ok(DerivedMemoryScope {
                scope,
                project_id: Some(project_id),
                thread_id: Some(thread_id),
            })
        }
    }
}

pub(crate) async fn search_query(
    ctx: &ToolContext,
    query: String,
    scope: MemoryScope,
    limit: Option<usize>,
) -> Result<MemorySearchQuery, String> {
    let derived = derive_scope(ctx, scope).await?;
    Ok(MemorySearchQuery {
        scope: derived.scope,
        project_id: derived.project_id,
        thread_id: derived.thread_id,
        query,
        mode: MemoryRecallMode::ToolPull,
        limit: tool_limit(ctx, limit),
        include_entries: true,
    })
}

pub(crate) fn source_refs_for_tool_call(
    ctx: &ToolContext,
    call: &ToolCall,
) -> Vec<MemorySourceRef> {
    let Some(thread_id) = ctx.thread_id.clone() else {
        return Vec::new();
    };
    vec![MemorySourceRef {
        thread_id,
        turn_id: ctx.turn_id.clone(),
        event_id: None,
        tool_call_id: Some(call.id.clone()),
        tool_invocation_id: ctx.tool_invocation_id.clone(),
    }]
}

pub(crate) fn tool_limit(ctx: &ToolContext, limit: Option<usize>) -> usize {
    let max = ctx.config.memory_tool_max_hits.max(1);
    limit.unwrap_or(max).clamp(1, max)
}

pub(crate) fn format_hits(hits: &[MemorySearchHit], max_chars: usize) -> String {
    if hits.is_empty() {
        return "No memory hits.".to_string();
    }
    if max_chars == 0 {
        return String::new();
    }

    let mut rendered = String::new();
    for (index, hit) in hits.iter().enumerate() {
        let mut line = format!(
            "{}. [{}/{}/{} stale={} quarantined={}] {}",
            index + 1,
            hit.source.as_str(),
            hit.scope.as_str(),
            hit.kind,
            hit.stale,
            hit.quarantined,
            single_line(&hit.title)
        );
        if !hit.body.trim().is_empty() {
            line.push('\n');
            line.push_str(&single_line(hit.body.trim()));
        }
        if !hit.files.is_empty() {
            line.push_str("\nfiles: ");
            line.push_str(&hit.files.join(", "));
        }
        if !rendered.is_empty() {
            line.insert_str(0, "\n\n");
        }
        if char_count(&rendered).saturating_add(char_count(&line)) > max_chars {
            append_truncation(&mut rendered, max_chars);
            break;
        }
        rendered.push_str(&line);
    }

    if rendered.is_empty() {
        "[TRUNCATED]".to_string()
    } else {
        rendered
    }
}

pub(crate) fn format_candidates(candidates: &[MemoryEntryRecord]) -> String {
    if candidates.is_empty() {
        return "No memory candidates.".to_string();
    }

    candidates
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{}. [{} {} {} pinned={} quarantined={}] {} - {}",
                index + 1,
                entry.id,
                status_label(entry.status),
                entry.scope.as_str(),
                entry.pinned,
                entry.privacy_flags.suspicious_injection,
                entry.kind.as_str(),
                entry.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn success_json(
    tool_call_id: String,
    tool_name: String,
    content: impl Into<String>,
    meta: serde_json::Value,
) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Success,
        content: content.into(),
        meta: Some(meta),
        parts: Vec::new(),
    })
}

pub(crate) fn error(
    tool_call_id: String,
    tool_name: String,
    content: impl Into<String>,
) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content: content.into(),
        meta: None,
        parts: Vec::new(),
    })
}

fn status_label(status: MemoryStatus) -> &'static str {
    status.as_str()
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_truncation(rendered: &mut String, max_chars: usize) {
    const MARKER: &str = "[TRUNCATED]";
    let marker_chars = char_count(MARKER);
    if max_chars <= marker_chars {
        rendered.clear();
        rendered.extend(MARKER.chars().take(max_chars));
        return;
    }

    let marker = if rendered.is_empty() {
        MARKER
    } else {
        "\n[TRUNCATED]"
    };
    let marker_chars = char_count(marker);
    while char_count(rendered).saturating_add(marker_chars) > max_chars {
        rendered.pop();
    }
    rendered.push_str(marker);
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::format_hits;
    use crate::state::memory::{
        MemoryCodeRef, MemoryRankSignals, MemoryScope, MemorySearchHit, MemorySourceKind,
    };

    #[test]
    fn format_hits_budget_counts_unicode_characters_not_bytes() {
        let hits = vec![hit("中文记忆", "用中文记录项目规则。")];
        let expected = format_hits(&hits, 4096);
        let budget = expected.chars().count();

        assert_eq!(format_hits(&hits, budget), expected);
    }

    fn hit(title: &str, body: &str) -> MemorySearchHit {
        MemorySearchHit {
            source_id: "entry_unicode".into(),
            source: MemorySourceKind::Entry,
            scope: MemoryScope::Project,
            kind: "workflow".into(),
            title: title.into(),
            body: body.into(),
            files: vec!["src/runtime/context.rs".into()],
            code_refs: vec![MemoryCodeRef {
                path: "src/runtime/context.rs".into(),
                line: Some(12),
                symbol: Some("ContextManager".into()),
            }],
            concepts: vec!["memory".into()],
            source_refs: vec![],
            confidence: 0.93,
            stale: false,
            quarantined: false,
            pinned: false,
            status: None,
            supersedes_id: None,
            use_count: 0,
            thread_id: None,
            turn_id: None,
            rank: MemoryRankSignals {
                text_rank: 0.0,
                scope_boost: 0.0,
                confidence_boost: 0.0,
                strength_boost: 0.0,
                recency_boost: 0.0,
                working_set_boost: 0.0,
                stale_penalty: 0.0,
                privacy_penalty: 0.0,
                final_score: 0.0,
            },
        }
    }
}
