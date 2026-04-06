use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};

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
