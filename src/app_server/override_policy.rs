use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::AgentConfig;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOverrides {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
}

pub struct OverridePolicy;

impl OverridePolicy {
    pub fn apply(base: &AgentConfig, overrides: RuntimeOverrides) -> Result<AgentConfig> {
        let mut config = base.clone();
        let workspace_root = match overrides.workspace_root.as_deref() {
            Some(raw_root) => canonicalize_from_current(raw_root)?,
            None => canonicalize_existing(&config.workspace_root)?,
        };
        config.workspace_root = workspace_root.clone();
        config.cwd = workspace_root.clone();

        if let Some(raw_cwd) = overrides.cwd.as_deref() {
            config.cwd = canonicalize_from_root(&workspace_root, raw_cwd)?;
        }

        Ok(config)
    }

    pub fn apply_workspace_only(
        base: &AgentConfig,
        workspace_root: Option<String>,
    ) -> Result<AgentConfig> {
        Self::apply(
            base,
            RuntimeOverrides {
                workspace_root,
                cwd: None,
            },
        )
    }
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| {
        format!(
            "Path does not exist or is not accessible: {}",
            path.display()
        )
    })
}

fn canonicalize_from_current(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(path)
    };

    canonicalize_existing(&path)
}

fn canonicalize_from_root(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };

    let candidate = canonicalize_existing(&candidate)?;

    if !candidate.starts_with(root) {
        bail!("cwd must stay within workspace_root");
    }

    Ok(candidate)
}
