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

pub fn resolve_workspace_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err(anyhow!("Absolute paths are not allowed"));
    }

    let mut path = PathBuf::from(root);
    for component in candidate.components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err(anyhow!("Path escapes workspace")),
        }
    }

    Ok(path)
}
