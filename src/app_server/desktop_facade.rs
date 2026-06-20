use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionResponse, ApprovalsListParams, ApprovalsListResponse,
    BoundaryOp, BoundaryOpResponse, CheckpointRestoreParams, CheckpointRestoreResponse,
    EventsReplayParams, EventsReplayResponse, EventsSubscribeParams, MemoryAuditParams,
    MemoryAuditResponse, MemoryForgetParams, MemoryForgetResponse, MemoryListArchivedParams,
    MemoryListArchivedResponse, MemoryListCandidatesParams, MemoryListCandidatesResponse,
    MemoryPromoteParams, MemoryPromoteResponse, MemorySaveParams, MemorySaveResponse,
    MemorySearchParams, MemorySearchResponse, MemoryUpdateParams, MemoryUpdateResponse,
    OpenQuestionResolveParams, OpenQuestionResolveResponse, SubmitUserInputParams,
    SubmitUserInputResponse, ThreadCompactParams, ThreadCompactResponse, ThreadForkParams,
    ThreadForkResponse, ThreadGoal, ThreadGoalClearParams, ThreadGoalClearResponse,
    ThreadGoalGetParams, ThreadGoalGetResponse, ThreadGoalMode, ThreadGoalSetParams,
    ThreadGoalSetResponse, ThreadGoalStatus, ThreadReadParams, ThreadReadResponse,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartParams, ThreadStartResponse,
    TurnInterruptParams, TurnInterruptResponse, TurnStartParams, TurnStartResponse,
    WorkflowStartParams, WorkflowStartResponse,
};
use crate::app_server::AppServerService;
use crate::events::RuntimeEvent;
use crate::index_db::{
    IndexDb, ProjectRecord, ProjectUpsert, ThreadGoalRecord, ThreadGoalStatusRecord,
    ThreadListFilter, ThreadRecord,
};
use crate::state::fork_edges::ThreadForkEdgeStore;
use crate::types::{ThreadId, TurnId};

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
        let project_id = filter.project_id.clone();
        let threads = self.index.list_threads(filter).await?;
        if threads.is_empty() {
            return Ok(threads);
        }
        let project = self.index.project_by_id(&project_id).await?;
        hydrate_thread_lineage(&project.path, threads)
    }

    pub async fn reindex_project(&self, project_id: &str) -> Result<Vec<ThreadRecord>> {
        let project = self.index.project_by_id(project_id).await?;
        self.index
            .reindex_project(project_id, &project.path)
            .await?;
        let threads = self
            .index
            .list_threads(ThreadListFilter {
                project_id: project_id.to_string(),
                include_archived: false,
                search: None,
            })
            .await?;
        hydrate_thread_lineage(&project.path, threads)
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

    pub async fn start_workflow(
        &self,
        project_id: &str,
        mut params: WorkflowStartParams,
    ) -> Result<WorkflowStartResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        params.cwd = Some(project.path.display().to_string());
        let response = self.service.workflow_start(params).await?;
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

    pub async fn fork_thread(
        &self,
        project_id: &str,
        mut params: ThreadForkParams,
    ) -> Result<ThreadForkResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        let response = self.service.thread_fork(params).await?;
        self.index
            .reindex_project(&project.id, &project.path)
            .await?;
        Ok(response)
    }

    pub async fn compact_thread(
        &self,
        project_id: &str,
        params: ThreadCompactParams,
    ) -> Result<ThreadCompactResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service
            .thread_compact(ThreadCompactParams {
                workspace_root: Some(project.path.display().to_string()),
                ..params
            })
            .await
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
            .await
    }

    pub async fn approvals_list(
        &self,
        project_id: &str,
        mut params: ApprovalsListParams,
    ) -> Result<ApprovalsListResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::ApprovalsList(params))
            .await?
        {
            BoundaryOpResponse::ApprovalsList(response) => Ok(response),
            _ => Err(anyhow!("approvals list returned unexpected response")),
        }
    }

    pub async fn checkpoint_restore(
        &self,
        project_id: &str,
        mut params: CheckpointRestoreParams,
    ) -> Result<CheckpointRestoreResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = project.path.display().to_string();
        match self
            .service
            .submit_boundary_op(BoundaryOp::CheckpointRestore(params))
            .await?
        {
            BoundaryOpResponse::CheckpointRestored(response) => Ok(response),
            _ => Err(anyhow!("checkpoint restore returned unexpected response")),
        }
    }

    pub async fn open_question_resolve(
        &self,
        project_id: &str,
        mut params: OpenQuestionResolveParams,
    ) -> Result<OpenQuestionResolveResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::OpenQuestionResolve(params))
            .await?
        {
            BoundaryOpResponse::OpenQuestionResolved(response) => Ok(response),
            _ => Err(anyhow!(
                "open question resolve returned unexpected response"
            )),
        }
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
        thread.goal_mode = match thread.goal.as_ref() {
            Some(goal) => {
                crate::runtime::forge::goal_modes::ForgeGoalModeStore::new(self.index.clone())
                    .mode_for_goal(&thread.id, &goal.goal_id)
                    .await?
            }
            None => ThreadGoalMode::Standard,
        };
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

    pub async fn submit_user_input(
        &self,
        project_id: &str,
        params: SubmitUserInputParams,
    ) -> Result<SubmitUserInputResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service
            .submit_user_input(SubmitUserInputParams {
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

    pub async fn memory_search(
        &self,
        project_id: &str,
        scope: Option<String>,
        query: &str,
        limit: usize,
    ) -> Result<MemorySearchResponse> {
        let project = self.index.project_by_id(project_id).await?;
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemorySearch(MemorySearchParams {
                workspace_root: Some(project.path.display().to_string()),
                scope,
                query: query.to_string(),
                limit,
            }))
            .await?
        {
            BoundaryOpResponse::MemorySearched(response) => Ok(response),
            _ => Err(anyhow!("memory search returned unexpected response")),
        }
    }

    pub async fn memory_save(
        &self,
        project_id: &str,
        mut params: MemorySaveParams,
    ) -> Result<MemorySaveResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemorySave(params))
            .await?
        {
            BoundaryOpResponse::MemorySaved(response) => Ok(response),
            _ => Err(anyhow!("memory save returned unexpected response")),
        }
    }

    pub async fn memory_update(
        &self,
        project_id: &str,
        mut params: MemoryUpdateParams,
    ) -> Result<MemoryUpdateResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryUpdate(params))
            .await?
        {
            BoundaryOpResponse::MemoryUpdated(response) => Ok(response),
            _ => Err(anyhow!("memory update returned unexpected response")),
        }
    }

    pub async fn memory_forget(
        &self,
        project_id: &str,
        mut params: MemoryForgetParams,
    ) -> Result<MemoryForgetResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryForget(params))
            .await?
        {
            BoundaryOpResponse::MemoryForgotten(response) => Ok(response),
            _ => Err(anyhow!("memory forget returned unexpected response")),
        }
    }

    pub async fn memory_audit(
        &self,
        project_id: &str,
        mut params: MemoryAuditParams,
    ) -> Result<MemoryAuditResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryAudit(params))
            .await?
        {
            BoundaryOpResponse::MemoryAudit(response) => Ok(response),
            _ => Err(anyhow!("memory audit returned unexpected response")),
        }
    }

    pub async fn memory_list_candidates(
        &self,
        project_id: &str,
        mut params: MemoryListCandidatesParams,
    ) -> Result<MemoryListCandidatesResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryListCandidates(params))
            .await?
        {
            BoundaryOpResponse::MemoryCandidatesListed(response) => Ok(response),
            _ => Err(anyhow!(
                "memory list candidates returned unexpected response"
            )),
        }
    }

    pub async fn memory_list_archived(
        &self,
        project_id: &str,
        mut params: MemoryListArchivedParams,
    ) -> Result<MemoryListArchivedResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryListArchived(params))
            .await?
        {
            BoundaryOpResponse::MemoryArchivedListed(response) => Ok(response),
            _ => Err(anyhow!("memory list archived returned unexpected response")),
        }
    }

    pub async fn memory_promote(
        &self,
        project_id: &str,
        mut params: MemoryPromoteParams,
    ) -> Result<MemoryPromoteResponse> {
        let project = self.index.project_by_id(project_id).await?;
        params.workspace_root = Some(project.path.display().to_string());
        match self
            .service
            .submit_boundary_op(BoundaryOp::MemoryPromote(params))
            .await?
        {
            BoundaryOpResponse::MemoryPromoted(response) => Ok(response),
            _ => Err(anyhow!("memory promote returned unexpected response")),
        }
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

fn hydrate_thread_lineage(
    project_path: &Path,
    mut threads: Vec<ThreadRecord>,
) -> Result<Vec<ThreadRecord>> {
    let fork_edges =
        match ThreadForkEdgeStore::for_workspace(project_path).list_for_workspace_blocking() {
            Ok(edges) => edges,
            Err(error) => {
                tracing::warn!(
                    workspace = %project_path.display(),
                    error = %error,
                    "failed to hydrate thread fork lineage"
                );
                return Ok(threads);
            }
        };
    let edges_by_child: HashMap<ThreadId, (ThreadId, TurnId)> = fork_edges
        .into_iter()
        .map(|edge| {
            (
                edge.child_thread_id,
                (edge.parent_thread_id, edge.fork_point_turn_id),
            )
        })
        .collect();

    for thread in &mut threads {
        if let Some((parent_thread_id, fork_point_turn_id)) = edges_by_child.get(&thread.id) {
            thread.fork_parent_thread_id = Some(parent_thread_id.clone());
            thread.fork_point_turn_id = Some(fork_point_turn_id.clone());
        }
    }

    Ok(threads)
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
