use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{bail, Context};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{Row, Sqlite, Transaction};
use time::OffsetDateTime;

use crate::{state::index_db::IndexDb, types::ThreadId};

use super::{
    code_awareness::CodeAwarenessSnapshot,
    privacy, safety,
    types::{
        MemoryCodeRef, MemoryEntryKind, MemoryEntryRecord, MemoryPrivacyFlags, MemoryRankSignals,
        MemorySaveInput, MemoryScope, MemorySearchHit, MemorySearchQuery, MemorySourceKind,
        MemorySourceRef, MemoryStatus,
    },
};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

impl IndexDb {
    pub async fn save_memory_entry_for_scope(
        &self,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
        input: MemorySaveInput,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.insert_memory_entry(project_id, thread_id, input, actor, MemoryStatus::Active)
            .await
    }

    pub async fn propose_memory_candidate(
        &self,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
        input: MemorySaveInput,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.insert_memory_entry(project_id, thread_id, input, actor, MemoryStatus::Candidate)
            .await
    }

    pub async fn find_duplicate_memory_candidate_or_entry(
        &self,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
        input: &MemorySaveInput,
    ) -> anyhow::Result<Option<MemoryEntryRecord>> {
        validate_memory_write_scope(input.scope, project_id, thread_id)?;

        let privacy::RedactedMemoryText { text: title, .. } =
            privacy::redact_memory_text(&input.title, 512);
        let privacy::RedactedMemoryText { text: content, .. } =
            privacy::redact_memory_text(&input.content, 12 * 1024);
        let thread_id_string = thread_id.map(|thread_id| thread_id.as_str().to_string());
        let mut sql = String::from(
            r#"
SELECT e.*
FROM memory_entries e
WHERE e.scope = ?
  AND e.kind = ?
  AND e.title = ?
  AND e.content = ?
  AND e.status IN ('candidate', 'active', 'rejected')
            "#,
        );
        match input.scope {
            MemoryScope::Global => {
                sql.push_str(" AND e.project_id IS NULL AND e.thread_id IS NULL")
            }
            MemoryScope::Project => sql.push_str(" AND e.project_id = ? AND e.thread_id IS NULL"),
            MemoryScope::Thread => sql.push_str(" AND e.project_id = ? AND e.thread_id = ?"),
        }
        sql.push_str(
            r#"
ORDER BY CASE e.status
  WHEN 'active' THEN 0
  WHEN 'candidate' THEN 1
  WHEN 'rejected' THEN 2
  ELSE 3
END, e.updated_at_ms DESC
LIMIT 1
            "#,
        );

        let mut query = sqlx::query(&sql)
            .bind(input.scope.as_str())
            .bind(input.kind.as_str())
            .bind(title)
            .bind(content);
        match input.scope {
            MemoryScope::Global => {}
            MemoryScope::Project => {
                query = query.bind(project_id);
            }
            MemoryScope::Thread => {
                query = query.bind(project_id).bind(thread_id_string.as_deref());
            }
        }

        query
            .fetch_optional(self.pool())
            .await?
            .map(|row| memory_entry_from_row(&row))
            .transpose()
    }

    async fn insert_memory_entry(
        &self,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
        input: MemorySaveInput,
        actor: &str,
        status: MemoryStatus,
    ) -> anyhow::Result<MemoryEntryRecord> {
        validate_memory_write_scope(input.scope, project_id, thread_id)?;

        let now = now_unix_millis();
        let privacy::RedactedMemoryText {
            text: title,
            flags: mut privacy_flags,
        } = privacy::redact_memory_text(&input.title, 512);
        let privacy::RedactedMemoryText {
            text: content,
            flags: content_flags,
        } = privacy::redact_memory_text(&input.content, 12 * 1024);
        merge_privacy_flags(&mut privacy_flags, content_flags);
        let files = sanitize_memory_files(&input.files, &mut privacy_flags);
        let concepts = sanitize_memory_concepts(&input.concepts, &mut privacy_flags);
        let scan = safety::scan_injection(&combined_memory_text(&title, &content, &concepts));
        privacy_flags.suspicious_injection |= scan.suspicious;

        let id = memory_id(now, &title);
        let project_id = project_id.map(str::to_string);
        let thread_id_string = thread_id.map(|thread_id| thread_id.as_str().to_string());
        let code_refs = code_refs_for_files(&files);
        let files_json = to_json(&files, "memory entry files")?;
        let code_refs_json = to_json(&code_refs, "memory entry code refs")?;
        let concepts_json = to_json(&concepts, "memory entry concepts")?;
        let source_observation_ids_json = to_json(
            &Vec::<String>::new(),
            "deprecated memory entry source observation ids",
        )?;
        let source_refs_json = to_json(&input.source_refs, "memory entry source refs")?;
        let privacy_flags_json = to_json(&privacy_flags, "memory entry privacy flags")?;
        let confidence = match status {
            MemoryStatus::Active => 0.9,
            MemoryStatus::Candidate => 0.6,
            _ => bail!("unsupported memory insert status {}", status.as_str()),
        };
        let audit_action = match status {
            MemoryStatus::Active => "save",
            MemoryStatus::Candidate => "propose",
            _ => unreachable!(),
        };

        let mut tx = self.pool().begin().await?;
        sqlx::query(
            r#"
INSERT INTO memory_entries (
	  id, scope, project_id, thread_id, kind, title, content, files_json,
	  code_refs_json, concepts_json, source_observation_ids_json, source_refs_json, confidence,
	  strength, pinned, status, inactive_reason, supersedes_id, suspicious_injection,
	  privacy_flags_json, created_by, created_at_ms, updated_at_ms, last_used_at_ms,
	  use_count
	)
	VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 7, ?, ?, NULL, NULL, ?, ?, ?, ?, ?, NULL, 0)
	            "#,
        )
        .bind(&id)
        .bind(input.scope.as_str())
        .bind(project_id.as_deref())
        .bind(thread_id_string.as_deref())
        .bind(input.kind.as_str())
        .bind(&title)
        .bind(&content)
        .bind(&files_json)
        .bind(&code_refs_json)
        .bind(&concepts_json)
        .bind(&source_observation_ids_json)
        .bind(&source_refs_json)
        .bind(confidence)
        .bind(if input.pinned { 1_i64 } else { 0_i64 })
        .bind(status.as_str())
        .bind(if privacy_flags.suspicious_injection {
            1_i64
        } else {
            0_i64
        })
        .bind(&privacy_flags_json)
        .bind(actor)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        insert_entry_fts(
            &mut tx,
            &id,
            input.scope,
            project_id.as_deref(),
            thread_id_string.as_deref(),
            &title,
            &content,
            &files,
            &concepts,
        )
        .await?;
        insert_audit_event(&mut tx, &id, audit_action, actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(&id).await
    }

