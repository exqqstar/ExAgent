use anyhow::Result;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    ApprovalsListParams, ApprovalsListResponse, PendingApprovalItem, PendingApprovalKind,
};
use crate::app_server::services::AppServerServices;
use crate::index_db::ThreadGoalStatusRecord;
use crate::policy::{PendingApprovalDetail, PendingApprovalSummary};
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

    Ok(ApprovalsListResponse { approvals })
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
