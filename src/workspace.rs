use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

pub fn canonicalize_from_current(raw: &str, label: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(path)
    };

    std::fs::canonicalize(&path).with_context(|| {
        format!(
            "{label} does not exist or is not accessible: {}",
            path.display()
        )
    })
}

pub fn canonicalize_existing_cwd(workspace_root: &Path, cwd: &Path) -> Result<PathBuf> {
    let raw = cwd
        .to_str()
        .ok_or_else(|| anyhow!("cwd must be valid UTF-8"))?;
    canonicalize_from_root(workspace_root, raw)
}

pub fn canonicalize_from_root(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };

    let candidate = std::fs::canonicalize(&candidate).with_context(|| {
        format!(
            "cwd does not exist or is not accessible: {}",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(root) {
        bail!("cwd must stay within workspace_root");
    }

    Ok(candidate)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspacePath {
    pub requested_path: PathBuf,
    pub normalized_path: PathBuf,
    pub canonical_path: PathBuf,
    pub was_absolute: bool,
}

pub fn resolve_workspace_path(root: &Path, raw: &str) -> Result<ResolvedWorkspacePath> {
    let root = std::fs::canonicalize(root).with_context(|| {
        format!(
            "workspace_root does not exist or is not accessible: {}",
            root.display()
        )
    })?;
    let requested_path = PathBuf::from(raw);
    let was_absolute = requested_path.is_absolute();
    let candidate = if was_absolute {
        requested_path.clone()
    } else {
        root.join(&requested_path)
    };
    let normalized_path = normalize_path(&candidate)?;
    let canonical_path = canonicalize_existing_or_missing(&normalized_path)?;

    if !canonical_path.starts_with(&root) {
        bail!("Path must stay within workspace_root");
    }

    Ok(ResolvedWorkspacePath {
        requested_path,
        normalized_path,
        canonical_path,
        was_absolute,
    })
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    bail!("Path escapes workspace");
                }
            }
        }
    }
    Ok(normalized)
}

fn canonicalize_existing_or_missing(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::<OsString>::new();

    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => {
                let mut canonical = std::fs::canonicalize(existing).with_context(|| {
                    format!(
                        "path does not exist or is not accessible: {}",
                        path.display()
                    )
                })?;
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    anyhow!("path does not have an existing parent: {}", path.display())
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    anyhow!("path does not have an existing parent: {}", path.display())
                })?;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "path does not exist or is not accessible: {}",
                        path.display()
                    )
                });
            }
        }
    }
}