    pub async fn promote_memory_candidate(
        &self,
        id: &str,
        actor: &str,
        allow_quarantined_override: bool,
    ) -> anyhow::Result<MemoryEntryRecord> {
        let record = self.memory_entry_by_id(id).await?;
        if record.status != MemoryStatus::Candidate {
            bail!(
                "memory entry {id} must be candidate to promote, found {}",
                record.status.as_str()
            );
        }

        let now = now_unix_millis();
        if record.privacy_flags.suspicious_injection && !allow_quarantined_override {
            let mut tx = self.pool().begin().await?;
            insert_audit_event(&mut tx, id, "promote_blocked_quarantine", actor, "{}", now).await?;
            tx.commit().await?;
            bail!("memory entry {id} is quarantined and requires override to promote");
        }

        let action = if record.privacy_flags.suspicious_injection {
            "promote_quarantined_override"
        } else {
            "promote"
        };
        let mut tx = self.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE memory_entries SET status = 'active', inactive_reason = NULL, updated_at_ms = ? WHERE id = ? AND status = 'candidate'",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            bail!("memory candidate {id} was not promoted");
        }
        insert_audit_event(&mut tx, id, action, actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(id).await
    }

    /// old -> 'superseded' (+ inactive_reason, delete its FTS row); insert new 'active' with
    /// supersedes_id = old_id; write two audit events ("supersede_old", "supersede_new").
    pub async fn supersede_memory_entry(
        &self,
        old_id: &str,
        input: MemorySaveInput,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.supersede_memory_entry_inner(old_id, input, actor, None, None)
            .await
    }

    pub async fn supersede_memory_entry_with_scope(
        &self,
        old_id: &str,
        input: MemorySaveInput,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.supersede_memory_entry_inner(old_id, input, actor, project_id, thread_id)
            .await
    }

    async fn supersede_memory_entry_inner(
        &self,
        old_id: &str,
        input: MemorySaveInput,
        actor: &str,
        allowed_project_id: Option<&str>,
        allowed_thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        let old = self.memory_entry_by_id(old_id).await?;
        if old.status != MemoryStatus::Active {
            bail!(
                "memory entry {old_id} must be active to supersede, found {}",
                old.status.as_str()
            );
        }
        if !entry_matches_allowed_scope(&old, allowed_project_id, allowed_thread_id) {
            bail!("memory entry {old_id} is outside the current memory scope");
        }
        if input.scope != old.scope {
            bail!(
                "memory supersession cannot change scope from {} to {}",
                old.scope.as_str(),
                input.scope.as_str()
            );
        }
        validate_memory_write_scope(
            input.scope,
            old.project_id.as_deref(),
            old.thread_id.as_ref(),
        )?;

        let now = now_unix_millis();
        let privacy::RedactedMemoryText {
            text: title,
            flags: mut privacy_flags,
        } = privacy::redact_memory_text(&input.title, 512);
        let privacy::RedactedMemoryText {
            text: content,
            flags: content_flags,
        } = privacy::redact_memory_text(&input.content, 12 * 1024);
        merge_privacy_flags(&mut privacy_flags, content_flags);
        let files = sanitize_memory_files(&input.files, &mut privacy_flags);
        let concepts = sanitize_memory_concepts(&input.concepts, &mut privacy_flags);
        let source_refs = merge_source_refs(&old.source_refs, &input.source_refs);
        let scan = safety::scan_injection(&combined_memory_text(&title, &content, &concepts));
        privacy_flags.suspicious_injection |= scan.suspicious;
        privacy_flags.suspicious_injection |= old.privacy_flags.suspicious_injection;

        let new_id = memory_id(now, &title);
        let thread_id_string = old
            .thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_string());
        let code_refs = code_refs_for_files(&files);
        let files_json = to_json(&files, "memory entry files")?;
        let code_refs_json = to_json(&code_refs, "memory entry code refs")?;
        let concepts_json = to_json(&concepts, "memory entry concepts")?;
        let source_observation_ids_json = to_json(
            &Vec::<String>::new(),
            "deprecated memory entry source observation ids",
        )?;
        let source_refs_json = to_json(&source_refs, "memory entry source refs")?;
        let privacy_flags_json = to_json(&privacy_flags, "memory entry privacy flags")?;
        let inactive_reason = format!("superseded_by:{new_id}");

