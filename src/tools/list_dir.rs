use std::path::{Path, PathBuf};

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};
use crate::workspace::{
    canonical_read_roots, path_stays_within_roots, resolve_readable_path, ResolvedWorkspacePath,
};

const DEFAULT_DEPTH: usize = 2;
const MAX_DEPTH: usize = 10;
const DEFAULT_MAX_ENTRIES: usize = 200;
const HARD_MAX_ENTRIES: usize = 1000;
const OUTPUT_TRUNCATED_MARKER: &str = "[output truncated]";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    /// Directory to list. Relative paths resolve under the workspace root.
    pub path: Option<String>,
    /// Glob filter on displayed paths, e.g. "**/*.test.ts".
    pub glob: Option<String>,
    /// Recursion depth. 1 = immediate children only. Defaults to 2.
    pub depth: Option<usize>,
    pub max_entries: Option<usize>,
}

pub struct ListDirTool;

#[async_trait]
impl ToolHandler for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "list_dir",
            "List files and directories in the workspace or configured skill roots (gitignore-aware); supports glob find-by-name",
            serde_json::to_value(schemars::schema_for!(ListDirArgs)).unwrap(),
        )
        // Internal contract: describes the structured `meta` side-channel this
        // tool emits (model-facing content is the formatted listing). ADR-0042.
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Canonical path that was listed." },
                "entry_count": { "type": "integer", "description": "Number of entries returned." },
                "truncated": { "type": "boolean", "description": "Whether the listing was truncated." },
                "depth": { "type": "integer", "description": "Traversal depth used for the listing." },
                "glob": { "type": ["string", "null"], "description": "Glob find-by-name filter applied, if any." }
            },
            "required": ["path", "entry_count", "truncated", "depth", "glob"],
            "additionalProperties": false
        }))
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<ListDirArgs>(call.arguments) {
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

        match list_dir(
            &ctx.config.workspace_root,
            &ctx.config.skills_user_roots,
            &args,
        ) {
            Ok((resolved, listing)) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: format_entries(&listing.entries, listing.truncated),
                meta: Some(json!({
                    "path": resolved.canonical_path,
                    "entry_count": listing.entries.len(),
                    "truncated": listing.truncated,
                    "depth": listing.depth,
                    "glob": args.glob,
                })),
                parts: Vec::new(),
            }),
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
struct DirListing {
    entries: Vec<String>,
    truncated: bool,
    depth: usize,
}

fn list_dir(
    workspace_root: &Path,
    extra_read_roots: &[PathBuf],
    args: &ListDirArgs,
) -> Result<(ResolvedWorkspacePath, DirListing), String> {
    let path = args.path.as_deref().unwrap_or(".");
    let resolved = resolve_readable_path(workspace_root, extra_read_roots, path)
        .map_err(|err| err.to_string())?;
    let metadata = std::fs::metadata(&resolved.canonical_path).map_err(|err| err.to_string())?;
    if !metadata.is_dir() {
        return Err("path must be a directory".to_string());
    }

    let depth = args.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let max_entries = args
        .max_entries
        .unwrap_or(DEFAULT_MAX_ENTRIES)
        .clamp(1, HARD_MAX_ENTRIES);
    let glob = compile_glob(args.glob.as_deref())?;
    let readable_roots =
        canonical_read_roots(workspace_root, extra_read_roots).map_err(|err| err.to_string())?;
    let workspace_root = readable_roots
        .first()
        .ok_or_else(|| "workspace_root does not exist or is not accessible".to_string())?
        .clone();

    let mut builder = WalkBuilder::new(&resolved.canonical_path);
    builder
        .max_depth(Some(depth))
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b));

    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path() == resolved.canonical_path {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let canonical_path = match std::fs::canonicalize(entry.path()) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !path_stays_within_roots(&canonical_path, &readable_roots) {
            continue;
        }
        let display = display_path(&workspace_root, &canonical_path, file_type.is_dir());
        if let Some(glob) = glob.as_ref() {
            if !glob.is_match(display.trim_end_matches('/')) {
                continue;
            }
        }
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        entries.push(display);
    }

    Ok((
        resolved,
        DirListing {
            entries,
            truncated,
            depth,
        },
    ))
}

fn display_path(workspace_root: &Path, canonical_path: &Path, is_dir: bool) -> String {
    let path = canonical_path
        .strip_prefix(workspace_root)
        .unwrap_or(canonical_path);
    let mut display = path.display().to_string();
    if is_dir && !display.ends_with('/') {
        display.push('/');
    }
    display
}

fn format_entries(entries: &[String], truncated: bool) -> String {
    if entries.is_empty() {
        if truncated {
            return OUTPUT_TRUNCATED_MARKER.to_string();
        }
        return "No entries found".to_string();
    }

    let mut lines = entries.to_vec();
    if truncated {
        lines.push(OUTPUT_TRUNCATED_MARKER.to_string());
    }
    lines.join("\n")
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
