use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};
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
    /// Regex query to match against UTF-8 text lines; invalid regexes fall back to literal text.
    pub query: String,
    /// Optional workspace-relative directory or file to scope the search. Defaults to the workspace root.
    pub path: Option<String>,
    /// Glob filter on the displayed file path, e.g. "*.rs" or "src/**/*.ts".
    pub glob: Option<String>,
    /// Run a case-insensitive search.
    pub case_insensitive: Option<bool>,
    /// Maximum number of matches to return. Defaults to 50; capped at 200.
    pub max_results: Option<usize>,
}

pub struct SearchFilesTool;

#[async_trait]
impl ToolHandler for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "search_files",
            "Search UTF-8 text files in the workspace or configured skill roots using regex, gitignore-aware traversal, optional glob filtering, and case-insensitive matching",
            serde_json::to_value(schemars::schema_for!(SearchFilesArgs)).unwrap(),
        )
        // Internal contract: describes the structured `meta` side-channel this
        // tool emits (model-facing content is the formatted matches). ADR-0042.
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The query that was searched for." },
                "path": { "type": "string", "description": "Canonical path that was searched." },
                "match_count": { "type": "integer", "description": "Number of matching lines returned." },
                "truncated": { "type": "boolean", "description": "Whether the result set was truncated." },
                "query_mode": { "type": "string", "description": "Whether the query ran as regex or literal text." },
                "glob": { "type": ["string", "null"], "description": "Glob filter applied to file paths, if any." },
                "case_insensitive": { "type": "boolean", "description": "Whether matching was case-insensitive." }
            },
            "required": ["query", "path", "match_count", "truncated", "query_mode", "glob", "case_insensitive"],
            "additionalProperties": false
        }))
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
                    parts: Vec::new(),
                })
            }
        };

        match search_files(
            &ctx.config.workspace_root,
            &ctx.config.skills_user_roots,
            &args,
        ) {
            Ok((resolved, matches)) => {
                let formatted = format_matches(&matches.entries);
                ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Success,
                    content: formatted.content,
                    meta: Some(json!({
                        "query": args.query,
                        "path": resolved.canonical_path,
                        "match_count": matches.entries.len(),
                        "truncated": formatted.truncated,
                        "query_mode": matches.query_mode,
                        "glob": args.glob,
                        "case_insensitive": args.case_insensitive.unwrap_or(false),
                    })),
                    parts: Vec::new(),
                })
            }
            Err(err) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Error,
                content: err,
                meta: None,
                parts: Vec::new(),
            }),
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchMatches {
    entries: Vec<SearchMatch>,
    query_mode: &'static str,
}

enum QueryMatcher {
    Regex(regex::Regex),
    Literal {
        needle: String,
        case_insensitive: bool,
    },
}

impl QueryMatcher {
    fn new(query: &str, case_insensitive: bool) -> Self {
        match RegexBuilder::new(query)
            .case_insensitive(case_insensitive)
            .build()
        {
            Ok(regex) => Self::Regex(regex),
            Err(_) => {
                let needle = if case_insensitive {
                    query.to_lowercase()
                } else {
                    query.to_string()
                };
                Self::Literal {
                    needle,
                    case_insensitive,
                }
            }
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(line),
            Self::Literal {
                needle,
                case_insensitive,
            } => {
                if *case_insensitive {
                    line.to_lowercase().contains(needle)
                } else {
                    line.contains(needle)
                }
            }
        }
    }

    fn mode(&self) -> &'static str {
        match self {
            Self::Regex(_) => "regex",
            Self::Literal { .. } => "literal",
        }
    }
}

fn search_files(
    workspace_root: &Path,
    extra_read_roots: &[PathBuf],
    args: &SearchFilesArgs,
) -> Result<(ResolvedWorkspacePath, SearchMatches), String> {
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
    let matcher = QueryMatcher::new(query, args.case_insensitive.unwrap_or(false));
    let glob = compile_glob(args.glob.as_deref())?;
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
            &matcher,
            glob.as_ref(),
            max_results,
            &mut matches,
        )?;
    } else if metadata.is_dir() {
        search_dir(
            &workspace_root,
            &readable_roots,
            &resolved.canonical_path,
            &matcher,
            glob.as_ref(),
            max_results,
            &mut matches,
        )?;
    } else {
        return Err("path must be a file or directory".to_string());
    }

    Ok((
        resolved,
        SearchMatches {
            entries: matches,
            query_mode: matcher.mode(),
        },
    ))
}

fn search_dir(
    workspace_root: &Path,
    readable_roots: &[PathBuf],
    dir: &Path,
    matcher: &QueryMatcher,
    glob: Option<&GlobMatcher>,
    max_results: usize,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), String> {
    if matches.len() >= max_results {
        return Ok(());
    }
    let mut builder = WalkBuilder::new(dir);
    builder
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b));

    for entry in builder.build() {
        if matches.len() >= max_results {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            search_file(
                workspace_root,
                readable_roots,
                entry.path(),
                matcher,
                glob,
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
    matcher: &QueryMatcher,
    glob: Option<&GlobMatcher>,
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
    if let Some(glob) = glob {
        if !glob.is_match(display_path) {
            return Ok(());
        }
    }
    for (index, line) in body.lines().enumerate() {
        if matches.len() >= max_results {
            break;
        }
        if matcher.is_match(line) {
            matches.push(SearchMatch {
                display_path: display_path.to_path_buf(),
                line_number: index + 1,
                line: line.to_string(),
            });
        }
    }
    Ok(())
}

fn compile_glob(pattern: Option<&str>) -> Result<Option<GlobMatcher>, String> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let glob = GlobBuilder::new(pattern)
        .build()
        .map_err(|err| err.to_string())?;
    Ok(Some(glob.compile_matcher()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolSpecKind;

    #[test]
    fn tool_spec_advertises_regex_gitignore_and_case_insensitive_search() {
        let spec = SearchFilesTool.spec();

        assert!(spec.description.contains("regex"));
        assert!(spec.description.contains("gitignore"));
        assert!(spec.description.contains("case-insensitive"));

        let ToolSpecKind::Function { input_schema } = spec.kind;
        assert!(input_schema["properties"]["query"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("regex")));
        assert!(
            input_schema["properties"]["case_insensitive"]["description"]
                .as_str()
                .is_some_and(|description| description.contains("case-insensitive"))
        );
    }
}
