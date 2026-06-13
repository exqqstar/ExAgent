use anyhow::Result;

use crate::app_server::protocol::{
    validate_thread_goal_objective, ThreadGoal, ThreadGoalClearParams, ThreadGoalClearResponse,
    ThreadGoalGetParams, ThreadGoalGetResponse, ThreadGoalMode, ThreadGoalSetParams,
    ThreadGoalSetResponse, ThreadGoalStatus,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::AppServerError;
use crate::index_db::{GoalUpdate, ThreadGoalRecord, ThreadGoalStatusRecord};
use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
use crate::runtime::goal::runtime::GoalRuntimeEvent;

pub(in crate::app_server) async fn thread_goal_set(
    services: &AppServerServices,
    params: ThreadGoalSetParams,
) -> Result<ThreadGoalSetResponse> {
    let db = goal_store(services)?;
    let current = db.get_thread_goal(&params.thread_id).await?;
    let creating_new_goal = current.is_none();
    if let Some(objective) = params.objective.as_ref() {
        validate_thread_goal_objective(objective).map_err(AppServerError::InvalidRequest)?;
    }
    if let Some(Some(token_budget)) = params.token_budget {
        if token_budget <= 0 {
            return Err(AppServerError::InvalidRequest(
                "token_budget must be positive".to_string(),
            )
            .into());
        }
    }
    if let Some(status) = params.status {
        validate_external_goal_status(status)?;
    }
    let requested_mode = params.mode;
    let mode_store = ForgeGoalModeStore::new(db.clone());
    let previous_goal_id = current.as_ref().map(|goal| goal.goal_id.clone());
    let previous_mode = match previous_goal_id.as_deref() {
        Some(goal_id) => mode_store.mode_for_goal(&params.thread_id, goal_id).await?,
        None => ThreadGoalMode::Standard,
    };

    account_before_external_mutation(services, &params.thread_id).await?;
    let previous_goal = current.clone().map(thread_goal_from_record);

    let goal = match current {
        None => {
            let objective = params.objective.ok_or_else(|| {
                AppServerError::InvalidRequest(
                    "objective is required when creating a thread goal".to_string(),
                )
            })?;
            let token_budget = params.token_budget.flatten();
            let status = params
                .status
                .map(status_record_from_protocol)
                .unwrap_or(ThreadGoalStatusRecord::Active);
            db.replace_thread_goal(&params.thread_id, &objective, status, token_budget)
                .await?
        }
        Some(current) => {
            let update = GoalUpdate {
                objective: params.objective,
                status: params.status.map(status_record_from_protocol),
                token_budget: params.token_budget,
                expected_goal_id: Some(current.goal_id),
            };
            db.update_thread_goal(&params.thread_id, update)
                .await?
                .ok_or_else(|| {
                    AppServerError::InvalidRequest(
                        "thread goal changed while applying update".to_string(),
                    )
                })?
        }
    };

    let goal = thread_goal_from_record(goal);
    let mode = if creating_new_goal {
        let mode = requested_mode.unwrap_or_default();
        mode_store
            .replace_for_thread_goal(&params.thread_id, &goal.goal_id, mode)
            .await?;
        mode
    } else if let Some(mode) = requested_mode {
        mode_store
            .set_mode(&params.thread_id, &goal.goal_id, mode)
            .await?;
        mode
    } else {
        mode_store
            .mode_for_goal(&params.thread_id, &goal.goal_id)
            .await?
    };
    let mode_changed =
        previous_goal_id.as_deref() != Some(goal.goal_id.as_str()) || previous_mode != mode;
    if let Some(runtime) = services.runtime_loader.runtime_for(&params.thread_id) {
        let effect = runtime
            .apply_goal_runtime_event(GoalRuntimeEvent::ExternalSet {
                thread_id: &params.thread_id,
                goal: goal.clone(),
                previous_goal,
            })
            .await?;
        runtime.enqueue_goal_runtime_effect(effect).await?;
        if mode_changed {
            runtime
                .enqueue_goal_runtime_effect(
                    crate::runtime::goal::runtime::GoalRuntimeEffect::EmitModeUpdated {
                        thread_id: params.thread_id.clone(),
                        goal_id: goal.goal_id.clone(),
                        mode,
                    },
                )
                .await?;
        }
    }

    Ok(ThreadGoalSetResponse { goal, mode })
}

pub(in crate::app_server) async fn thread_goal_get(
    services: &AppServerServices,
    params: ThreadGoalGetParams,
) -> Result<ThreadGoalGetResponse> {
    let db = goal_store(services)?;
    let goal = db
        .get_thread_goal(&params.thread_id)
        .await?
        .map(thread_goal_from_record);
    let mode = match goal.as_ref() {
        Some(goal) => {
            ForgeGoalModeStore::new(db.clone())
                .mode_for_goal(&params.thread_id, &goal.goal_id)
                .await?
        }
        None => ThreadGoalMode::Standard,
    };
    Ok(ThreadGoalGetResponse { goal, mode })
}

pub(in crate::app_server) async fn thread_goal_clear(
    services: &AppServerServices,
    params: ThreadGoalClearParams,
) -> Result<ThreadGoalClearResponse> {
    let db = goal_store(services)?;
    account_before_external_mutation(services, &params.thread_id).await?;
    let cleared = db.delete_thread_goal(&params.thread_id).await?;
    ForgeGoalModeStore::new(db.clone())
        .clear_for_thread(&params.thread_id)
        .await?;
    if let Some(runtime) = services.runtime_loader.runtime_for(&params.thread_id) {
        let effect = runtime
            .apply_goal_runtime_event(GoalRuntimeEvent::ExternalClear {
                thread_id: &params.thread_id,
            })
            .await?;
        runtime.enqueue_goal_runtime_effect(effect).await?;
    }
    Ok(ThreadGoalClearResponse { cleared })
}

fn goal_store(services: &AppServerServices) -> Result<&crate::index_db::IndexDb> {
    services.goal_store.as_ref().ok_or_else(|| {
        AppServerError::InvalidRequest("thread goal store is not configured".to_string()).into()
    })
}

async fn account_before_external_mutation(
    services: &AppServerServices,
    thread_id: &crate::types::ThreadId,
) -> Result<()> {
    let Some(runtime) = services.runtime_loader.runtime_for(thread_id) else {
        return Ok(());
    };
    let active_turn = runtime.active_turn_id();
    let _ = runtime
        .apply_goal_runtime_event(GoalRuntimeEvent::ExternalMutationStarting {
            thread_id,
            turn_id: active_turn.as_ref(),
        })
        .await?;
    Ok(())
}

fn validate_external_goal_status(status: ThreadGoalStatus) -> Result<()> {
    if matches!(
        status,
        ThreadGoalStatus::UsageLimited | ThreadGoalStatus::BudgetLimited
    ) {
        return Err(AppServerError::InvalidRequest(
            "usage_limited and budget_limited are runtime-owned goal statuses".to_string(),
        )
        .into());
    }
    Ok(())
}

fn thread_goal_from_record(record: ThreadGoalRecord) -> ThreadGoal {
    ThreadGoal {
        thread_id: record.thread_id,
        goal_id: record.goal_id,
        objective: record.objective,
        status: status_protocol_from_record(record.status),
        token_budget: record.token_budget,
        tokens_used: record.tokens_used,
        time_used_seconds: record.time_used_seconds,
        continuation_suppressed: record.continuation_suppressed,
        continuation_suppressed_after_turn_id: record.continuation_suppressed_after_turn_id,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::app_server::protocol::{ThreadGoalClearParams, ThreadGoalMode, ThreadGoalSetParams};
    use crate::config::AgentConfig;
    use crate::index_db::{IndexDb, ProjectUpsert};
    use crate::resolver::EnvModelResolver;
    use crate::runtime::forge::goal_modes::ForgeGoalModeStore;
    use crate::types::ThreadId;

    use super::*;

    #[tokio::test]
    async fn clear_goal_removes_forge_goal_mode_without_loaded_runtime() {
        let thread_id = ThreadId::new("thread_goal_clear_mode_without_runtime");
        let (_dir, services, db, goal_id) = services_with_goal(&thread_id).await;
        let store = ForgeGoalModeStore::new(db.clone());
        store
            .set_intensive(&thread_id, &goal_id, true)
            .await
            .unwrap();

        let response = thread_goal_clear(
            &services,
            ThreadGoalClearParams {
                thread_id: thread_id.clone(),
                workspace_root: None,
            },
        )
        .await
        .unwrap();

        assert!(response.cleared);
        assert!(!store.is_intensive(&thread_id, &goal_id).await.unwrap());
    }

    #[tokio::test]
    async fn externally_created_goal_replaces_stale_forge_goal_mode() {
        let thread_id = ThreadId::new("thread_goal_set_replaces_mode_without_runtime");
        let (_dir, services, db, old_goal_id) = services_with_goal(&thread_id).await;
        let store = ForgeGoalModeStore::new(db.clone());
        store
            .set_intensive(&thread_id, &old_goal_id, true)
            .await
            .unwrap();
        db.delete_thread_goal(&thread_id).await.unwrap();

        let response = thread_goal_set(
            &services,
            ThreadGoalSetParams {
                thread_id: thread_id.clone(),
                workspace_root: None,
                objective: Some("external replacement".to_string()),
                status: None,
                token_budget: None,
                mode: None,
            },
        )
        .await
        .unwrap();

        assert_ne!(response.goal.goal_id, old_goal_id);
        assert!(!store.is_intensive(&thread_id, &old_goal_id).await.unwrap());
        assert!(!store
            .is_intensive(&thread_id, &response.goal.goal_id)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn externally_created_goal_persists_requested_mode() {
        let thread_id = ThreadId::new("thread_goal_set_reviewed_mode");
        let (_dir, services, db, _old_goal_id) = services_with_goal(&thread_id).await;
        db.delete_thread_goal(&thread_id).await.unwrap();

        let response = thread_goal_set(
            &services,
            ThreadGoalSetParams {
                thread_id: thread_id.clone(),
                workspace_root: None,
                objective: Some("ship reviewed goal".to_string()),
                status: None,
                token_budget: Some(None),
                mode: Some(ThreadGoalMode::Reviewed),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.mode, ThreadGoalMode::Reviewed);
        assert_eq!(
            ForgeGoalModeStore::new(db)
                .mode_for_goal(&thread_id, &response.goal.goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Reviewed
        );
    }

    #[tokio::test]
    async fn status_only_external_goal_update_preserves_mode() {
        let thread_id = ThreadId::new("thread_goal_status_preserves_mode");
        let (_dir, services, db, goal_id) = services_with_goal(&thread_id).await;
        let store = ForgeGoalModeStore::new(db.clone());
        store
            .set_mode(&thread_id, &goal_id, ThreadGoalMode::Intensive)
            .await
            .unwrap();

        let response = thread_goal_set(
            &services,
            ThreadGoalSetParams {
                thread_id: thread_id.clone(),
                workspace_root: None,
                objective: None,
                status: Some(ThreadGoalStatus::Paused),
                token_budget: None,
                mode: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.mode, ThreadGoalMode::Intensive);
        assert_eq!(
            store
                .mode_for_goal(&thread_id, &response.goal.goal_id)
                .await
                .unwrap(),
            ThreadGoalMode::Intensive
        );
    }

    async fn services_with_goal(
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
                name: "Goal Processor".into(),
                path: workspace.clone(),
            })
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
        .bind(workspace.join("rollout.jsonl").display().to_string())
        .bind("thread title")
        .bind("thread preview")
        .execute(db.pool())
        .await
        .unwrap();
        let goal = db
            .insert_thread_goal(thread_id, "clear stale mode", None)
            .await
            .unwrap()
            .unwrap();
        let services = AppServerServices::with_model_resolver(
            AgentConfig {
                workspace_root: workspace.clone(),
                cwd: workspace,
                ..AgentConfig::default()
            },
            Arc::new(EnvModelResolver),
        )
        .with_goal_store(db.clone());
        (dir, services, db, goal.goal_id)
    }
}

fn status_protocol_from_record(status: ThreadGoalStatusRecord) -> ThreadGoalStatus {
    match status {
        ThreadGoalStatusRecord::Active => ThreadGoalStatus::Active,
        ThreadGoalStatusRecord::Paused => ThreadGoalStatus::Paused,
        ThreadGoalStatusRecord::Blocked => ThreadGoalStatus::Blocked,
        ThreadGoalStatusRecord::UsageLimited => ThreadGoalStatus::UsageLimited,
        ThreadGoalStatusRecord::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        ThreadGoalStatusRecord::Complete => ThreadGoalStatus::Complete,
    }
}

fn status_record_from_protocol(status: ThreadGoalStatus) -> ThreadGoalStatusRecord {
    match status {
        ThreadGoalStatus::Active => ThreadGoalStatusRecord::Active,
        ThreadGoalStatus::Paused => ThreadGoalStatusRecord::Paused,
        ThreadGoalStatus::Blocked => ThreadGoalStatusRecord::Blocked,
        ThreadGoalStatus::UsageLimited => ThreadGoalStatusRecord::UsageLimited,
        ThreadGoalStatus::BudgetLimited => ThreadGoalStatusRecord::BudgetLimited,
        ThreadGoalStatus::Complete => ThreadGoalStatusRecord::Complete,
    }
}
