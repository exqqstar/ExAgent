use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::{Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolCall, ToolResult, ToolStatus};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyPatchArgs {
    pub patch: String,
}

pub struct ApplyPatchTool;

#[async_trait]
impl ToolHandler for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "apply_patch",
            "Apply a structured patch to files inside the workspace",
            serde_json::to_value(schemars::schema_for!(ApplyPatchArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::mutating(false)
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<ApplyPatchArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => {
                return ToolOutcome::from_result(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    status: ToolStatus::Error,
                    content: err.to_string(),
                    meta: None,
                });
            }
        };

        match apply_patch(&ctx.config.workspace_root, &args.patch) {
            Ok(summary) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: format!("Applied patch to {} file(s)", summary.changed_files.len()),
                meta: Some(json!({ "changed_files": summary.changed_files })),
            }),
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
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Apply a structured patch to files inside the workspace"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(ApplyPatchArgs)).unwrap()
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
struct PatchSummary {
    changed_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FilePatch {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        changes: Vec<ChangeLine>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChangeLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreparedPatch {
    Add {
        path: String,
        target: PathBuf,
        content: String,
    },
    Delete {
        path: String,
        target: PathBuf,
    },
    Update {
        path: String,
        target: PathBuf,
        content: String,
    },
    Move {
        path: String,
        source: PathBuf,
        move_to: String,
        destination: PathBuf,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

fn apply_patch(workspace_root: &Path, patch: &str) -> Result<PatchSummary, String> {
    let patches = parse_patch(patch)?;
    let prepared = prepare_patches(workspace_root, patches)?;
    let changed_files = prepared.iter().map(PreparedPatch::changed_path).collect();

    commit_prepared_patches(&prepared)?;

    Ok(PatchSummary { changed_files })
}

fn prepare_patches(
    workspace_root: &Path,
    patches: Vec<FilePatch>,
) -> Result<Vec<PreparedPatch>, String> {
    let mut prepared = Vec::new();
    let mut planned_mutations = HashSet::new();

    for patch in patches {
        match patch {
            FilePatch::Add { path, lines } => {
                let target = resolve_patch_path(workspace_root, &path, false)?;
                if path_exists(&target)? {
                    return Err(format!("Add file already exists: {path}"));
                }
                reserve_mutation(&mut planned_mutations, &target, &path)?;
                prepared.push(PreparedPatch::Add {
                    path,
                    target,
                    content: lines_to_file(&lines),
                });
            }
            FilePatch::Delete { path } => {
                let target = resolve_patch_path(workspace_root, &path, true)?;
                reserve_mutation(&mut planned_mutations, &target, &path)?;
                prepared.push(PreparedPatch::Delete { path, target });
            }
            FilePatch::Update {
                path,
                move_to,
                changes,
            } => {
                let source = resolve_patch_path(workspace_root, &path, true)?;
                let body = std::fs::read_to_string(&source).map_err(|err| err.to_string())?;
                let updated = apply_change_lines(&body, &changes)?;

                if let Some(move_to) = move_to {
                    let destination = resolve_patch_path(workspace_root, &move_to, false)?;
                    if destination != source && path_exists(&destination)? {
                        return Err(format!("Move target already exists: {move_to}"));
                    }
                    reserve_mutation(&mut planned_mutations, &source, &path)?;
                    if destination != source {
                        reserve_mutation(&mut planned_mutations, &destination, &move_to)?;
                    }
                    prepared.push(PreparedPatch::Move {
                        path,
                        source,
                        move_to,
                        destination,
                        content: updated,
                    });
                } else {
                    reserve_mutation(&mut planned_mutations, &source, &path)?;
                    prepared.push(PreparedPatch::Update {
                        path,
                        target: source,
                        content: updated,
                    });
                }
            }
        }
    }

    Ok(prepared)
}

impl PreparedPatch {
    fn changed_path(&self) -> String {
        match self {
            PreparedPatch::Add { path, .. }
            | PreparedPatch::Delete { path, .. }
            | PreparedPatch::Update { path, .. } => path.clone(),
            PreparedPatch::Move { move_to, .. } => move_to.clone(),
        }
    }
}

fn reserve_mutation(
    planned_mutations: &mut HashSet<PathBuf>,
    target: &Path,
    label: &str,
) -> Result<(), String> {
    if !planned_mutations.insert(target.to_path_buf()) {
        return Err(format!("Path is modified more than once in patch: {label}"));
    }
    Ok(())
}

fn commit_prepared_patches(prepared: &[PreparedPatch]) -> Result<(), String> {
    let mut rollback = RollbackLog::default();

    for patch in prepared {
        let result = (|| -> Result<(), String> {
            match patch {
                PreparedPatch::Add {
                    target, content, ..
                }
                | PreparedPatch::Update {
                    target, content, ..
                } => {
                    rollback.capture(target)?;
                    write_file(target, content)
                }
                PreparedPatch::Delete { target, .. } => {
                    rollback.capture(target)?;
                    std::fs::remove_file(target).map_err(|err| err.to_string())
                }
                PreparedPatch::Move {
                    source,
                    destination,
                    content,
                    ..
                } => {
                    rollback.capture(destination)?;
                    rollback.capture(source)?;
                    write_file(destination, content)?;
                    if destination != source {
                        std::fs::remove_file(source).map_err(|err| err.to_string())?;
                    }
                    Ok(())
                }
            }
        })();

        if let Err(err) = result {
            if let Err(rollback_err) = rollback.rollback() {
                return Err(format!("{err}; rollback failed: {rollback_err}"));
            }
            return Err(err);
        }
    }

    Ok(())
}

#[derive(Default)]
struct RollbackLog {
    snapshots: Vec<FileSnapshot>,
    seen: HashSet<PathBuf>,
}

impl RollbackLog {
    fn capture(&mut self, path: &Path) -> Result<(), String> {
        if !self.seen.insert(path.to_path_buf()) {
            return Ok(());
        }
        let content = match std::fs::read(path) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.to_string()),
        };
        self.snapshots.push(FileSnapshot {
            path: path.to_path_buf(),
            content,
        });
        Ok(())
    }

    fn rollback(&self) -> Result<(), String> {
        for snapshot in self.snapshots.iter().rev() {
            match &snapshot.content {
                Some(content) => {
                    if let Some(parent) = snapshot.path.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
                    }
                    std::fs::write(&snapshot.path, content).map_err(|err| err.to_string())?;
                }
                None => {
                    if path_exists(&snapshot.path)? {
                        std::fs::remove_file(&snapshot.path).map_err(|err| err.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(path, content).map_err(|err| err.to_string())
}

fn path_exists(path: &Path) -> Result<bool, String> {
    path.try_exists().map_err(|err| err.to_string())
}

fn parse_patch(patch: &str) -> Result<Vec<FilePatch>, String> {
    let lines = patch.lines().collect::<Vec<_>>();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err("patch must start with `*** Begin Patch`".to_string());
    }

    let mut patches = Vec::new();
    let mut index = 1usize;
    while index < lines.len() {
        let line = lines[index];
        if line == "*** End Patch" {
            if index + 1 != lines.len() {
                return Err("unexpected content after `*** End Patch`".to_string());
            }
            return Ok(patches);
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut body = Vec::new();
            while index < lines.len() && !is_patch_boundary(lines[index]) {
                let line = lines[index];
                let Some(content) = line.strip_prefix('+') else {
                    return Err(format!("add file lines must start with `+`: {line}"));
                };
                body.push(content.to_string());
                index += 1;
            }
            patches.push(FilePatch::Add {
                path: path.to_string(),
                lines: body,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            patches.push(FilePatch::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            let mut changes = Vec::new();
            if index < lines.len() {
                if let Some(destination) = lines[index].strip_prefix("*** Move to: ") {
                    move_to = Some(destination.to_string());
                    index += 1;
                }
            }
            while index < lines.len() && !is_patch_boundary(lines[index]) {
                let line = lines[index];
                if line.starts_with("@@") {
                    index += 1;
                    continue;
                }
                if let Some(content) = line.strip_prefix('+') {
                    changes.push(ChangeLine::Add(content.to_string()));
                } else if let Some(content) = line.strip_prefix('-') {
                    changes.push(ChangeLine::Remove(content.to_string()));
                } else if let Some(content) = line.strip_prefix(' ') {
                    changes.push(ChangeLine::Context(content.to_string()));
                } else {
                    return Err(format!(
                        "update lines must start with ` `, `-`, `+`, or `@@`: {line}"
                    ));
                }
                index += 1;
            }
            patches.push(FilePatch::Update {
                path: path.to_string(),
                move_to,
                changes,
            });
            continue;
        }

        return Err(format!("unsupported patch header: {line}"));
    }

    Err("patch must end with `*** End Patch`".to_string())
}

fn is_patch_boundary(line: &str) -> bool {
    line == "*** End Patch"
        || line.starts_with("*** Add File: ")
        || line.starts_with("*** Delete File: ")
        || line.starts_with("*** Update File: ")
}

fn apply_change_lines(body: &str, changes: &[ChangeLine]) -> Result<String, String> {
    let original = body.lines().map(str::to_string).collect::<Vec<_>>();
    let had_trailing_newline = body.ends_with('\n');
    let mut output = Vec::new();
    let mut cursor = 0usize;

    for change in changes {
        match change {
            ChangeLine::Add(line) => output.push(line.clone()),
            ChangeLine::Context(expected) => {
                let found = find_line(&original, cursor, expected).ok_or_else(|| {
                    format!(
                        "context line not found after line {}: {expected}",
                        cursor + 1
                    )
                })?;
                output.extend(original[cursor..found].iter().cloned());
                output.push(original[found].clone());
                cursor = found + 1;
            }
            ChangeLine::Remove(expected) => {
                let found = find_line(&original, cursor, expected).ok_or_else(|| {
                    format!(
                        "remove line not found after line {}: {expected}",
                        cursor + 1
                    )
                })?;
                output.extend(original[cursor..found].iter().cloned());
                cursor = found + 1;
            }
        }
    }

    output.extend(original[cursor..].iter().cloned());
    Ok(lines_to_file_with_trailing(&output, had_trailing_newline))
}

fn find_line(lines: &[String], start: usize, expected: &str) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, line)| (line == expected).then_some(index))
}

fn lines_to_file(lines: &[String]) -> String {
    lines_to_file_with_trailing(lines, true)
}

fn lines_to_file_with_trailing(lines: &[String], trailing_newline: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut body = lines.join("\n");
    if trailing_newline {
        body.push('\n');
    }
    body
}

fn resolve_patch_path(
    workspace_root: &Path,
    raw: &str,
    must_exist: bool,
) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(workspace_root).map_err(|err| err.to_string())?;
    let requested = PathBuf::from(raw);
    let candidate = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    };
    let normalized = normalize_path(&candidate)?;
    let resolved = if must_exist {
        std::fs::canonicalize(&normalized).map_err(|err| err.to_string())?
    } else {
        canonicalize_existing_or_missing(&normalized)?
    };
    if !resolved.starts_with(&root) {
        return Err("Path must stay within workspace_root".to_string());
    }
    Ok(resolved)
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    return Err("Path escapes workspace".to_string());
                }
            }
        }
    }
    Ok(normalized)
}

fn canonicalize_existing_or_missing(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path;
    let mut missing = Vec::<OsString>::new();

    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => {
                let mut canonical = std::fs::canonicalize(existing)
                    .map_err(|err| format!("path does not exist or is not accessible: {err}"))?;
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    format!("path does not have an existing parent: {}", path.display())
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    format!("path does not have an existing parent: {}", path.display())
                })?;
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}