        let mut tx = self.pool().begin().await?;
        let old_result = sqlx::query(
            "UPDATE memory_entries SET status = 'superseded', inactive_reason = ?, updated_at_ms = ? WHERE id = ? AND status = 'active'",
        )
        .bind(&inactive_reason)
        .bind(now)
        .bind(old_id)
        .execute(&mut *tx)
        .await?;
        if old_result.rows_affected() == 0 {
            bail!("memory entry {old_id} was not superseded");
        }
        sqlx::query("DELETE FROM memory_entries_fts WHERE id = ?")
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
INSERT INTO memory_entries (
	  id, scope, project_id, thread_id, kind, title, content, files_json,
	  code_refs_json, concepts_json, source_observation_ids_json, source_refs_json, confidence,
	  strength, pinned, status, inactive_reason, supersedes_id, suspicious_injection,
	  privacy_flags_json, created_by, created_at_ms, updated_at_ms, last_used_at_ms,
	  use_count
	)
	VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0.9, 7, ?, 'active', NULL, ?, ?, ?, ?, ?, ?, NULL, 0)
	            "#,
        )
        .bind(&new_id)
        .bind(input.scope.as_str())
        .bind(old.project_id.as_deref())
        .bind(thread_id_string.as_deref())
        .bind(input.kind.as_str())
        .bind(&title)
        .bind(&content)
        .bind(&files_json)
        .bind(&code_refs_json)
        .bind(&concepts_json)
        .bind(&source_observation_ids_json)
        .bind(&source_refs_json)
        .bind(if input.pinned { 1_i64 } else { 0_i64 })
        .bind(old_id)
        .bind(if privacy_flags.suspicious_injection {
            1_i64
        } else {
            0_i64
        })
        .bind(&privacy_flags_json)
        .bind(actor)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_entry_fts(
            &mut tx,
            &new_id,
            input.scope,
            old.project_id.as_deref(),
            thread_id_string.as_deref(),
            &title,
            &content,
            &files,
            &concepts,
        )
        .await?;
        insert_audit_event(&mut tx, old_id, "supersede_old", actor, "{}", now).await?;
        insert_audit_event(&mut tx, &new_id, "supersede_new", actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(&new_id).await
    }

    pub async fn set_memory_entry_pinned(
        &self,
        id: &str,
        pinned: bool,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.set_memory_entry_pinned_inner(id, pinned, actor, None, None)
            .await
    }

    pub async fn set_memory_entry_pinned_with_scope(
        &self,
        id: &str,
        pinned: bool,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.set_memory_entry_pinned_inner(id, pinned, actor, project_id, thread_id)
            .await
    }

    async fn set_memory_entry_pinned_inner(
        &self,
        id: &str,
        pinned: bool,
        actor: &str,
        allowed_project_id: Option<&str>,
        allowed_thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        let record = self.memory_entry_by_id(id).await?;
        if record.status != MemoryStatus::Active {
            bail!(
                "memory entry {id} must be active to pin or unpin, found {}",
                record.status.as_str()
            );
        }
        if !entry_matches_allowed_scope(&record, allowed_project_id, allowed_thread_id) {
            bail!("memory entry {id} is outside the current memory scope");
        }

        let now = now_unix_millis();
        let mut tx = self.pool().begin().await?;
        let result =
            sqlx::query("UPDATE memory_entries SET pinned = ?, updated_at_ms = ? WHERE id = ?")
                .bind(if pinned { 1_i64 } else { 0_i64 })
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        if result.rows_affected() == 0 {
            bail!("memory entry {id} does not exist");
        }
        let action = if pinned { "pin" } else { "unpin" };
        insert_audit_event(&mut tx, id, action, actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(id).await
    }

    pub async fn archive_memory_entry(
        &self,
        id: &str,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.archive_memory_entry_with_scope(id, actor, None, None)
            .await
    }

    pub async fn archive_memory_entry_with_scope(
        &self,
        id: &str,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.transition_memory_entry_archive_state(
            id,
            MemoryStatus::Active,
            MemoryStatus::Archived,
            Some("archived"),
            "archive",
            actor,
            project_id,
            thread_id,
        )
        .await
    }

    pub async fn unarchive_memory_entry(
        &self,
        id: &str,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.unarchive_memory_entry_with_scope(id, actor, None, None)
            .await
    }

    pub async fn unarchive_memory_entry_with_scope(
        &self,
        id: &str,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.transition_memory_entry_archive_state(
            id,
            MemoryStatus::Archived,
            MemoryStatus::Active,
            None,
            "unarchive",
            actor,
            project_id,
            thread_id,
        )
        .await
    }

    async fn transition_memory_entry_archive_state(
        &self,
        id: &str,
        expected_status: MemoryStatus,
        target_status: MemoryStatus,
        inactive_reason: Option<&str>,
        action: &str,
        actor: &str,
        allowed_project_id: Option<&str>,
        allowed_thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        let record = self.memory_entry_by_id(id).await?;
        if record.status != expected_status {
            bail!(
                "memory entry {id} must be {} to {action}, found {}",
                expected_status.as_str(),
                record.status.as_str()
            );
        }
        if !entry_matches_allowed_scope(&record, allowed_project_id, allowed_thread_id) {
            bail!("memory entry {id} is outside the current memory scope");
        }

        let now = now_unix_millis();
        let thread_id_string = record
            .thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_string());
        let mut tx = self.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE memory_entries SET status = ?, inactive_reason = ?, updated_at_ms = ? WHERE id = ? AND status = ?",
        )
        .bind(target_status.as_str())
        .bind(inactive_reason)
        .bind(now)
        .bind(id)
        .bind(expected_status.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            bail!("memory entry {id} was not {action}d");
        }

        sqlx::query("DELETE FROM memory_entries_fts WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if target_status == MemoryStatus::Active {
            insert_entry_fts(
                &mut tx,
                id,
                record.scope,
                record.project_id.as_deref(),
                thread_id_string.as_deref(),
                &record.title,
                &record.content,
                &record.files,
                &record.concepts,
            )
            .await?;
        }
        insert_audit_event(&mut tx, id, action, actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(id).await
    }

    pub async fn forget_memory_entry(&self, id: &str, actor: &str) -> anyhow::Result<()> {
        self.forget_memory_entry_with_scope(id, actor, None, None)
            .await
    }

    pub async fn forget_memory_entry_with_scope(
        &self,
        id: &str,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<()> {
        let now = now_unix_millis();
        let mut tx = self.pool().begin().await?;
        let mut sql =
            "UPDATE memory_entries SET status = 'deleted', inactive_reason = 'user_forget', updated_at_ms = ? WHERE id = ?"
                .to_string();
        if project_id.is_some() {
            sql.push_str(" AND ((scope = 'project' AND project_id = ?)");
            if thread_id.is_some() {
                sql.push_str(" OR (scope = 'thread' AND project_id = ? AND thread_id = ?)");
            }
            sql.push(')');
        }

        let mut update = sqlx::query(&sql).bind(now).bind(id);
        if let Some(project_id) = project_id {
            update = update.bind(project_id);
            if let Some(thread_id) = thread_id {
                update = update.bind(project_id).bind(thread_id.as_str());
            }
        }
        let result = update.execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            bail!("memory entry {id} does not exist");
        }
        sqlx::query("DELETE FROM memory_entries_fts WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        insert_audit_event(&mut tx, id, "forget", actor, "{}", now).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn reject_memory_candidate(
        &self,
        id: &str,
        actor: &str,
    ) -> anyhow::Result<MemoryEntryRecord> {
        self.reject_memory_candidate_with_scope(id, actor, None, None)
            .await
    }

    pub async fn reject_memory_candidate_with_scope(
        &self,
        id: &str,
        actor: &str,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
    ) -> anyhow::Result<MemoryEntryRecord> {
        let record = self.memory_entry_by_id(id).await?;
        if record.status != MemoryStatus::Candidate {
            bail!(
                "memory entry {id} must be candidate to reject, found {}",
                record.status.as_str()
            );
        }
        if !entry_matches_allowed_scope(&record, project_id, thread_id) {
            bail!("memory entry {id} is outside the current memory scope");
        }

        let now = now_unix_millis();
        let mut tx = self.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE memory_entries SET status = 'rejected', inactive_reason = 'candidate_rejected', updated_at_ms = ? WHERE id = ? AND status = 'candidate'",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            bail!("memory candidate {id} was not rejected");
        }
        sqlx::query("DELETE FROM memory_entries_fts WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        insert_audit_event(&mut tx, id, "reject", actor, "{}", now).await?;
        tx.commit().await?;

        self.memory_entry_by_id(id).await
    }

    pub async fn list_memory_candidates(
        &self,
        query: &MemorySearchQuery,
    ) -> anyhow::Result<Vec<MemoryEntryRecord>> {
        self.list_memory_entries_by_status(query, MemoryStatus::Candidate)
            .await
    }

    pub async fn list_archived_memory_entries(
        &self,
        query: &MemorySearchQuery,
    ) -> anyhow::Result<Vec<MemoryEntryRecord>> {
        self.list_memory_entries_by_status(query, MemoryStatus::Archived)
            .await
    }

    async fn list_memory_entries_by_status(
        &self,
        query: &MemorySearchQuery,
        status: MemoryStatus,
    ) -> anyhow::Result<Vec<MemoryEntryRecord>> {
        if !query.include_entries {
            return Ok(Vec::new());
        }

        let mut sql = format!(
            r#"
SELECT e.*
FROM memory_entries e
WHERE status = ?
  AND {}
            "#,
            scope_predicate("e", query.scope)
        );
        let has_search = !query.query.trim().is_empty();
        if has_search {
            sql.push_str(
                " AND (instr(title, ?) > 0 OR instr(content, ?) > 0 OR instr(files_json, ?) > 0 OR instr(concepts_json, ?) > 0)",
            );
        }
        sql.push_str(" ORDER BY updated_at_ms DESC LIMIT ?");

        let thread_id = query
            .thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_string());
        let mut db_query = sqlx::query(&sql).bind(status.as_str());
        db_query = bind_scope_context(db_query, query, thread_id.as_deref());
        if has_search {
            let term = query.query.trim().to_string();
            db_query = db_query
                .bind(term.clone())
                .bind(term.clone())
                .bind(term.clone())
                .bind(term);
        }
        db_query = db_query.bind(query.limit as i64);

        let rows = db_query.fetch_all(self.pool()).await?;
        rows.into_iter()
            .map(|row| memory_entry_from_row(&row))
            .collect()
    }

    pub async fn search_memory(
        &self,
        query: MemorySearchQuery,
    ) -> anyhow::Result<Vec<MemorySearchHit>> {
        crate::state::memory::ranker::search_and_rank(self, query).await
    }

    pub async fn inspect_memory_for_scope(
        &self,
        query: &MemorySearchQuery,
    ) -> anyhow::Result<Vec<MemorySearchHit>> {
        let limit = query.limit.max(1).min(200) as i64;
        let thread_id = query
            .thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_string());
        let mut hits = Vec::new();

        if query.include_entries {
            let sql = format!(
                r#"
SELECT e.*
FROM memory_entries e
WHERE e.status = 'active'
  AND {}
ORDER BY e.pinned DESC, e.updated_at_ms DESC
LIMIT ?
                "#,
                scope_predicate("e", query.scope)
            );
            let mut db_query = sqlx::query(&sql);
            db_query = bind_scope_context(db_query, query, thread_id.as_deref());
            let rows = db_query.bind(limit).fetch_all(self.pool()).await?;
            for row in rows {
                hits.push(memory_entry_hit_from_row(&row)?);
            }
        }

        Ok(hits)
    }

    pub async fn frozen_memory_for_scope(
        &self,
        project_id: Option<&str>,
        thread_id: Option<&ThreadId>,
        max_chars: usize,
    ) -> anyhow::Result<Vec<MemorySearchHit>> {
        let row_limit = ((max_chars / 160).max(8).min(200)) as i64;
        let thread_id_string = thread_id.map(|thread_id| thread_id.as_str().to_string());
        let mut hits = Vec::new();

        let entry_sql = format!(
            r#"
SELECT e.*
FROM memory_entries e
WHERE e.status = 'active'
  AND e.pinned = 1
  AND e.suspicious_injection = 0
  AND {}
ORDER BY e.scope DESC, e.updated_at_ms DESC
LIMIT ?
            "#,
            frozen_scope_predicate("e", project_id.is_some(), thread_id.is_some())
        );
        let mut entry_query = sqlx::query(&entry_sql);
        entry_query =
            bind_frozen_scope_context(entry_query, project_id, thread_id_string.as_deref());
        let entry_rows = entry_query.bind(row_limit).fetch_all(self.pool()).await?;
        for row in entry_rows {
            let hit = memory_entry_hit_from_row(&row)?;
            if hit.confidence.is_finite() {
                hits.push(hit);
            }
        }

        if let Some(project_id) = project_id {
            if let Some(workspace_root) = self.workspace_root_for_project(project_id).await? {
                let code_awareness = CodeAwarenessSnapshot::from_prompt(Some(workspace_root), "");
                let mut stale_check_budget = usize::MAX;
                for hit in &mut hits {
                    let code_score = code_awareness.score_refs_with_budget(
                        &hit.files,
                        &hit.code_refs,
                        &mut stale_check_budget,
                    );
                    hit.stale = code_score.stale;
                    hit.rank.working_set_boost = code_score.working_set_boost;
                    hit.rank.stale_penalty = code_score.stale_penalty;
                }
                hits.retain(|hit| !hit.stale);
            }
        }

        Ok(hits)
    }

    pub async fn project_id_for_existing_path(
        &self,
        workspace_root: impl AsRef<Path>,
    ) -> anyhow::Result<Option<String>> {
        let path = tokio::fs::canonicalize(workspace_root.as_ref()).await?;
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM projects WHERE path = ?")
            .bind(path.display().to_string())
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(|(id,)| id))
    }

    pub async fn workspace_root_for_project(
        &self,
        project_id: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT path FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(|(path,)| PathBuf::from(path)))
    }

    pub(crate) async fn memory_search_candidates_for_ranker(
        &self,
        query: &MemorySearchQuery,
        fts: &str,
    ) -> anyhow::Result<Vec<MemorySearchHit>> {
        let mut hits = Vec::new();
        let thread_id = query
            .thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_string());

        if query.include_entries {
            let sql = format!(
                r#"
SELECT e.*
FROM memory_entries_fts
JOIN memory_entries e ON e.id = memory_entries_fts.id
WHERE memory_entries_fts MATCH ?
  AND e.status = 'active'
  AND {}
ORDER BY bm25(memory_entries_fts), e.pinned DESC, e.updated_at_ms DESC
LIMIT 200
                "#,
                scope_predicate("e", query.scope)
            );
            let mut db_query = sqlx::query(&sql).bind(fts);
            db_query = bind_scope_context(db_query, query, thread_id.as_deref());
            let rows = db_query.fetch_all(self.pool()).await?;

            for row in rows {
                hits.push(memory_entry_hit_from_row(&row)?);
            }
        }

        Ok(hits)
    }

    /// Exists only for integration tests until the real memory audit API lands.
    #[doc(hidden)]
    pub async fn memory_audit_actions_for_tests(
        &self,
        memory_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT action FROM memory_audit_events WHERE memory_id = ? ORDER BY created_at_ms, id",
        )
        .bind(memory_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| row.try_get("action").map_err(Into::into))
            .collect()
    }

    /// Exists only for integration tests until the real memory inspection API lands.
    #[doc(hidden)]
    pub async fn memory_entry_for_tests(&self, id: &str) -> anyhow::Result<MemoryEntryRecord> {
        self.memory_entry_by_id(id).await
    }

    async fn memory_entry_by_id(&self, id: &str) -> anyhow::Result<MemoryEntryRecord> {
        let row = sqlx::query("SELECT * FROM memory_entries WHERE id = ?")
            .bind(id)
            .fetch_one(self.pool())
            .await?;
        memory_entry_from_row(&row)
    }
}

