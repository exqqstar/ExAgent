use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionResponse, EventsReplayParams, EventsReplayResponse,
    EventsSubscribeParams, ThreadGoal, ThreadGoalClearParams, ThreadGoalClearResponse,
    ThreadGoalGetParams, ThreadGoalGetResponse, ThreadGoalSetParams, ThreadGoalSetResponse,
    ThreadGoalStatus, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, TurnInterruptParams,
    TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use crate::app_server::AppServerService;
use crate::events::RuntimeEvent;
use crate::index_db::{
    IndexDb, ProjectRecord, ProjectUpsert, ThreadGoalRecord, ThreadGoalStatusRecord,
    ThreadListFilter, ThreadRecord,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct NewProjectRequest {
    pub name: String,
    pub path: PathBuf,
}

pub struct DesktopFacade {
    service: AppServerService,
    index: IndexDb,
}

impl DesktopFacade {
    pub fn new(service: AppServerService, index: IndexDb) -> Self {
        Self { service, index }
    }

    pub async fn add_project(&self, request: NewProjectRequest) -> Result<ProjectRecord> {
        let project = self
            .index
            .upsert_project(ProjectUpsert {
                name: request.name,
                path: request.path,
            })
            .await?;
        self.index
            .reindex_project(&project.id, &project.path)
            .await?;
        Ok(project)
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        self.index.list_projects().await
    }

    pub async fn rename_project(&self, project_id: &str, name: &str) -> Result<ProjectRecord> {
        if name.trim().is_empty() {
            return Err(anyhow!("project name cannot be empty"));
        }
        self.index.rename_project(project_id, name).await?;
        self.index.project_by_id(project_id).await
    }

    pub async fn set_project_pinned(
        &self,
        project_id: &str,
        pinned: bool,
    ) -> Result<ProjectRecord> {
        self.index.set_project_pinned(project_id, pinned).await?;
        self.index.project_by_id(project_id).await
    }

    pub async fn archive_project(&self, project_id: &str) -> Result<()> {
        self.index.archive_project(project_id).await
    }

    pub async fn remove_project(&self, project_id: &str) -> Result<()> {
        self.index.remove_project(project_id).await
    }

    pub async fn archive_project_threads(&self, project_id: &str) -> Result<()> {
        self.index.archive_project_threads(project_id).await
    }

    pub async fn create_project_worktree(&self, project_id: &str) -> Result<ProjectRecord> {
        let project = self.index.project_by_id(project_id).await?;
        let root = git_root(&project.path).await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let slug = slugify(&project.name);
        let parent = root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(".exagent"));
        let worktree_path = parent.join(".worktrees").join(format!("{slug}-{now}"));
        let branch = format!("codex/{slug}-worktree-{now}");

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch)
            .arg(&worktree_path)
            .output()
            .await?;
        if !output.status.success() {
            return Err(anyhow!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        self.add_project(NewProjectRequest {
            name: format!("{} Worktree", project.name),
            path: worktree_path,
        })
        .await
    }

    pub async fn list_threads(&self, filter: ThreadListFilter) -> Result<Vec<ThreadRecord>> {
        self.index.list_threads(filter).await
    }

    pub async fn reindex_project(&self, project_id: &str) -> Result<Vec<ThreadRecord>> {
        let project = self.index.project_by_id(project_id).await?;
        self.index
            .reindex_project(project_id, &project.path)
            .await?;
        self.index
            .list_threads(ThreadListFilter {
                project_id: project_id.to_string(),
                include_archived: false,
                search: None,
            })
            .await
    }

    pub async fn start_thread(&self, project_id: &str) -> Result<ThreadStartResponse> {
        let project = self.index.project_by_id(project_id).await?;
        let response = self.service.thread_start(ThreadStartParams {
            workspace_root: Some(project.path.display().to_string()),
            cwd: Some(project.path.display().to_string()),
            permission_profile: None,
        })?;
        self.index
            .reindex_project(&project.id, &project.path)
            .await?;
        Ok(response)
    }

    pub async fn read_thread(
        &self,
        project_id: &str,
        params: ThreadReadParams,
    ) -> Result<ThreadReadResponse> {
        let project = self.index.project_by_id(project_id).await?;
        let mut response = self.service.thread_read(ThreadReadParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })?;
        self.hydrate_thread_goal(&mut response.thread).await?;
        Ok(response)
    }

    pub async fn resume_thread(
        &self,
        project_id: &str,
        params: ThreadResumeParams,
    ) -> Result<ThreadResumeResponse> {
        let project = self.index.project_by_id(project_id).await?;
        let mut response = self.service.thread_resume(ThreadResumeParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })?;
        self.hydrate_thread_goal(&mut response.thread).await?;
        Ok(response)
    }

    pub async fn agent_tree(
        &self,
        project_id: &str,
        params: crate::app_server::protocol::AgentTreeParams,
    ) -> Result<crate::app_server::protocol::AgentTreeResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service
            .agent_tree(crate::app_server::protocol::AgentTreeParams {
                workspace_root: Some(project.path.display().to_string()),
                ..params
            })
    }

    pub async fn thread_goal_set(
        &self,
        project_id: &str,
        mut params: ThreadGoalSetParams,
    ) -> Result<ThreadGoalSetResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(crate::app_server::protocol::BoundaryOp::ThreadGoalSet(
                params,
            ))
            .await?
        {
            crate::app_server::protocol::BoundaryOpResponse::ThreadGoalSet(response) => {
                Ok(response)
            }
            _ => Err(anyhow!("thread goal set returned unexpected response")),
        }
    }

    pub async fn thread_goal_get(
        &self,
        project_id: &str,
        mut params: ThreadGoalGetParams,
    ) -> Result<ThreadGoalGetResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(crate::app_server::protocol::BoundaryOp::ThreadGoalGet(
                params,
            ))
            .await?
        {
            crate::app_server::protocol::BoundaryOpResponse::ThreadGoalGet(response) => {
                Ok(response)
            }
            _ => Err(anyhow!("thread goal get returned unexpected response")),
        }
    }

    pub async fn thread_goal_clear(
        &self,
        project_id: &str,
        mut params: ThreadGoalClearParams,
    ) -> Result<ThreadGoalClearResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(crate::app_server::protocol::BoundaryOp::ThreadGoalClear(
                params,
            ))
            .await?
        {
            crate::app_server::protocol::BoundaryOpResponse::ThreadGoalCleared(response) => {
                Ok(response)
            }
            _ => Err(anyhow!("thread goal clear returned unexpected response")),
        }
    }

    async fn hydrate_thread_goal(
        &self,
        thread: &mut crate::app_server::protocol::ThreadView,
    ) -> Result<()> {
        thread.goal = self
            .index
            .get_thread_goal(&thread.id)
            .await?
            .map(thread_goal_from_record);
        Ok(())
    }

    pub async fn start_turn(
        &self,
        project_id: &str,
        params: TurnStartParams,
    ) -> Result<TurnStartResponse> {
        let project = self.index.project_by_id(project_id).await?;
        let response = self
            .service
            .turn_start(TurnStartParams {
                workspace_root: Some(project.path.display().to_string()),
                ..params
            })
            .await?;
        self.index
            .reindex_project(&project.id, &project.path)
            .await?;
        Ok(response)
    }

    pub async fn interrupt_turn(
        &self,
        project_id: &str,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service
            .turn_interrupt(TurnInterruptParams {
                workspace_root: Some(project.path.display().to_string()),
                ..params
            })
            .await
    }

    pub async fn approval_decision(
        &self,
        project_id: &str,
        params: ApprovalDecisionParams,
    ) -> Result<ApprovalDecisionResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service
            .approval_decision(ApprovalDecisionParams {
                workspace_root: Some(project.path.display().to_string()),
                ..params
            })
            .await
    }

    pub async fn events_replay(
        &self,
        project_id: &str,
        params: EventsReplayParams,
    ) -> Result<EventsReplayResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service.events_replay(EventsReplayParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })
    }

    pub async fn events_subscribe(
        &self,
        project_id: &str,
        params: EventsSubscribeParams,
    ) -> Result<broadcast::Receiver<RuntimeEvent>> {
        let project = self.index.project_by_id(project_id).await?;
        self.service.events_subscribe(EventsSubscribeParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })
    }

    pub async fn rename_thread(
        &self,
        thread_id: &crate::types::ThreadId,
        title: &str,
    ) -> Result<()> {
        if title.trim().is_empty() {
            return Err(anyhow!("thread title cannot be empty"));
        }
        self.index.rename_thread(thread_id, title).await
    }

    pub async fn set_thread_pinned(
        &self,
        thread_id: &crate::types::ThreadId,
        pinned: bool,
    ) -> Result<()> {
        self.index.set_thread_pinned(thread_id, pinned).await
    }

    pub async fn archive_thread(&self, thread_id: &crate::types::ThreadId) -> Result<()> {
        self.index.archive_thread(thread_id).await
    }

    pub async fn unarchive_thread(&self, thread_id: &crate::types::ThreadId) -> Result<()> {
        self.index.unarchive_thread(thread_id).await
    }
}

