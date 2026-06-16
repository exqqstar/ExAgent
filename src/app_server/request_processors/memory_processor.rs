use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use sqlx::Row;

use crate::app_server::override_policy::OverridePolicy;
use crate::app_server::protocol::{
    MemoryAuditEventView, MemoryAuditParams, MemoryAuditResponse, MemoryEntryView,
    MemoryForgetParams, MemoryForgetResponse, MemoryHitView, MemoryListArchivedParams,
    MemoryListArchivedResponse, MemoryListCandidatesParams, MemoryListCandidatesResponse,
    MemoryPromoteParams, MemoryPromoteResponse, MemorySaveInputView, MemorySaveParams,
    MemorySaveResponse, MemorySearchParams, MemorySearchResponse, MemoryUpdateAction,
    MemoryUpdateParams, MemoryUpdateResponse,
};
use crate::app_server::services::AppServerServices;
use crate::state::memory::{
    MemoryEntryKind, MemoryEntryRecord, MemoryRecallMode, MemorySaveInput, MemoryScope,
    MemorySearchHit, MemorySearchQuery,
};

const DESKTOP_MEMORY_ACTOR: &str = "desktop";
const DEFAULT_MEMORY_LIMIT: usize = 50;
const MAX_MEMORY_LIMIT: usize = 200;

struct MemoryRequestContext<'a> {
    db: &'a crate::index_db::IndexDb,
    scope: MemoryScope,
    project_id: Option<String>,
}

pub(in crate::app_server) async fn memory_search(
    services: &AppServerServices,
    params: MemorySearchParams,
) -> Result<MemorySearchResponse> {
    let context = memory_context(services, params.workspace_root.clone(), params.scope).await?;
    let query = MemorySearchQuery {
        scope: context.scope,
        project_id: context.project_id,
        thread_id: None,
        query: params.query,
        mode: MemoryRecallMode::DesktopInspect,
        limit: clamp_limit(params.limit),
        include_entries: true,
    };
    let hits = if query.query.trim().is_empty() {
        context.db.inspect_memory_for_scope(&query).await?
    } else {
        context.db.search_memory(query).await?
    };

    Ok(MemorySearchResponse {
        hits: hits.into_iter().map(memory_hit_view).collect(),
    })
}

pub(in crate::app_server) async fn memory_save(
    services: &AppServerServices,
    params: MemorySaveParams,
) -> Result<MemorySaveResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    let input = memory_save_input(params.input, context.scope)?;
    let entry = context
        .db
        .save_memory_entry_for_scope(
            context.project_id.as_deref(),
            None,
            input,
            DESKTOP_MEMORY_ACTOR,
        )
        .await?;

    Ok(MemorySaveResponse {
        entry: memory_entry_view(entry),
    })
}