fn validate_memory_write_scope(
    scope: MemoryScope,
    project_id: Option<&str>,
    thread_id: Option<&ThreadId>,
) -> anyhow::Result<()> {
    match scope {
        MemoryScope::Global => {
            if project_id.is_some() || thread_id.is_some() {
                bail!("global memory entries cannot include project_id or thread_id");
            }
        }
        MemoryScope::Project => {
            if project_id.is_none() {
                bail!("project memory entries require project_id");
            }
            if thread_id.is_some() {
                bail!("project memory entries cannot include thread_id");
            }
        }
        MemoryScope::Thread => {
            if project_id.is_none() {
                bail!("thread memory entries require project_id");
            }
            if thread_id.is_none() {
                bail!("thread memory entries require thread_id");
            }
        }
    }
    Ok(())
}

fn entry_matches_allowed_scope(
    entry: &MemoryEntryRecord,
    project_id: Option<&str>,
    thread_id: Option<&ThreadId>,
) -> bool {
    let Some(project_id) = project_id else {
        return true;
    };

    match entry.scope {
        MemoryScope::Global => false,
        MemoryScope::Project => entry.project_id.as_deref() == Some(project_id),
        MemoryScope::Thread => {
            let Some(thread_id) = thread_id else {
                return false;
            };
            entry.project_id.as_deref() == Some(project_id)
                && entry.thread_id.as_ref().map(|id| id.as_str()) == Some(thread_id.as_str())
        }
    }
}