fn thread_goal_from_record(record: ThreadGoalRecord) -> ThreadGoal {
    ThreadGoal {
        thread_id: record.thread_id,
        goal_id: record.goal_id,
        objective: record.objective,
        status: match record.status {
            ThreadGoalStatusRecord::Active => ThreadGoalStatus::Active,
            ThreadGoalStatusRecord::Paused => ThreadGoalStatus::Paused,
            ThreadGoalStatusRecord::Blocked => ThreadGoalStatus::Blocked,
            ThreadGoalStatusRecord::UsageLimited => ThreadGoalStatus::UsageLimited,
            ThreadGoalStatusRecord::BudgetLimited => ThreadGoalStatus::BudgetLimited,
            ThreadGoalStatusRecord::Complete => ThreadGoalStatus::Complete,
        },
        token_budget: record.token_budget,
        tokens_used: record.tokens_used,
        time_used_seconds: record.time_used_seconds,
        continuation_suppressed: record.continuation_suppressed,
        continuation_suppressed_after_turn_id: record.continuation_suppressed_after_turn_id,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

async fn git_root(path: &std::path::Path) -> Result<PathBuf> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!(
            "project is not inside a git repository: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn slugify(name: &str) -> String {
    let slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "project".into()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn git_root_rejects_non_git_project_paths() {
        let dir = tempfile::tempdir().unwrap();

        let error = git_root(dir.path()).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("project is not inside a git repository"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn slugify_generates_stable_ascii_slug() {
        assert_eq!(slugify("ExAgent Desktop"), "exagent-desktop");
        assert_eq!(slugify("  项目 管理  "), "project");
    }
}