pub(in crate::app_server) async fn memory_update(
    services: &AppServerServices,
    params: MemoryUpdateParams,
) -> Result<MemoryUpdateResponse> {
    let context = memory_context(
        services,
        params.workspace_root.clone(),
        params.scope.clone(),
    )
    .await?;
    let entry = match params.action {
        MemoryUpdateAction::Pin => {
            reject_edit_fields_for_state_action(&params)?;
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "active",
            )
            .await?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .set_memory_entry_pinned(&params.entry_id, true, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .set_memory_entry_pinned_with_scope(
                        &params.entry_id,
                        true,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
        MemoryUpdateAction::Unpin => {
            reject_edit_fields_for_state_action(&params)?;
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "active",
            )
            .await?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .set_memory_entry_pinned(&params.entry_id, false, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .set_memory_entry_pinned_with_scope(
                        &params.entry_id,
                        false,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
        MemoryUpdateAction::Archive => {
            reject_edit_fields_for_state_action(&params)?;
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "active",
            )
            .await?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .archive_memory_entry(&params.entry_id, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .archive_memory_entry_with_scope(
                        &params.entry_id,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
        MemoryUpdateAction::Unarchive => {
            reject_edit_fields_for_state_action(&params)?;
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "archived",
            )
            .await?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .unarchive_memory_entry(&params.entry_id, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .unarchive_memory_entry_with_scope(
                        &params.entry_id,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
        MemoryUpdateAction::Reject => {
            reject_edit_fields_for_state_action(&params)?;
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "candidate",
            )
            .await?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .reject_memory_candidate(&params.entry_id, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .reject_memory_candidate_with_scope(
                        &params.entry_id,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
        MemoryUpdateAction::Supersede => {
            ensure_entry_has_status_in_scope(
                context.db,
                &params.entry_id,
                context.scope,
                context.project_id.as_deref(),
                None,
                "active",
            )
            .await?;
            let input = memory_update_input(&params, context.scope)?;
            if context.scope == MemoryScope::Global {
                context
                    .db
                    .supersede_memory_entry(&params.entry_id, input, DESKTOP_MEMORY_ACTOR)
                    .await?
            } else {
                context
                    .db
                    .supersede_memory_entry_with_scope(
                        &params.entry_id,
                        input,
                        DESKTOP_MEMORY_ACTOR,
                        context.project_id.as_deref(),
                        None,
                    )
                    .await?
            }
        }
    };

    Ok(MemoryUpdateResponse {
        entry: memory_entry_view(entry),
    })
}

pub(in crate::app_server) async fn memory_forget(
    services: &AppServerServices,
    params: MemoryForgetParams,
) -> Result<MemoryForgetResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    ensure_entry_visible_in_scope(
        context.db,
        &params.entry_id,
        context.scope,
        context.project_id.as_deref(),
        None,
    )
    .await?;
    if context.scope == MemoryScope::Global {
        context
            .db
            .forget_memory_entry(&params.entry_id, DESKTOP_MEMORY_ACTOR)
            .await?;
    } else {
        context
            .db
            .forget_memory_entry_with_scope(
                &params.entry_id,
                DESKTOP_MEMORY_ACTOR,
                context.project_id.as_deref(),
                None,
            )
            .await?;
    }

    Ok(MemoryForgetResponse {
        entry_id: params.entry_id,
        forgotten: true,
    })
}

pub(in crate::app_server) async fn memory_audit(
    services: &AppServerServices,
    params: MemoryAuditParams,
) -> Result<MemoryAuditResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    let limit = clamp_limit(params.limit.unwrap_or(DEFAULT_MEMORY_LIMIT));
    let rows = if let Some(entry_id) = params.entry_id {
        let (predicate, project_id) =
            audit_scope_predicate(context.scope, context.project_id.as_deref())?;
        let sql = format!(
            r#"
SELECT a.id, a.memory_id, a.action, a.actor, a.detail_json, a.created_at_ms
FROM memory_audit_events a
JOIN memory_entries e ON e.id = a.memory_id
WHERE a.memory_id = ?
  AND {predicate}
ORDER BY a.created_at_ms DESC, a.id DESC
LIMIT ?
            "#,
        );
        let mut query = sqlx::query(&sql).bind(entry_id);
        if let Some(project_id) = project_id {
            query = query.bind(project_id);
        }
        query
            .bind(limit as i64)
            .fetch_all(context.db.pool())
            .await?
    } else {
        let (predicate, project_id) =
            audit_scope_predicate(context.scope, context.project_id.as_deref())?;
        let sql = format!(
            r#"
SELECT a.id, a.memory_id, a.action, a.actor, a.detail_json, a.created_at_ms
FROM memory_audit_events a
JOIN memory_entries e ON e.id = a.memory_id
WHERE {predicate}
ORDER BY a.created_at_ms DESC, a.id DESC
LIMIT ?
            "#,
        );
        let mut query = sqlx::query(&sql);
        if let Some(project_id) = project_id {
            query = query.bind(project_id);
        }
        query
            .bind(limit as i64)
            .fetch_all(context.db.pool())
            .await?
    };

    Ok(MemoryAuditResponse {
        events: rows
            .into_iter()
            .map(|row| {
                let detail_json: String = row.try_get("detail_json")?;
                let details = serde_json::from_str(&detail_json).unwrap_or(serde_json::Value::Null);
                Ok(MemoryAuditEventView {
                    id: row.try_get("id")?,
                    memory_id: row.try_get("memory_id")?,
                    action: row.try_get("action")?,
                    actor: row.try_get("actor")?,
                    created_at_ms: row.try_get("created_at_ms")?,
                    details,
                })
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

pub(in crate::app_server) async fn memory_list_candidates(
    services: &AppServerServices,
    params: MemoryListCandidatesParams,
) -> Result<MemoryListCandidatesResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    let candidates = context
        .db
        .list_memory_candidates(&MemorySearchQuery {
            scope: context.scope,
            project_id: context.project_id,
            thread_id: None,
            query: params.query.unwrap_or_default(),
            mode: MemoryRecallMode::DesktopInspect,
            limit: clamp_limit(params.limit.unwrap_or(DEFAULT_MEMORY_LIMIT)),
            include_entries: true,
        })
        .await?;

    Ok(MemoryListCandidatesResponse {
        candidates: candidates.into_iter().map(memory_entry_view).collect(),
    })
}

pub(in crate::app_server) async fn memory_list_archived(
    services: &AppServerServices,
    params: MemoryListArchivedParams,
) -> Result<MemoryListArchivedResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    let archived = context
        .db
        .list_archived_memory_entries(&MemorySearchQuery {
            scope: context.scope,
            project_id: context.project_id,
            thread_id: None,
            query: params.query.unwrap_or_default(),
            mode: MemoryRecallMode::DesktopInspect,
            limit: clamp_limit(params.limit.unwrap_or(DEFAULT_MEMORY_LIMIT)),
            include_entries: true,
        })
        .await?;

    Ok(MemoryListArchivedResponse {
        archived: archived.into_iter().map(memory_entry_view).collect(),
    })
}

pub(in crate::app_server) async fn memory_promote(
    services: &AppServerServices,
    params: MemoryPromoteParams,
) -> Result<MemoryPromoteResponse> {
    let context = memory_context(services, params.workspace_root, params.scope).await?;
    ensure_entry_has_status_in_scope(
        context.db,
        &params.entry_id,
        context.scope,
        context.project_id.as_deref(),
        None,
        "candidate",
    )
    .await?;

    let entry = context
        .db
        .promote_memory_candidate(
            &params.entry_id,
            DESKTOP_MEMORY_ACTOR,
            params.allow_quarantined_override,
        )
        .await?;

    Ok(MemoryPromoteResponse {
        entry: memory_entry_view(entry),
    })
}

async fn memory_context<'a>(
    services: &'a AppServerServices,
    workspace_root: Option<String>,
    scope: Option<String>,
) -> Result<MemoryRequestContext<'a>> {
    let Some(memory_runtime) = services.memory_runtime.as_ref() else {
        bail!("memory runtime unavailable");
    };
    let scope = parse_memory_scope(scope.as_deref())?;
    if scope == MemoryScope::Global {
        return Ok(MemoryRequestContext {
            db: memory_runtime.db(),
            scope,
            project_id: None,
        });
    }

    let workspace_root = workspace_root.context("workspace_root is required for memory APIs")?;
    let workspace_root =
        OverridePolicy::merge_thread_read(&services.base_config, Some(workspace_root))?
            .workspace_root;
    let project_id = memory_runtime
        .resolve_project_id_cached(&PathBuf::from(workspace_root))
        .await?
        .context("workspace_root is not registered as a desktop project")?;
    Ok(MemoryRequestContext {
        db: memory_runtime.db(),
        scope,
        project_id: Some(project_id),
    })
}

fn memory_save_input(input: MemorySaveInputView, scope: MemoryScope) -> Result<MemorySaveInput> {
    Ok(MemorySaveInput {
        scope,
        kind: parse_entry_kind(&input.kind)?,
        title: input.title,
        content: input.content,
        files: input.files,
        concepts: input.concepts,
        source_refs: vec![],
        pinned: input.pinned,
    })
}

fn memory_update_input(params: &MemoryUpdateParams, scope: MemoryScope) -> Result<MemorySaveInput> {
    Ok(MemorySaveInput {
        scope,
        kind: parse_entry_kind(
            params
                .kind
                .as_deref()
                .context("memory_update supersede/edit requires kind")?,
        )?,
        title: params
            .title
            .clone()
            .context("memory_update supersede/edit requires title")?,
        content: params
            .content
            .clone()
            .context("memory_update supersede/edit requires content")?,
        files: params.files.clone().unwrap_or_default(),
        concepts: params.concepts.clone().unwrap_or_default(),
        source_refs: vec![],
        pinned: params.pinned.unwrap_or(false),
    })
}

fn reject_edit_fields_for_state_action(params: &MemoryUpdateParams) -> Result<()> {
    if params.kind.is_some()
        || params.title.is_some()
        || params.content.is_some()
        || params.files.is_some()
        || params.concepts.is_some()
        || params.pinned.is_some()
    {
        bail!("memory_update state action does not accept edit fields");
    }
    Ok(())
}

async fn ensure_entry_visible_in_scope(
    db: &crate::index_db::IndexDb,
    entry_id: &str,
    scope: MemoryScope,
    project_id: Option<&str>,
    thread_id: Option<&str>,
) -> Result<()> {
    if memory_entry_matches_scope(db, entry_id, scope, project_id, thread_id, None).await? {
        Ok(())
    } else {
        bail!("memory entry {entry_id} is not visible in the selected scope")
    }
}

async fn ensure_entry_has_status_in_scope(
    db: &crate::index_db::IndexDb,
    entry_id: &str,
    scope: MemoryScope,
    project_id: Option<&str>,
    thread_id: Option<&str>,
    status: &str,
) -> Result<()> {
    if memory_entry_matches_scope(db, entry_id, scope, project_id, thread_id, Some(status)).await? {
        Ok(())
    } else {
        bail!("memory entry {entry_id} is not visible in the selected scope with status {status}")
    }
}

async fn memory_entry_matches_scope(
    db: &crate::index_db::IndexDb,
    entry_id: &str,
    scope: MemoryScope,
    project_id: Option<&str>,
    thread_id: Option<&str>,
    status: Option<&str>,
) -> Result<bool> {
    let mut sql = String::from(
        r#"
SELECT 1
FROM memory_entries
WHERE id = ?
        "#,
    );
    match scope {
        MemoryScope::Global => sql.push_str(" AND scope = 'global'"),
        MemoryScope::Project => sql.push_str(" AND scope = 'project' AND project_id = ?"),
        MemoryScope::Thread => {
            sql.push_str(" AND scope = 'thread' AND project_id = ? AND thread_id = ?")
        }
    }
    if status.is_some() {
        sql.push_str(" AND status = ?");
    }
    sql.push_str(" LIMIT 1");

    let mut query = sqlx::query_as::<_, (i64,)>(&sql).bind(entry_id);
    match scope {
        MemoryScope::Global => {}
        MemoryScope::Project => {
            query = query.bind(project_id.context("project scope requires project_id")?);
        }
        MemoryScope::Thread => {
            query = query
                .bind(project_id.context("thread scope requires project_id")?)
                .bind(thread_id.context("thread scope requires thread_id")?);
        }
    }
    if let Some(status) = status {
        query = query.bind(status);
    }
    let row = query.fetch_optional(db.pool()).await?;
    Ok(row.is_some())
}

fn parse_memory_scope(scope: Option<&str>) -> Result<MemoryScope> {
    match scope.unwrap_or("project") {
        "project" => Ok(MemoryScope::Project),
        "global" => Ok(MemoryScope::Global),
        other => bail!("unsupported desktop memory scope {other:?}"),
    }
}

fn audit_scope_predicate<'a>(
    scope: MemoryScope,
    project_id: Option<&'a str>,
) -> Result<(&'static str, Option<&'a str>)> {
    match scope {
        MemoryScope::Global => Ok(("e.scope = 'global'", None)),
        MemoryScope::Project => Ok((
            "e.scope = 'project' AND e.project_id = ?",
            Some(project_id.context("project scope requires project_id")?),
        )),
        MemoryScope::Thread => bail!("thread scope is not exposed by desktop memory APIs"),
    }
}

fn parse_entry_kind(kind: &str) -> Result<MemoryEntryKind> {
    match kind {
        "architecture" => Ok(MemoryEntryKind::Architecture),
        "preference" => Ok(MemoryEntryKind::Preference),
        "workflow" => Ok(MemoryEntryKind::Workflow),
        "bug" => Ok(MemoryEntryKind::Bug),
        "fact" => Ok(MemoryEntryKind::Fact),
        _ => bail!("unsupported memory kind {kind:?}"),
    }
}

fn memory_hit_view(hit: MemorySearchHit) -> MemoryHitView {
    MemoryHitView {
        id: hit.source_id,
        source: hit.source.as_str().to_string(),
        scope: hit.scope.as_str().to_string(),
        kind: hit.kind,
        title: hit.title,
        body: hit.body,
        files: hit.files,
        concepts: hit.concepts,
        confidence: hit.confidence,
        stale: hit.stale,
        quarantined: hit.quarantined,
        rank: hit.rank.final_score,
        pinned: hit.pinned,
        status: hit.status.map(|status| status.as_str().to_string()),
        use_count: hit.use_count,
        supersedes_id: hit.supersedes_id,
    }
}

fn memory_entry_view(entry: MemoryEntryRecord) -> MemoryEntryView {
    let quarantined = entry.privacy_flags.suspicious_injection;
    MemoryEntryView {
        id: entry.id,
        scope: entry.scope.as_str().to_string(),
        kind: entry.kind.as_str().to_string(),
        title: entry.title,
        body: entry.content,
        files: entry.files,
        concepts: entry.concepts,
        confidence: entry.confidence,
        pinned: entry.pinned,
        status: entry.status.as_str().to_string(),
        stale: false,
        quarantined,
        inactive_reason: entry.inactive_reason,
        supersedes_id: entry.supersedes_id,
        quarantine_reason: if quarantined {
            Some("Prompt-injection or sensitive provenance flagged".to_string())
        } else {
            None
        },
        created_at_ms: entry.created_at_ms,
        updated_at_ms: entry.updated_at_ms,
    }
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_MEMORY_LIMIT)
}
