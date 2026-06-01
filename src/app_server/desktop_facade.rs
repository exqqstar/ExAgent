use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::app_server::protocol::{
    ApprovalDecisionParams, ApprovalDecisionResponse, EventsReplayParams, EventsReplayResponse,
    EventsSubscribeParams, ThreadReadParams, ThreadReadResponse, ThreadResumeParams,
    ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, TurnInterruptParams,
    TurnInterruptResponse, TurnStartParams, TurnStartResponse,
};
use crate::app_server::AppServerService;
use crate::events::RuntimeEvent;
use crate::index_db::{IndexDb, ProjectRecord, ProjectUpsert, ThreadListFilter, ThreadRecord};

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
        self.service.thread_read(ThreadReadParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })
    }

    pub async fn resume_thread(
        &self,
        project_id: &str,
        params: ThreadResumeParams,
    ) -> Result<ThreadResumeResponse> {
        let project = self.index.project_by_id(project_id).await?;
        self.service.thread_resume(ThreadResumeParams {
            workspace_root: Some(project.path.display().to_string()),
            ..params
        })
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
