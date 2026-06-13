use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use sqlx::Row;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    ApprovalsListParams, ApprovalsListResponse, OpenQuestionResolveParams,
    OpenQuestionResolveResponse, OpenQuestionResolveStatus, PendingApprovalItem,
    PendingApprovalKind,
};
use crate::app_server::services::AppServerServices;
use crate::events::{RuntimeEvent, RuntimeEventKind};
use crate::index_db::ThreadGoalStatusRecord;
use crate::policy::{PendingApprovalDetail, PendingApprovalSummary};
use crate::runtime::forge::open_questions::{OpenQuestion, OpenQuestionStore};
use crate::session::ApprovalId;
use crate::state::rollout::{RolloutItem, RolloutStore};
use crate::types::ThreadId;

pub(in crate::app_server) async fn approvals_list(
    services: &AppServerServices,
    params: ApprovalsListParams,
) -> Result<ApprovalsListResponse> {
    let workspace_root = match params.workspace_root {
        Some(workspace_root) => Some(
            OverridePolicy::merge_thread_read(&services.base_config, Some(workspace_root))?
                .workspace_root,
        ),
        None => None,
    };

    let mut approvals = Vec::new();
    for summary in services.policy.list_pending().await {
        let Some(runtime) = services.runtime_loader.runtime_for(&summary.thread_id) else {
            continue;
        };
        let runtime_workspace_root = runtime.live_view().snapshot.workspace_root;
        if let Some(workspace_root) = workspace_root.as_ref() {
            if runtime_workspace_root != *workspace_root {
                continue;
            }
        }

        let goal_id = active_goal_id(services, &summary.thread_id).await?;
        approvals.push(pending_item_from_summary(summary, goal_id));
    }
    if let Some(goal_store) = services.goal_store.as_ref() {
        let question_store = OpenQuestionStore::new(goal_store.clone());
        for question in question_store
            .unresolved_for_workspace(workspace_root.as_deref())
            .await?
        {
            approvals.push(pending_item_from_open_question(question));
        }
    }

    Ok(ApprovalsListResponse { approvals })
}

