use serde_json::Value;

use super::privacy::{classify_memory_path, redact_memory_text, MemoryPathSensitivity};
use super::safety;
use super::types::{
    MemoryCodeRef, MemoryObservationKind, MemoryObservationUpsert, MemoryPrivacyFlags, MemoryScope,
};
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::state::rollout::{ResponseItem, RolloutItem};
use crate::types::{ConversationMessage, MessageRole, ThreadId, ToolResult, ToolStatus, TurnId};

const TITLE_MAX_CHARS: usize = 512;
const NARRATIVE_MAX_CHARS: usize = 12 * 1024;

pub fn project_memory_observations_from_rollout(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    items: &[RolloutItem],
    start_index: usize,
    now_ms: i64,
) -> Vec<MemoryObservationUpsert> {
    let mut observations = Vec::new();

    for item in items.iter().skip(start_index) {
        match item {
            RolloutItem::ResponseItem(response) => {
                if let Some(observation) =
                    project_user_response(project_id, thread_id, response, now_ms)
                {
                    observations.push(observation);
                }
            }
            RolloutItem::EventMsg(event) => {
                if let Some(observation) = project_event(project_id, thread_id, event, now_ms) {
                    observations.push(observation);
                }
            }
            RolloutItem::ThreadMeta(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::Compacted(_) => {}
        }
    }

    observations.sort_by(|left, right| left.id.cmp(&right.id));
    observations
}

fn project_user_response(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    response: &ResponseItem,
    now_ms: i64,
) -> Option<MemoryObservationUpsert> {
    if !is_visible_user_message(&response.message) {
        return None;
    }

    let prompt = response.message.content.trim();
    if prompt.is_empty() || !looks_like_user_rule(prompt) {
        return None;
    }

    let id = format!(
        "obs_{}_{}_userrule_{}",
        thread_id.as_str(),
        response.turn_id.as_str(),
        stable_hash8(prompt)
    );
    let title = "User rule".to_string();
    let narrative = prompt.to_string();

    Some(finalize_observation(ObservationDraft {
        id,
        project_id,
        thread_id,
        turn_id: Some(response.turn_id.clone()),
        event_id: None,
        source_tool_call_id: None,
        kind: MemoryObservationKind::UserRule,
        title,
        narrative,
        files: vec![],
        code_refs: vec![],
        concepts: vec![],
        importance: 7,
        confidence: 0.75,
        requested_auto_inject: true,
        created_at_ms: now_ms,
    }))
}

fn project_event(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    event: &RuntimeEvent,
    now_ms: i64,
) -> Option<MemoryObservationUpsert> {
    match &event.kind {
        RuntimeEventKind::ToolResult { result } => {
            project_tool_result(project_id, thread_id, event, result, now_ms)
        }
        RuntimeEventKind::RuntimeError { message } => Some(project_runtime_error(
            project_id, thread_id, event, message, now_ms,
        )),
        RuntimeEventKind::ThreadGoalReport { report } => Some(project_goal_report(
            project_id, thread_id, event, report, now_ms,
        )),
        _ => None,
    }
}

fn project_tool_result(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    event: &RuntimeEvent,
    result: &ToolResult,
    now_ms: i64,
) -> Option<MemoryObservationUpsert> {
    let event_id = event.event_id.as_str();
    let id = format!(
        "obs_{}_{}_{}",
        thread_id.as_str(),
        event_id,
        result.tool_call_id
    );
    let source_tool_call_id = Some(result.tool_call_id.clone());
    let meta = result.meta.as_ref();

    let (kind, title, narrative, files, importance, confidence) = match result.tool_name.as_str() {
        "read_file" => {
            let path = path_from_meta(meta);
            let title = path
                .as_deref()
                .map(|path| format!("Read file {}", display_path(path)))
                .unwrap_or_else(|| "Read file".to_string());
            let narrative = path
                .as_deref()
                .map(|path| format!("Tool read_file read {}.", display_path(path)))
                .unwrap_or_else(|| "Tool read_file completed.".to_string());
            (
                MemoryObservationKind::FileRead,
                title,
                narrative,
                path.into_iter().collect(),
                2,
                0.35,
            )
        }
        "write_file" => {
            let path = path_from_meta(meta);
            let title = path
                .as_deref()
                .map(|path| format!("Wrote file {}", display_path(path)))
                .unwrap_or_else(|| "Wrote file".to_string());
            let narrative = path
                .as_deref()
                .map(|path| format!("Tool write_file wrote {}.", display_path(path)))
                .unwrap_or_else(|| "Tool write_file completed.".to_string());
            (
                MemoryObservationKind::FileWrite,
                title,
                narrative,
                path.into_iter().collect(),
                5,
                0.45,
            )
        }
        "apply_patch" => {
            let files = string_array_from_meta(meta, "changed_files");
            let title = if files.is_empty() {
                "Applied patch".to_string()
            } else {
                format!("Edited files: {}", display_file_list(&files))
            };
            let narrative = if files.is_empty() {
                "Tool apply_patch edited files.".to_string()
            } else {
                format!("Tool apply_patch edited {}.", display_file_list(&files))
            };
            (
                MemoryObservationKind::FileEdit,
                title,
                narrative,
                files,
                6,
                0.45,
            )
        }
        "search_files" => {
            let query = string_from_meta(meta, "query");
            let path = string_from_meta(meta, "path");
            let label = query.clone().unwrap_or_else(|| {
                path.as_deref()
                    .map(display_path)
                    .unwrap_or_else(|| "workspace".to_string())
            });
            let title = format!("Searched files for {label}");
            let narrative = match (query.as_deref(), path.as_deref()) {
                (Some(query), Some(path)) => {
                    format!(
                        "Tool search_files searched for {query} in {}.",
                        display_path(path)
                    )
                }
                (Some(query), None) => format!("Tool search_files searched for {query}."),
                (None, Some(path)) => {
                    format!("Tool search_files searched in {}.", display_path(path))
                }
                (None, None) => "Tool search_files completed.".to_string(),
            };
            (
                MemoryObservationKind::Search,
                title,
                narrative,
                path.into_iter().collect(),
                3,
                0.35,
            )
        }
        "list_dir" => {
            let path = string_from_meta(meta, "path");
            let label = path
                .as_deref()
                .map(display_path)
                .unwrap_or_else(|| "workspace".to_string());
            let title = format!("Listed directory {label}");
            let narrative = path
                .as_deref()
                .map(|path| format!("Tool list_dir listed {}.", display_path(path)))
                .unwrap_or_else(|| "Tool list_dir completed.".to_string());
            (
                MemoryObservationKind::Search,
                title,
                narrative,
                path.into_iter().collect(),
                3,
                0.35,
            )
        }
        "exec_command" | "run_command" => {
            let command = string_from_meta(meta, "command");
            let exit_code = exit_code_from_meta(meta);
            let stderr = string_from_meta(meta, "stderr");
            let failed =
                result.status != ToolStatus::Success || exit_code.is_some_and(|code| code != 0);
            let title = if failed {
                "Command failed".to_string()
            } else {
                "Command ran".to_string()
            };
            let narrative = command_narrative(command.as_deref(), exit_code, stderr.as_deref());
            (
                MemoryObservationKind::CommandRun,
                title,
                narrative,
                vec![],
                if failed { 6 } else { 3 },
                0.4,
            )
        }
        _ => return None,
    };

    Some(finalize_observation(ObservationDraft {
        id,
        project_id,
        thread_id,
        turn_id: event.turn_id.clone(),
        event_id: Some(event.event_id.as_str().to_string()),
        source_tool_call_id,
        kind,
        title,
        narrative,
        files,
        code_refs: vec![],
        concepts: vec![],
        importance,
        confidence,
        requested_auto_inject: false,
        created_at_ms: now_ms,
    }))
}

fn project_runtime_error(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    event: &RuntimeEvent,
    message: &str,
    now_ms: i64,
) -> MemoryObservationUpsert {
    finalize_observation(ObservationDraft {
        id: format!("obs_{}_{}", thread_id.as_str(), event.event_id.as_str()),
        project_id,
        thread_id,
        turn_id: event.turn_id.clone(),
        event_id: Some(event.event_id.as_str().to_string()),
        source_tool_call_id: None,
        kind: MemoryObservationKind::RuntimeError,
        title: "Runtime error".to_string(),
        narrative: message.to_string(),
        files: vec![],
        code_refs: vec![],
        concepts: vec![],
        importance: 6,
        confidence: 0.4,
        requested_auto_inject: false,
        created_at_ms: now_ms,
    })
}

fn project_goal_report(
    project_id: Option<&str>,
    thread_id: &ThreadId,
    event: &RuntimeEvent,
    report: &crate::app_server::protocol::ThreadGoalReport,
    now_ms: i64,
) -> MemoryObservationUpsert {
    let narrative = if report.summary.trim().is_empty() {
        fallback_goal_report_narrative(report)
    } else {
        report.summary.trim().to_string()
    };

    finalize_observation(ObservationDraft {
        id: format!("obs_{}_{}", thread_id.as_str(), event.event_id.as_str()),
        project_id,
        thread_id,
        turn_id: event.turn_id.clone(),
        event_id: Some(event.event_id.as_str().to_string()),
        source_tool_call_id: None,
        kind: MemoryObservationKind::GoalReport,
        title: format!("Goal report: {}", report.objective),
        narrative,
        files: report.changed_files.clone(),
        code_refs: vec![],
        concepts: vec![],
        importance: 6,
        confidence: 0.75,
        requested_auto_inject: true,
        created_at_ms: now_ms,
    })
}

fn fallback_goal_report_narrative(
    report: &crate::app_server::protocol::ThreadGoalReport,
) -> String {
    let mut narrative = format!(
        "Goal \"{}\" finished as {:?}. {} file(s) changed.",
        report.objective,
        report.final_status,
        report.changed_files.len()
    );

    if !report.open_questions.is_empty() {
        let questions = report
            .open_questions
            .iter()
            .map(|question| question.question.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        narrative.push_str(" Open questions: ");
        narrative.push_str(&questions);
    }

    narrative
}

struct ObservationDraft<'a> {
    id: String,
    project_id: Option<&'a str>,
    thread_id: &'a ThreadId,
    turn_id: Option<TurnId>,
    event_id: Option<String>,
    source_tool_call_id: Option<String>,
    kind: MemoryObservationKind,
    title: String,
    narrative: String,
    files: Vec<String>,
    code_refs: Vec<MemoryCodeRef>,
    concepts: Vec<String>,
    importance: i64,
    confidence: f64,
    requested_auto_inject: bool,
    created_at_ms: i64,
}

fn finalize_observation(draft: ObservationDraft<'_>) -> MemoryObservationUpsert {
    let title = redact_memory_text(&draft.title, TITLE_MAX_CHARS);
    let narrative = redact_memory_text(&draft.narrative, NARRATIVE_MAX_CHARS);
    let mut privacy_flags = merge_privacy_flags(title.flags, narrative.flags);
    let (title_text, title_sensitive_path) = redact_sensitive_paths_in_text(&title.text);
    let (narrative_text, narrative_sensitive_path) =
        redact_sensitive_paths_in_text(&narrative.text);
    let (concepts, concepts_sensitive_path) = redact_sensitive_paths_in_concepts(draft.concepts);

    let (files, files_sensitive) = filter_sensitive_files(draft.files);
    let (code_refs, code_refs_sensitive) = filter_sensitive_code_refs(draft.code_refs);
    privacy_flags.sensitive_path = files_sensitive
        || code_refs_sensitive
        || title_sensitive_path
        || narrative_sensitive_path
        || concepts_sensitive_path;

    let scan = safety::scan_injection(&format!("{title_text}\n{narrative_text}"));
    privacy_flags.suspicious_injection = scan.suspicious;

    let auto_inject_eligible = draft.requested_auto_inject
        && draft.kind.auto_inject_kind_allowed()
        && draft.confidence >= 0.72
        && !privacy_flags.redacted_secret
        && !privacy_flags.sensitive_path
        && !privacy_flags.suspicious_injection;

    MemoryObservationUpsert {
        id: draft.id,
        scope: scope_for_project(draft.project_id),
        project_id: draft.project_id.map(str::to_string),
        thread_id: draft.thread_id.clone(),
        turn_id: draft.turn_id,
        event_id: draft.event_id,
        source_tool_call_id: draft.source_tool_call_id,
        kind: draft.kind,
        title: title_text,
        narrative: narrative_text,
        files,
        code_refs,
        concepts,
        importance: draft.importance,
        confidence: draft.confidence,
        auto_inject_eligible,
        privacy_flags,
        created_at_ms: draft.created_at_ms,
    }
}

fn scope_for_project(project_id: Option<&str>) -> MemoryScope {
    if project_id.is_some() {
        MemoryScope::Project
    } else {
        MemoryScope::Thread
    }
}

fn merge_privacy_flags(left: MemoryPrivacyFlags, right: MemoryPrivacyFlags) -> MemoryPrivacyFlags {
    MemoryPrivacyFlags {
        redacted_secret: left.redacted_secret || right.redacted_secret,
        redacted_private_block: left.redacted_private_block || right.redacted_private_block,
        sensitive_path: left.sensitive_path || right.sensitive_path,
        output_truncated: left.output_truncated || right.output_truncated,
        suspicious_injection: left.suspicious_injection || right.suspicious_injection,
    }
}

fn filter_sensitive_files(files: Vec<String>) -> (Vec<String>, bool) {
    let mut filtered = Vec::new();
    let mut sensitive = false;

    for file in files {
        if classify_memory_path(&file) == MemoryPathSensitivity::Sensitive {
            sensitive = true;
        } else if !filtered.contains(&file) {
            filtered.push(file);
        }
    }

    (filtered, sensitive)
}

fn filter_sensitive_code_refs(code_refs: Vec<MemoryCodeRef>) -> (Vec<MemoryCodeRef>, bool) {
    let mut filtered = Vec::new();
    let mut sensitive = false;

    for code_ref in code_refs {
        if classify_memory_path(&code_ref.path) == MemoryPathSensitivity::Sensitive {
            sensitive = true;
        } else if !filtered.iter().any(|existing: &MemoryCodeRef| {
            existing.path == code_ref.path
                && existing.line == code_ref.line
                && existing.symbol == code_ref.symbol
        }) {
            filtered.push(code_ref);
        }
    }

    (filtered, sensitive)
}

fn display_file_list(files: &[String]) -> String {
    files
        .iter()
        .map(|file| display_path(file))
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_path(path: &str) -> String {
    if classify_memory_path(path) == MemoryPathSensitivity::Sensitive {
        "[REDACTED_PATH]".to_string()
    } else {
        path.to_string()
    }
}

fn redact_sensitive_paths_in_concepts(concepts: Vec<String>) -> (Vec<String>, bool) {
    let mut sensitive = false;
    let concepts = concepts
        .into_iter()
        .map(|concept| {
            let (redacted, concept_sensitive) = redact_sensitive_paths_in_text(&concept);
            sensitive |= concept_sensitive;
            redacted
        })
        .collect();

    (concepts, sensitive)
}

fn redact_sensitive_paths_in_text(input: &str) -> (String, bool) {
    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    let mut token_start = None;
    let mut sensitive = false;

    for (index, ch) in input.char_indices() {
        if is_path_token_char(ch) {
            token_start.get_or_insert(index);
            continue;
        }

        if let Some(start) = token_start.take() {
            sensitive |=
                redact_sensitive_path_token(input, start, index, &mut output, &mut last_end);
        }
    }

    if let Some(start) = token_start {
        sensitive |=
            redact_sensitive_path_token(input, start, input.len(), &mut output, &mut last_end);
    }

    if sensitive {
        output.push_str(&input[last_end..]);
        (output, true)
    } else {
        (input.to_string(), false)
    }
}

fn redact_sensitive_path_token(
    input: &str,
    start: usize,
    end: usize,
    output: &mut String,
    last_end: &mut usize,
) -> bool {
    let token = &input[start..end];
    let classification_token = path_classification_token(token);
    if !looks_like_free_text_path(classification_token)
        || classify_memory_path(classification_token) != MemoryPathSensitivity::Sensitive
    {
        return false;
    }

    output.push_str(&input[*last_end..start]);
    output.push_str("[REDACTED_PATH]");
    *last_end = end;
    true
}

fn path_classification_token(token: &str) -> &str {
    let trimmed = token.trim_end_matches('.');
    if trimmed.is_empty() {
        token
    } else {
        trimmed
    }
}

fn looks_like_free_text_path(token: &str) -> bool {
    let lower = token.replace('\\', "/").to_ascii_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or(lower.as_str());

    lower.contains('/')
        || basename == ".env"
        || basename.starts_with(".env.")
        || basename == ".ssh"
        || basename == "credentials"
        || basename.starts_with("credentials.")
        || matches!(basename, "id_rsa" | "id_ed25519")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
}

fn is_path_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | '\\' | '~')
}

