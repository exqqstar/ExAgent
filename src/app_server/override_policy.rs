use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::app_server::protocol::TurnContextOverrides;
use crate::app_server::AppServerError;
use crate::config::AgentConfig;
use crate::session::SessionSnapshot;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOverrides {
    pub workspace_root: Option<String>,
    pub cwd: Option<String>,
}

pub struct OverridePolicy;

impl OverridePolicy {
    pub fn merge_thread_start(
        base: &AgentConfig,
        overrides: RuntimeOverrides,
    ) -> Result<AgentConfig> {
        Self::apply(base, overrides)
    }

    pub fn merge_thread_read(
        base: &AgentConfig,
        workspace_root: Option<String>,
    ) -> Result<AgentConfig> {
        Self::apply_workspace_only(base, workspace_root)
    }

    pub fn merge_thread_resume(
        base: &AgentConfig,
        workspace_root: Option<String>,
    ) -> Result<AgentConfig> {
        Self::apply_workspace_only(base, workspace_root)
    }

    pub fn merge_turn_start(
        base: &AgentConfig,
        workspace_root: Option<String>,
    ) -> Result<AgentConfig> {
        Self::apply_workspace_only(base, workspace_root)
    }

    pub fn merge_events_replay(
        base: &AgentConfig,
        workspace_root: Option<String>,
    ) -> Result<AgentConfig> {
        Self::apply_workspace_only(base, workspace_root)
    }

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

    pub fn apply_turn_context(
        snapshot: &SessionSnapshot,
        overrides: TurnContextOverrides,
    ) -> Result<SessionSnapshot> {
        let mut snapshot = snapshot.clone();

        if let Some(raw_cwd) = overrides.cwd.as_deref() {
            snapshot.cwd = canonicalize_from_root(&snapshot.workspace_root, raw_cwd)?;
        }

        Ok(snapshot)
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
        bail!(AppServerError::InvalidRequest(
            "cwd must stay within workspace_root".into()
        ));
    }

    Ok(candidate)
}
