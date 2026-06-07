use anyhow::Result;

use crate::app_server::protocol::{
    validate_thread_goal_objective, ThreadGoal, ThreadGoalClearParams, ThreadGoalClearResponse,
    ThreadGoalGetParams, ThreadGoalGetResponse, ThreadGoalSetParams, ThreadGoalSetResponse,
    ThreadGoalStatus,
};
use crate::app_server::services::AppServerServices;
use crate::app_server::AppServerError;
use crate::index_db::{GoalUpdate, ThreadGoalRecord, ThreadGoalStatusRecord};
use crate::runtime::goal::runtime::GoalRuntimeEvent;

pub(in crate::app_server) async fn thread_goal_set(
    services: &AppServerServices,
    params: ThreadGoalSetParams,
) -> Result<ThreadGoalSetResponse> {
    let db = goal_store(services)?;
    let current = db.get_thread_goal(&params.thread_id).await?;
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
    if let Some(runtime) = services.runtime_loader.runtime_for(&params.thread_id) {
        let effect = runtime
            .apply_goal_runtime_event(GoalRuntimeEvent::ExternalSet {
                thread_id: &params.thread_id,
                goal: goal.clone(),
                previous_goal,
            })
            .await?;
        runtime.enqueue_goal_runtime_effect(effect).await?;
    }

    Ok(ThreadGoalSetResponse { goal })
}

pub(in crate::app_server) async fn thread_goal_get(
    services: &AppServerServices,
    params: ThreadGoalGetParams,
) -> Result<ThreadGoalGetResponse> {
    let db = goal_store(services)?;
    Ok(ThreadGoalGetResponse {
        goal: db
            .get_thread_goal(&params.thread_id)
            .await?
            .map(thread_goal_from_record),
    })
}

pub(in crate::app_server) async fn thread_goal_clear(
    services: &AppServerServices,
    params: ThreadGoalClearParams,
) -> Result<ThreadGoalClearResponse> {
    let db = goal_store(services)?;
    account_before_external_mutation(services, &params.thread_id).await?;
    let cleared = db.delete_thread_goal(&params.thread_id).await?;
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