fn is_visible_user_message(message: &ConversationMessage) -> bool {
    message.role == MessageRole::User && !message.injected
}

fn looks_like_user_rule(text: &str) -> bool {
    const CJK_MARKERS: &[&str] = &[
        "不要", "必须", "务必", "记住", "切记", "以后", "今后", "请勿", "禁止", "千万", "约定",
    ];

    CJK_MARKERS.iter().any(|marker| text.contains(marker)) || looks_like_ascii_user_rule(text)
}

fn looks_like_ascii_user_rule(text: &str) -> bool {
    const ASCII_RULE_PREFIXES: &[&str] = &[
        "always ", "never ", "avoid ", "dont ", "don't ", "do not ", "must ",
    ];

    let lower = text.to_ascii_lowercase();
    let mut candidate = lower.trim_start();
    if candidate.ends_with('?') {
        return false;
    }

    for prefix in ["please ", "please, "] {
        if let Some(rest) = candidate.strip_prefix(prefix) {
            candidate = rest.trim_start();
            break;
        }
    }

    ASCII_RULE_PREFIXES
        .iter()
        .any(|prefix| candidate.starts_with(prefix))
}

fn stable_hash8(input: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

fn path_from_meta(meta: Option<&Value>) -> Option<String> {
    [
        "normalized_path",
        "path",
        "canonical_path",
        "requested_path",
    ]
    .iter()
    .find_map(|key| string_from_meta(meta, key))
}

fn string_from_meta(meta: Option<&Value>, key: &str) -> Option<String> {
    meta.and_then(|meta| meta.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn string_array_from_meta(meta: Option<&Value>, key: &str) -> Vec<String> {
    meta.and_then(|meta| meta.get(key))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn exit_code_from_meta(meta: Option<&Value>) -> Option<i64> {
    meta.and_then(|meta| meta.get("exit_code"))
        .and_then(Value::as_i64)
}

fn command_narrative(
    command: Option<&str>,
    exit_code: Option<i64>,
    stderr: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(command) = command {
        parts.push(format!("Command: {command}"));
    }
    if let Some(exit_code) = exit_code {
        parts.push(format!("Exit code: {exit_code}"));
    }
    if let Some(stderr) = stderr {
        parts.push(format!("Stderr:\n{stderr}"));
    }
    if parts.is_empty() {
        parts.push("Command completed.".to_string());
    }

    parts.join("\n")
}