fn scope_predicate(alias: &str, scope: MemoryScope) -> String {
    match scope {
        MemoryScope::Global => format!("{alias}.scope = 'global'"),
        MemoryScope::Project => format!(
            "({alias}.scope = 'global' OR ({alias}.scope = 'project' AND {alias}.project_id = ?))"
        ),
        MemoryScope::Thread => format!(
            "({alias}.scope = 'global' OR ({alias}.scope = 'project' AND {alias}.project_id = ?) OR ({alias}.scope = 'thread' AND {alias}.project_id = ? AND {alias}.thread_id = ?))"
        ),
    }
}

fn bind_scope_context<'q>(
    mut query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    search: &'q MemorySearchQuery,
    thread_id: Option<&'q str>,
) -> sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match search.scope {
        MemoryScope::Global => {}
        MemoryScope::Project => {
            query = query.bind(search.project_id.as_deref());
        }
        MemoryScope::Thread => {
            query = query
                .bind(search.project_id.as_deref())
                .bind(search.project_id.as_deref())
                .bind(thread_id);
        }
    }
    query
}

fn frozen_scope_predicate(alias: &str, has_project_id: bool, has_thread_id: bool) -> String {
    match (has_project_id, has_thread_id) {
        (true, true) => format!(
            "({alias}.scope = 'global' OR ({alias}.scope = 'project' AND {alias}.project_id = ?) OR ({alias}.scope = 'thread' AND {alias}.project_id = ? AND {alias}.thread_id = ?))"
        ),
        (true, false) => format!(
            "({alias}.scope = 'global' OR ({alias}.scope = 'project' AND {alias}.project_id = ?))"
        ),
        (false, _) => format!("{alias}.scope = 'global'"),
    }
}