pub(in crate::app_server) async fn open_question_resolve(
    services: &AppServerServices,
    params: OpenQuestionResolveParams,
) -> Result<OpenQuestionResolveResponse> {
    let workspace_root = match params.workspace_root {
        Some(workspace_root) => Some(
            OverridePolicy::merge_thread_read(&services.base_config, Some(workspace_root))?
                .workspace_root,
        ),
        None => None,
    };
    let Some(goal_store) = services.goal_store.as_ref() else {
        bail!("open question resolve requires a goal store");
    };
    let question_store = OpenQuestionStore::new(goal_store.clone());
    let question = question_store
        .get_question(&params.question_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown open question: {}", params.question_id))?;
    if question.thread_id != params.thread_id {
        bail!(
            "open question {} belongs to thread {}, not {}",
            params.question_id,
            question.thread_id.as_str(),
            params.thread_id.as_str()
        );
    }
    let rollout_path =
        thread_rollout_path(goal_store, &params.thread_id, workspace_root.as_deref())
            .await?
            .ok_or_else(|| anyhow::anyhow!("thread not indexed: {}", params.thread_id.as_str()))?;
    let resolved = question_store
        .resolve_question(&params.question_id, params.answer)
        .await?;
    append_open_question_resolved_event(&rollout_path, &resolved).await?;
    Ok(OpenQuestionResolveResponse {
        thread_id: resolved.thread_id,
        question_id: resolved.question_id,
        goal_id: resolved.goal_id,
        status: OpenQuestionResolveStatus::Resolved,
    })
}

async fn active_goal_id(
    services: &AppServerServices,
    thread_id: &ThreadId,
) -> Result<Option<String>> {
    let Some(goal_store) = services.goal_store.as_ref() else {
        return Ok(None);
    };
    Ok(goal_store
        .get_thread_goal(thread_id)
        .await?
        .filter(|goal| goal.status == ThreadGoalStatusRecord::Active)
        .map(|goal| goal.goal_id))
}

fn pending_item_from_summary(
    summary: PendingApprovalSummary,
    goal_id: Option<String>,
) -> PendingApprovalItem {
    match summary.detail {
        PendingApprovalDetail::Command {
            tool_name, command, ..
        } => PendingApprovalItem {
            thread_id: summary.thread_id,
            approval_id: summary.approval_id,
            kind: PendingApprovalKind::Command,
            summary: format!("{tool_name}: {command}"),
            detail: command,
            goal_id,
            requested_at_ms: summary.requested_at_ms,
            checkpoint_id: summary.checkpoint_id,
        },
    }
}

fn pending_item_from_open_question(question: OpenQuestion) -> PendingApprovalItem {
    PendingApprovalItem {
        thread_id: question.thread_id,
        approval_id: ApprovalId::new(question.question_id),
        kind: PendingApprovalKind::OpenQuestion,
        summary: question.question,
        detail: format!(
            "Blocks: {}\n\n{}",
            question.blocks_what,
            question.answer.unwrap_or_default()
        ),
        goal_id: Some(question.goal_id),
        requested_at_ms: question.created_at_ms.max(0) as u64,
        checkpoint_id: None,
    }
}

async fn thread_rollout_path(
    db: &crate::index_db::IndexDb,
    thread_id: &ThreadId,
    expected_workspace_root: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let row = sqlx::query(
        r#"
SELECT t.rollout_path, p.path AS workspace_path
FROM threads t
JOIN projects p ON p.id = t.project_id
WHERE t.id = ?
        "#,
    )
    .bind(thread_id.as_str())
    .fetch_optional(db.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let workspace_path: String = row.try_get("workspace_path")?;
    if let Some(expected_workspace_root) = expected_workspace_root {
        let expected = expected_workspace_root.display().to_string();
        if workspace_path != expected {
            bail!(
                "thread {} belongs to workspace {}, not {}",
                thread_id.as_str(),
                workspace_path,
                expected
            );
        }
    }
    Ok(Some(PathBuf::from(
        row.try_get::<String, _>("rollout_path")?,
    )))
}

async fn append_open_question_resolved_event(
    rollout_path: &Path,
    question: &OpenQuestion,
) -> Result<()> {
    RolloutStore::new(rollout_path.to_path_buf())
        .append_items(&[RolloutItem::EventMsg(RuntimeEvent {
            event_id: crate::policy::new_policy_event_id(),
            thread_id: question.thread_id.clone(),
            turn_id: None,
            kind: RuntimeEventKind::OpenQuestionResolved {
                question_id: question.question_id.clone(),
                goal_id: question.goal_id.clone(),
                answer: question.answer.clone(),
            },
        })])
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::app_server::protocol::{
        OpenQuestionResolveParams, OpenQuestionResolveStatus, PendingApprovalKind,
    };
    use crate::config::AgentConfig;
    use crate::index_db::{IndexDb, ProjectUpsert};
    use crate::resolver::EnvModelResolver;
    use crate::runtime::forge::open_questions::OpenQuestionStore;
    use crate::state::rollout::{rollout_paths, RolloutItem, RolloutStore};

    async fn services_with_thread(
        thread_id: &ThreadId,
    ) -> (tempfile::TempDir, AppServerServices, IndexDb, String) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let project = db
            .upsert_project(ProjectUpsert {
                name: "Forge".into(),
                path: workspace.clone(),
            })
            .await
            .unwrap();
        let paths = rollout_paths(&workspace, thread_id);
        RolloutStore::new(paths.rollout_path.clone())
            .append_items(
                &[RolloutItem::ThreadMeta(crate::state::rollout::ThreadMeta {
                    thread_id: thread_id.clone(),
                    workspace_root: workspace.clone(),
                    initial_cwd: workspace.clone(),
                    permission_profile: Default::default(),
                    thread_source: Default::default(),
                    lineage: None,
                    created_at: "2026-06-13T00:00:00Z".to_string(),
                })],
            )
            .await
            .unwrap();
        sqlx::query(
            r#"
INSERT INTO threads (
  id, project_id, rollout_path, fallback_title, preview, title_source,
  pinned, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'test', 0, 'idle', 1, 1)
            "#,
        )
        .bind(thread_id.as_str())
        .bind(project.id)
        .bind(paths.rollout_path.display().to_string())
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
        let goal = db
            .insert_thread_goal(thread_id, "answer forge question", None)
            .await
            .unwrap()
            .unwrap();
        let services = AppServerServices::with_model_resolver(
            AgentConfig {
                workspace_root: workspace.clone(),
                cwd: workspace.clone(),
                forge_review_gate_enabled: true,
                ..AgentConfig::default()
            },
            Arc::new(EnvModelResolver),
        )
        .with_goal_store(db.clone());
        (dir, services, db, goal.goal_id)
    }

    #[tokio::test]
    async fn approvals_list_includes_open_questions_and_resolve_records_event() {
        let thread_id = ThreadId::new("thread_open_question_inbox");
        let (_dir, services, db, goal_id) = services_with_thread(&thread_id).await;
        let question_store = OpenQuestionStore::new(db.clone());
        let question = question_store
            .record_question(
                thread_id.clone(),
                goal_id.clone(),
                "Which cohort ships first?",
                "Release targeting",
            )
            .await
            .unwrap();

        let listed = approvals_list(
            &services,
            ApprovalsListParams {
                workspace_root: Some(services.base_config.workspace_root.display().to_string()),
            },
        )
        .await
        .unwrap();

        assert_eq!(listed.approvals.len(), 1);
        let item = &listed.approvals[0];
        assert_eq!(item.kind, PendingApprovalKind::OpenQuestion);
        assert_eq!(item.thread_id, thread_id);
        assert_eq!(item.approval_id.as_str(), question.question_id);
        assert_eq!(item.summary, "Which cohort ships first?");
        assert!(item.detail.contains("Release targeting"));
        assert_eq!(item.goal_id.as_deref(), Some(goal_id.as_str()));
        assert!(item.checkpoint_id.is_none());

        let response = open_question_resolve(
            &services,
            OpenQuestionResolveParams {
                thread_id: thread_id.clone(),
                question_id: question.question_id.clone(),
                answer: Some("Beta cohort first".to_string()),
                workspace_root: Some(services.base_config.workspace_root.display().to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(response.status, OpenQuestionResolveStatus::Resolved);
        assert_eq!(response.goal_id, goal_id);
        assert!(approvals_list(
            &services,
            ApprovalsListParams {
                workspace_root: Some(services.base_config.workspace_root.display().to_string()),
            },
        )
        .await
        .unwrap()
        .approvals
        .is_empty());

        let paths = rollout_paths(&services.base_config.workspace_root, &thread_id);
        let items = RolloutStore::read_items(&paths.rollout_path).await.unwrap();
        assert!(items.iter().any(|item| matches!(
            item,
            RolloutItem::EventMsg(event)
                if matches!(
                    &event.kind,
                    crate::events::RuntimeEventKind::OpenQuestionResolved { question_id, answer, .. }
                        if question_id == &question.question_id
                            && answer.as_deref() == Some("Beta cohort first")
                )
        )));
    }
}
