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
    resolve_readable_path(root, &[], raw)
}

pub fn resolve_readable_path(
    root: &Path,
    extra_read_roots: &[PathBuf],
    raw: &str,
) -> Result<ResolvedWorkspacePath> {
    let readable_roots = canonical_read_roots(root, extra_read_roots)?;
    let root = readable_roots
        .first()
        .ok_or_else(|| anyhow!("workspace_root does not exist or is not accessible"))?;
    let requested_path = PathBuf::from(raw);
    let was_absolute = requested_path.is_absolute();
    let candidate = if was_absolute {
        requested_path.clone()
    } else {
        root.join(&requested_path)
    };
    let normalized_path = normalize_path(&candidate)?;
    let canonical_path = canonicalize_existing_or_missing(&normalized_path)?;

    if !path_stays_within_roots(&canonical_path, &readable_roots) {
        bail!("Path must stay within workspace_root");
    }

    Ok(ResolvedWorkspacePath {
        requested_path,
        normalized_path,
        canonical_path,
        was_absolute,
    })
}

pub fn canonical_read_roots(root: &Path, extra_read_roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let root = std::fs::canonicalize(root).with_context(|| {
        format!(
            "workspace_root does not exist or is not accessible: {}",
            root.display()
        )
    })?;
    let mut roots = vec![root];

    for extra_root in extra_read_roots {
        if let Ok(canonical_root) = std::fs::canonicalize(extra_root) {
            roots.push(canonical_root);
        }
    }

    Ok(roots)
}

pub fn path_stays_within_roots(canonical_path: &Path, canonical_roots: &[PathBuf]) -> bool {
    canonical_roots
        .iter()
        .any(|root| canonical_path.starts_with(root))
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

#[cfg(test)]
mod tests {
    use super::{resolve_readable_path, resolve_workspace_path};
    use tempfile::tempdir;

    #[test]
    fn resolve_readable_path_accepts_absolute_path_under_extra_root() {
        let workspace = tempdir().unwrap();
        let skill_root = tempdir().unwrap();
        let skill_path = skill_root.path().join("skill").join("SKILL.md");
        std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        std::fs::write(&skill_path, "body").unwrap();

        let resolved = resolve_readable_path(
            workspace.path(),
            &[skill_root.path().to_path_buf()],
            &skill_path.display().to_string(),
        )
        .unwrap();

        assert_eq!(
            resolved.canonical_path,
            std::fs::canonicalize(skill_path).unwrap()
        );
        assert!(resolved.was_absolute);
    }

    #[test]
    fn resolve_readable_path_rejects_absolute_path_outside_allowed_roots() {
        let workspace = tempdir().unwrap();
        let skill_root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_path = outside.path().join("secret.txt");
        std::fs::write(&outside_path, "secret").unwrap();

        let err = resolve_readable_path(
            workspace.path(),
            &[skill_root.path().to_path_buf()],
            &outside_path.display().to_string(),
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "Path must stay within workspace_root");
    }

    #[test]
    fn resolve_readable_path_keeps_relative_paths_anchored_to_workspace() {
        let workspace = tempdir().unwrap();
        let skill_root = tempdir().unwrap();
        std::fs::write(workspace.path().join("notes.txt"), "workspace").unwrap();
        std::fs::write(skill_root.path().join("notes.txt"), "skill").unwrap();

        let resolved = resolve_readable_path(
            workspace.path(),
            &[skill_root.path().to_path_buf()],
            "notes.txt",
        )
        .unwrap();

        assert_eq!(
            resolved.canonical_path,
            std::fs::canonicalize(workspace.path().join("notes.txt")).unwrap()
        );
        assert!(!resolved.was_absolute);
    }

    #[test]
    fn resolve_readable_path_skips_nonexistent_extra_roots() {
        let workspace = tempdir().unwrap();
        let root_parent = tempdir().unwrap();
        let missing_root = root_parent.path().join("missing-root");
        let path_under_missing_root = missing_root.join("skill").join("SKILL.md");

        let err = resolve_readable_path(
            workspace.path(),
            &[missing_root],
            &path_under_missing_root.display().to_string(),
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "Path must stay within workspace_root");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_readable_path_rejects_symlink_escape_from_extra_root() {
        let workspace = tempdir().unwrap();
        let skill_root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_path = outside.path().join("secret.txt");
        std::fs::write(&outside_path, "secret").unwrap();
        let link_path = skill_root.path().join("secret-link.txt");
        std::os::unix::fs::symlink(&outside_path, &link_path).unwrap();

        let err = resolve_readable_path(
            workspace.path(),
            &[skill_root.path().to_path_buf()],
            &link_path.display().to_string(),
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "Path must stay within workspace_root");
    }

    #[test]
    fn resolve_readable_path_with_empty_extra_roots_matches_workspace_resolver() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_path = outside.path().join("outside.txt");
        std::fs::write(&outside_path, "outside").unwrap();
        let raw = outside_path.display().to_string();

        let readable_err = resolve_readable_path(workspace.path(), &[], &raw)
            .unwrap_err()
            .to_string();
        let workspace_err = resolve_workspace_path(workspace.path(), &raw)
            .unwrap_err()
            .to_string();

        assert_eq!(readable_err, workspace_err);
    }
}