fn bind_frozen_scope_context<'q>(
    mut query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    project_id: Option<&'q str>,
    thread_id: Option<&'q str>,
) -> sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match (project_id, thread_id) {
        (Some(project_id), Some(thread_id)) => {
            query = query.bind(project_id).bind(project_id).bind(thread_id);
        }
        (Some(project_id), None) => {
            query = query.bind(project_id);
        }
        (None, _) => {}
    }
    query
}

async fn insert_entry_fts(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    scope: MemoryScope,
    project_id: Option<&str>,
    thread_id: Option<&str>,
    title: &str,
    content: &str,
    files: &[String],
    concepts: &[String],
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
INSERT INTO memory_entries_fts (
  id, scope, project_id, thread_id, title, content, files, concepts
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(scope.as_str())
    .bind(project_id)
    .bind(thread_id)
    .bind(title)
    .bind(content)
    .bind(files.join(" "))
    .bind(concepts.join(" "))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_audit_event(
    tx: &mut Transaction<'_, Sqlite>,
    memory_id: &str,
    action: &str,
    actor: &str,
    detail_json: &str,
    now: i64,
) -> anyhow::Result<()> {
    let id = audit_id(now, memory_id, action);
    sqlx::query(
        r#"
INSERT INTO memory_audit_events (
  id, memory_id, action, actor, detail_json, created_at_ms
)
VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(memory_id)
    .bind(action)
    .bind(actor)
    .bind(detail_json)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn memory_entry_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<MemoryEntryRecord> {
    let files = parse_json_vec::<String>(row.try_get("files_json")?, "memory entry files_json")?;
    let code_refs = parse_code_refs(row.try_get("code_refs_json")?, &files)?;
    let mut privacy_flags = parse_privacy_flags(row.try_get("privacy_flags_json")?);
    privacy_flags.suspicious_injection |= row.try_get::<i64, _>("suspicious_injection")? != 0;
    Ok(MemoryEntryRecord {
        id: row.try_get("id")?,
        scope: parse_scope(row.try_get::<String, _>("scope")?.as_str())?,
        project_id: row.try_get("project_id")?,
        thread_id: row
            .try_get::<Option<String>, _>("thread_id")?
            .map(ThreadId::new),
        kind: parse_entry_kind(row.try_get::<String, _>("kind")?.as_str())?,
        title: row.try_get("title")?,
        content: row.try_get("content")?,
        files,
        code_refs,
        concepts: parse_json_vec(row.try_get("concepts_json")?, "memory entry concepts_json")?,
        source_refs: parse_json_vec(
            row.try_get("source_refs_json")?,
            "memory entry source_refs_json",
        )?,
        confidence: row.try_get("confidence")?,
        strength: row.try_get("strength")?,
        pinned: row.try_get::<i64, _>("pinned")? != 0,
        status: parse_status(row.try_get::<String, _>("status")?.as_str())?,
        inactive_reason: row.try_get("inactive_reason")?,
        supersedes_id: row.try_get("supersedes_id")?,
        privacy_flags,
        created_by: row.try_get("created_by")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
        last_used_at_ms: row.try_get("last_used_at_ms")?,
        use_count: row.try_get("use_count")?,
    })
}

fn memory_entry_hit_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<MemorySearchHit> {
    let entry = memory_entry_from_row(row)?;
    let quarantined = entry.privacy_flags.suspicious_injection;
    Ok(MemorySearchHit {
        source_id: entry.id,
        source: MemorySourceKind::Entry,
        scope: entry.scope,
        kind: entry.kind.as_str().to_string(),
        title: entry.title,
        body: entry.content,
        files: entry.files,
        code_refs: entry.code_refs,
        concepts: entry.concepts,
        source_refs: entry.source_refs,
        confidence: entry.confidence,
        stale: false,
        quarantined,
        pinned: entry.pinned,
        status: Some(entry.status),
        supersedes_id: entry.supersedes_id,
        use_count: entry.use_count,
        thread_id: entry.thread_id,
        turn_id: None,
        rank: zero_rank(),
    })
}

fn parse_scope(value: &str) -> anyhow::Result<MemoryScope> {
    match value {
        "global" => Ok(MemoryScope::Global),
        "project" => Ok(MemoryScope::Project),
        "thread" => Ok(MemoryScope::Thread),
        _ => bail!("unknown memory scope {value:?}"),
    }
}

fn parse_entry_kind(value: &str) -> anyhow::Result<MemoryEntryKind> {
    match value {
        "architecture" => Ok(MemoryEntryKind::Architecture),
        "preference" => Ok(MemoryEntryKind::Preference),
        "workflow" => Ok(MemoryEntryKind::Workflow),
        "bug" => Ok(MemoryEntryKind::Bug),
        "fact" => Ok(MemoryEntryKind::Fact),
        _ => bail!("unknown memory entry kind {value:?}"),
    }
}

fn parse_status(value: &str) -> anyhow::Result<MemoryStatus> {
    match value {
        "candidate" => Ok(MemoryStatus::Candidate),
        "active" => Ok(MemoryStatus::Active),
        "superseded" => Ok(MemoryStatus::Superseded),
        "rejected" => Ok(MemoryStatus::Rejected),
        "archived" => Ok(MemoryStatus::Archived),
        "deleted" => Ok(MemoryStatus::Deleted),
        _ => bail!("unknown memory status {value:?}"),
    }
}

fn parse_json_vec<T>(raw: String, field: &str) -> anyhow::Result<Vec<T>>
where
    T: DeserializeOwned,
{
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {field}"))
}

fn parse_code_refs(raw: String, files: &[String]) -> anyhow::Result<Vec<MemoryCodeRef>> {
    if raw.trim().is_empty() {
        return Ok(code_refs_for_files(files));
    }
    let refs: Vec<MemoryCodeRef> =
        serde_json::from_str(&raw).context("failed to parse memory code_refs_json")?;
    if refs.is_empty() {
        Ok(code_refs_for_files(files))
    } else {
        Ok(refs)
    }
}

fn parse_privacy_flags(raw: String) -> MemoryPrivacyFlags {
    if raw.trim().is_empty() {
        return MemoryPrivacyFlags::default();
    }
    serde_json::from_str(&raw).unwrap_or_default()
}

fn to_json<T>(value: &T, field: &str) -> anyhow::Result<String>
where
    T: Serialize,
{
    serde_json::to_string(value).with_context(|| format!("failed to serialize {field}"))
}

fn code_refs_for_files(files: &[String]) -> Vec<MemoryCodeRef> {
    files
        .iter()
        .map(|path| MemoryCodeRef {
            path: path.clone(),
            line: None,
            symbol: None,
        })
        .collect()
}

fn sanitize_memory_files(files: &[String], privacy_flags: &mut MemoryPrivacyFlags) -> Vec<String> {
    files
        .iter()
        .filter_map(|path| {
            if is_sensitive_memory_path(path) {
                privacy_flags.sensitive_path = true;
                None
            } else {
                Some(path.clone())
            }
        })
        .collect()
}

fn sanitize_memory_concepts(
    concepts: &[String],
    privacy_flags: &mut MemoryPrivacyFlags,
) -> Vec<String> {
    concepts
        .iter()
        .map(|concept| {
            let redacted = privacy::redact_memory_text(concept, 512);
            merge_privacy_flags(privacy_flags, redacted.flags);
            redacted.text
        })
        .collect()
}

fn is_sensitive_memory_path(path: &str) -> bool {
    matches!(
        privacy::classify_memory_path(path),
        privacy::MemoryPathSensitivity::Sensitive
    )
}

fn combined_memory_text(title: &str, body: &str, concepts: &[String]) -> String {
    let concepts_len = concepts.iter().map(String::len).sum::<usize>();
    let mut text =
        String::with_capacity(title.len() + body.len() + concepts_len + concepts.len() + 1);
    text.push_str(title);
    text.push('\n');
    text.push_str(body);
    for concept in concepts {
        text.push('\n');
        text.push_str(concept);
    }
    text
}

fn merge_privacy_flags(target: &mut MemoryPrivacyFlags, other: MemoryPrivacyFlags) {
    target.redacted_secret |= other.redacted_secret;
    target.redacted_private_block |= other.redacted_private_block;
    target.sensitive_path |= other.sensitive_path;
    target.output_truncated |= other.output_truncated;
    target.suspicious_injection |= other.suspicious_injection;
}

fn merge_source_refs(
    existing: &[MemorySourceRef],
    incoming: &[MemorySourceRef],
) -> Vec<MemorySourceRef> {
    let mut merged = Vec::with_capacity(existing.len() + incoming.len());
    for source_ref in existing.iter().chain(incoming.iter()) {
        if !merged.iter().any(|known| known == source_ref) {
            merged.push(source_ref.clone());
        }
    }
    merged
}

fn zero_rank() -> MemoryRankSignals {
    MemoryRankSignals {
        text_rank: 0.0,
        scope_boost: 0.0,
        confidence_boost: 0.0,
        strength_boost: 0.0,
        recency_boost: 0.0,
        working_set_boost: 0.0,
        stale_penalty: 0.0,
        privacy_penalty: 0.0,
        final_score: 0.0,
    }
}

fn memory_id(now: i64, title: &str) -> String {
    format!(
        "mem_{now}_{}_{}",
        ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        id_fragment(title)
    )
}

fn audit_id(now: i64, memory_id: &str, action: &str) -> String {
    format!(
        "audit_{now}_{}_{}_{}",
        ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        id_fragment(action),
        id_fragment(memory_id)
    )
}

fn id_fragment(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !out.is_empty() {
            out.push('_');
            last_was_separator = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "memory".into()
    } else {
        out
    }
}

fn now_unix_millis() -> i64 {
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    (nanos / 1_000_000) as i64
}
