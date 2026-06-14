use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

use super::types::MemoryCodeRef;

const MAX_EXISTENCE_CHECKS_PER_HIT: usize = 6;
const WORKING_SET_BOOST: f64 = 0.25;
const STALE_PENALTY: f64 = -0.45;

#[derive(Debug, Clone, PartialEq)]
pub struct CodeAwarenessSnapshot {
    pub workspace_root: Option<PathBuf>,
    pub prompt_paths: HashSet<String>,
    pub working_set_paths: HashSet<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CodeRefScore {
    pub stale: bool,
    pub working_set_boost: f64,
    pub stale_penalty: f64,
}

impl CodeAwarenessSnapshot {
    pub fn from_prompt(workspace_root: Option<PathBuf>, prompt: &str) -> Self {
        let prompt_paths = extract_paths(workspace_root.as_deref(), prompt);
        Self {
            workspace_root,
            working_set_paths: prompt_paths.clone(),
            prompt_paths,
        }
    }

    pub fn score_refs(&self, files: &[String], code_refs: &[MemoryCodeRef]) -> CodeRefScore {
        let mut stale_check_budget = MAX_EXISTENCE_CHECKS_PER_HIT;
        self.score_refs_with_budget(files, code_refs, &mut stale_check_budget)
    }

    pub fn score_refs_with_budget(
        &self,
        files: &[String],
        code_refs: &[MemoryCodeRef],
        stale_check_budget: &mut usize,
    ) -> CodeRefScore {
        let mut referenced_paths = Vec::new();
        for file in files {
            push_normalized_path(&mut referenced_paths, self.workspace_root.as_deref(), file);
        }
        for code_ref in code_refs {
            push_normalized_path(
                &mut referenced_paths,
                self.workspace_root.as_deref(),
                &code_ref.path,
            );
        }

        let working_set_boost = if referenced_paths
            .iter()
            .any(|path| self.matches_working_set(path))
        {
            WORKING_SET_BOOST
        } else {
            0.0
        };

        let stale = self
            .workspace_root
            .as_ref()
            .filter(|root| root.exists())
            .is_some_and(|root| {
                referenced_paths
                    .iter()
                    .take_while(|_| {
                        if *stale_check_budget == 0 {
                            false
                        } else {
                            *stale_check_budget -= 1;
                            true
                        }
                    })
                    .any(|path| !root.join(path).exists())
            });

        CodeRefScore {
            stale,
            working_set_boost,
            stale_penalty: if stale { STALE_PENALTY } else { 0.0 },
        }
    }

    fn matches_working_set(&self, path: &str) -> bool {
        self.prompt_paths
            .iter()
            .chain(self.working_set_paths.iter())
            .any(|working_path| paths_match(path, working_path))
    }
}

fn extract_paths(workspace_root: Option<&Path>, prompt: &str) -> HashSet<String> {
    let mut paths = HashSet::new();
    for raw in prompt.split(|ch: char| ch.is_whitespace()) {
        let token = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\''
                    | '`'
                    | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
            )
        });
        if !looks_like_path(token) {
            continue;
        }
        if let Some(path) = normalize_ref_path(workspace_root, token) {
            paths.insert(path);
        }
    }
    paths
}

fn looks_like_path(token: &str) -> bool {
    token.contains('/') || token.starts_with("./") || token.starts_with("../")
}

fn push_normalized_path(paths: &mut Vec<String>, workspace_root: Option<&Path>, raw: &str) {
    if let Some(path) = normalize_ref_path(workspace_root, raw) {
        if !paths.iter().any(|existing| existing == &path) {
            paths.push(path);
        }
    }
}

fn normalize_ref_path(workspace_root: Option<&Path>, raw: &str) -> Option<String> {
    let without_fragment = raw
        .split_once('#')
        .map(|(path, _)| path)
        .unwrap_or(raw)
        .split_once(':')
        .map(|(path, suffix)| {
            if suffix.chars().all(|ch| ch.is_ascii_digit()) {
                path
            } else {
                raw
            }
        })
        .unwrap_or(raw)
        .trim();
    let trimmed = without_fragment.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        let root = workspace_root?;
        if let Ok(relative) = path.strip_prefix(root) {
            return normalize_relative_path(relative);
        }

        let canonical_path = path.canonicalize().ok()?;
        let canonical_root = root.canonicalize().ok()?;
        let relative = canonical_path.strip_prefix(canonical_root).ok()?;
        return normalize_relative_path(relative);
    }

    normalize_relative_path(path)
}

fn normalize_relative_path(path: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => components.push(part.to_string_lossy().to_string()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if components.is_empty() {
        None
    } else {
        Some(components.join("/"))
    }
}

fn paths_match(reference: &str, working_path: &str) -> bool {
    reference == working_path
        || reference.ends_with(&format!("/{working_path}"))
        || working_path.ends_with(&format!("/{reference}"))
}
