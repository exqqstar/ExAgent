use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::{Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolCall, ToolResult, ToolStatus};
use crate::workspace::{
    canonical_read_roots, path_stays_within_roots, resolve_readable_path, ResolvedWorkspacePath,
};

const DEFAULT_MAX_RESULTS: usize = 50;
const HARD_MAX_RESULTS: usize = 200;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LINE_CHARS: usize = 500;
const MAX_FORMATTED_BYTES: usize = 16 * 1024;
const OUTPUT_TRUNCATED_MARKER: &str = "[output truncated]";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchFilesArgs {
    pub query: String,
    pub path: Option<String>,
    pub max_results: Option<usize>,
}

pub struct SearchFilesTool;

#[async_trait]
impl ToolHandler for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "search_files",
            "Search UTF-8 text files in the workspace or configured skill roots and return matching lines",
            serde_json::to_value(schemars::schema_for!(SearchFilesArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<SearchFilesArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                })
            }
        };

        match search_files(
            &ctx.config.workspace_root,
            &ctx.config.skills_user_roots,
            &args,
        ) {
            Ok((resolved, matches)) => {
                let formatted = format_matches(&matches);
                ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Success,
                    content: formatted.content,
                    meta: Some(json!({
                        "query": args.query,
                        "path": resolved.canonical_path,
                        "match_count": matches.len(),
                        "truncated": formatted.truncated,
                    })),
                })
            }
            Err(err) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err,
                meta: None,
            }),
        }
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }

    fn description(&self) -> &'static str {
        "Search UTF-8 text files in the workspace or configured skill roots"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(SearchFilesArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.handle(invocation, ctx).await.model_result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchMatch {
    display_path: PathBuf,
    line_number: usize,
    line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FormattedMatches {
    content: String,
    truncated: bool,
}

fn search_files(
    workspace_root: &Path,
    extra_read_roots: &[PathBuf],
    args: &SearchFilesArgs,
) -> Result<(ResolvedWorkspacePath, Vec<SearchMatch>), String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    let search_path = args.path.as_deref().unwrap_or(".");
    let resolved = resolve_readable_path(workspace_root, extra_read_roots, search_path)
        .map_err(|err| err.to_string())?;
    let metadata = std::fs::metadata(&resolved.canonical_path).map_err(|err| err.to_string())?;
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, HARD_MAX_RESULTS);
    let readable_roots =
        canonical_read_roots(workspace_root, extra_read_roots).map_err(|err| err.to_string())?;
    let workspace_root = readable_roots
        .first()
        .ok_or_else(|| "workspace_root does not exist or is not accessible".to_string())?
        .clone();
    let mut matches = Vec::new();

    if metadata.is_file() {
        search_file(
            &workspace_root,
            &readable_roots,
            &resolved.canonical_path,
            query,
            max_results,
            &mut matches,
        )?;
    } else if metadata.is_dir() {
        search_dir(
            &workspace_root,
            &readable_roots,
            &resolved.canonical_path,
            query,
            max_results,
            &mut matches,
        )?;
    } else {
        return Err("path must be a file or directory".to_string());
    }

    Ok((resolved, matches))
}

fn search_dir(
    workspace_root: &Path,
    readable_roots: &[PathBuf],
    dir: &Path,
    query: &str,
    max_results: usize,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), String> {
    if matches.len() >= max_results {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(dir)
        .map_err(|err| err.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if matches.len() >= max_results {
            break;
        }
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            search_dir(
                workspace_root,
                readable_roots,
                &path,
                query,
                max_results,
                matches,
            )?;
        } else if file_type.is_file() {
            search_file(
                workspace_root,
                readable_roots,
                &path,
                query,
                max_results,
                matches,
            )?;
        }
    }

    Ok(())
}

fn search_file(
    workspace_root: &Path,
    readable_roots: &[PathBuf],
    file: &Path,
    query: &str,
    max_results: usize,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), String> {
    if matches.len() >= max_results {
        return Ok(());
    }
    let canonical_file = std::fs::canonicalize(file).map_err(|err| err.to_string())?;
    if !path_stays_within_roots(&canonical_file, readable_roots) {
        return Ok(());
    }
    let metadata = std::fs::metadata(&canonical_file).map_err(|err| err.to_string())?;
    if metadata.len() > MAX_FILE_BYTES {
        return Ok(());
    }
    let body = match std::fs::read_to_string(&canonical_file) {
        Ok(body) => body,
        Err(_) => return Ok(()),
    };
    let display_path = canonical_file
        .strip_prefix(workspace_root)
        .unwrap_or(&canonical_file);
    for (index, line) in body.lines().enumerate() {
        if matches.len() >= max_results {
            break;
        }
        if line.contains(query) {
            matches.push(SearchMatch {
                display_path: display_path.to_path_buf(),
                line_number: index + 1,
                line: line.to_string(),
            });
        }
    }
    Ok(())
}

fn format_matches(matches: &[SearchMatch]) -> FormattedMatches {
    if matches.is_empty() {
        return FormattedMatches {
            content: "No matches found".to_string(),
            truncated: false,
        };
    }
    let mut lines = Vec::new();
    let mut total_bytes = 0usize;
    let mut truncated = false;

    for entry in matches {
        let line = truncate_line(&entry.line);
        truncated |= line.truncated;
        let formatted = format!(
            "{}:{}: {}",
            entry.display_path.display(),
            entry.line_number,
            line.content
        );
        let next_bytes = total_bytes
            .saturating_add(if lines.is_empty() { 0 } else { 1 })
            .saturating_add(formatted.len());
        if next_bytes > MAX_FORMATTED_BYTES {
            truncated = true;
            push_output_truncated_marker(&mut lines, &mut total_bytes);
            break;
        }
        total_bytes = next_bytes;
        lines.push(formatted);
    }

    FormattedMatches {
        content: lines.join("\n"),
        truncated,
    }
}

fn push_output_truncated_marker(lines: &mut Vec<String>, total_bytes: &mut usize) {
    loop {
        let separator_bytes = if lines.is_empty() { 0 } else { 1 };
        if total_bytes
            .saturating_add(separator_bytes)
            .saturating_add(OUTPUT_TRUNCATED_MARKER.len())
            <= MAX_FORMATTED_BYTES
        {
            *total_bytes = total_bytes
                .saturating_add(separator_bytes)
                .saturating_add(OUTPUT_TRUNCATED_MARKER.len());
            lines.push(OUTPUT_TRUNCATED_MARKER.to_string());
            return;
        }

        let Some(removed) = lines.pop() else {
            lines.push(OUTPUT_TRUNCATED_MARKER.to_string());
            *total_bytes = OUTPUT_TRUNCATED_MARKER.len();
            return;
        };
        *total_bytes = total_bytes.saturating_sub(removed.len());
        if !lines.is_empty() {
            *total_bytes = total_bytes.saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TruncatedLine {
    content: String,
    truncated: bool,
}

fn truncate_line(line: &str) -> TruncatedLine {
    let mut chars = line.chars();
    let content = chars.by_ref().take(MAX_LINE_CHARS).collect::<String>();
    if chars.next().is_some() {
        TruncatedLine {
            content: format!("{content} [line truncated]"),
            truncated: true,
        }
    } else {
        TruncatedLine {
            content,
            truncated: false,
        }
    }
}
