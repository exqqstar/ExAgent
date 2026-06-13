#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::bail;
use sqlx::Row;

use crate::index_db::IndexDb;

static REVIEW_TICKET_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewTicket {
    pub(crate) ticket_id: String,
    pub(crate) goal_id: String,
    pub(crate) baseline_hash: Option<String>,
    pub(crate) status: ReviewStatus,
    pub(crate) reviewed_hash: Option<String>,
    pub(crate) findings: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewVerdict {
    Approve,
    Reject,
}

#[derive(Clone)]
pub(crate) struct ReviewStore {
    db: IndexDb,
}

impl ReviewStore {
    pub(crate) fn new(db: IndexDb) -> Self {
        Self { db }
    }

    pub(crate) async fn mint_ticket(
        &self,
        goal_id: impl Into<String>,
        baseline_hash: Option<String>,
    ) -> anyhow::Result<ReviewTicket> {
        let goal_id = goal_id.into();
        let now = now_unix_millis();
        let ticket_order = REVIEW_TICKET_COUNTER.fetch_add(1, Ordering::Relaxed) as i64;
        let ticket_id = format!("rev_{now}_{ticket_order}");
        sqlx::query(
            r#"
INSERT INTO forge_review_tickets (
  ticket_id,
  goal_id,
  baseline_hash,
  status,
  reviewed_hash,
  findings,
  created_at_ms,
  updated_at_ms,
  ticket_order
) VALUES (?, ?, ?, ?, NULL, NULL, ?, ?, ?)
            "#,
        )
        .bind(&ticket_id)
        .bind(&goal_id)
        .bind(&baseline_hash)
        .bind(ReviewStatus::Pending.as_str())
        .bind(now)
        .bind(now)
        .bind(ticket_order)
        .execute(self.db.pool())
        .await?;
        Ok(ReviewTicket {
            ticket_id,
            goal_id,
            baseline_hash,
            status: ReviewStatus::Pending,
            reviewed_hash: None,
            findings: None,
        })
    }

    pub(crate) async fn resolve_ticket(
        &self,
        ticket_id: &str,
        verdict: ReviewVerdict,
        reviewed_hash: Option<String>,
        findings: Option<String>,
    ) -> anyhow::Result<ReviewTicket> {
        let now = now_unix_millis();
        let status = match verdict {
            ReviewVerdict::Approve => ReviewStatus::Approved,
            ReviewVerdict::Reject => ReviewStatus::Rejected,
        };
        let result = sqlx::query(
            r#"
UPDATE forge_review_tickets
SET status = ?,
    reviewed_hash = ?,
    findings = ?,
    updated_at_ms = ?
WHERE ticket_id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(&reviewed_hash)
        .bind(&findings)
        .bind(now)
        .bind(ticket_id)
        .execute(self.db.pool())
        .await?;
        if result.rows_affected() == 0 {
            bail!("unknown review ticket: {ticket_id}");
        }
        Ok(self
            .get_ticket(ticket_id)
            .await?
            .expect("resolved ticket should exist"))
    }

    pub(crate) async fn latest_fresh_approval(
        &self,
        goal_id: &str,
        current_hash: Option<&str>,
    ) -> anyhow::Result<Option<ReviewTicket>> {
        let Some(ticket) = self.latest_ticket(goal_id).await? else {
            return Ok(None);
        };
        if ticket.status == ReviewStatus::Approved
            && optional_str_eq(ticket.reviewed_hash.as_deref(), current_hash)
        {
            Ok(Some(ticket))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn consecutive_stuck_count(&self, goal_id: &str) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            r#"
SELECT
  ticket_id,
  goal_id,
  baseline_hash,
  status,
  reviewed_hash,
  findings
FROM forge_review_tickets
WHERE goal_id = ?
ORDER BY ticket_order DESC
            "#,
        )
        .bind(goal_id)
        .fetch_all(self.db.pool())
        .await?;

        let mut tickets = rows
            .iter()
            .map(review_ticket_from_row)
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter();
        let Some(first) = tickets.next() else {
            return Ok(0);
        };
        if first.status != ReviewStatus::Rejected {
            return Ok(0);
        }
        let baseline_hash = first.baseline_hash.clone();
        let mut count = 1;
        for ticket in tickets {
            if ticket.status == ReviewStatus::Rejected && ticket.baseline_hash == baseline_hash {
                count += 1;
            } else {
                break;
            }
        }
        Ok(count)
    }

    async fn get_ticket(&self, ticket_id: &str) -> anyhow::Result<Option<ReviewTicket>> {
        let row = sqlx::query(
            r#"
SELECT
  ticket_id,
  goal_id,
  baseline_hash,
  status,
  reviewed_hash,
  findings
FROM forge_review_tickets
WHERE ticket_id = ?
            "#,
        )
        .bind(ticket_id)
        .fetch_optional(self.db.pool())
        .await?;
        row.as_ref().map(review_ticket_from_row).transpose()
    }

    async fn latest_ticket(&self, goal_id: &str) -> anyhow::Result<Option<ReviewTicket>> {
        let row = sqlx::query(
            r#"
SELECT
  ticket_id,
  goal_id,
  baseline_hash,
  status,
  reviewed_hash,
  findings
FROM forge_review_tickets
WHERE goal_id = ?
ORDER BY ticket_order DESC
LIMIT 1
            "#,
        )
        .bind(goal_id)
        .fetch_optional(self.db.pool())
        .await?;
        row.as_ref().map(review_ticket_from_row).transpose()
    }
}

impl ReviewStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            _ => bail!("unknown review status: {value}"),
        }
    }
}

fn review_ticket_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<ReviewTicket> {
    Ok(ReviewTicket {
        ticket_id: row.try_get("ticket_id")?,
        goal_id: row.try_get("goal_id")?,
        baseline_hash: row.try_get("baseline_hash")?,
        status: ReviewStatus::from_str(row.try_get::<String, _>("status")?.as_str())?,
        reviewed_hash: row.try_get("reviewed_hash")?,
        findings: row.try_get("findings")?,
    })
}

fn optional_str_eq(left: Option<&str>, right: Option<&str>) -> bool {
    left == right
}

fn now_unix_millis() -> i64 {
    let now = ::time::OffsetDateTime::now_utc();
    now.unix_timestamp()
        .saturating_mul(1_000)
        .saturating_add(i64::from(now.millisecond()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StoreFixture {
        _dir: tempfile::TempDir,
        store: ReviewStore,
    }

    async fn store() -> StoreFixture {
        let dir = tempfile::tempdir().unwrap();
        let db = IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        StoreFixture {
            _dir: dir,
            store: ReviewStore::new(db),
        }
    }

    #[tokio::test]
    async fn mint_ticket_returns_unique_pending_ticket() {
        let fixture = store().await;
        let store = &fixture.store;

        let first = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();
        let second = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();

        assert_ne!(first.ticket_id, second.ticket_id);
        assert!(first.ticket_id.starts_with("rev_"));
        assert_eq!(first.goal_id, "goal_1");
        assert_eq!(first.baseline_hash.as_deref(), Some("hash_a"));
        assert_eq!(first.status, ReviewStatus::Pending);
        assert_eq!(first.reviewed_hash, None);
        assert_eq!(first.findings, None);
    }

    #[tokio::test]
    async fn resolve_ticket_records_verdict_hash_and_findings() {
        let fixture = store().await;
        let store = &fixture.store;
        let ticket = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();

        let resolved = store
            .resolve_ticket(
                &ticket.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_reviewed".to_string()),
                Some("missing assertion coverage".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(resolved.status, ReviewStatus::Rejected);
        assert_eq!(resolved.reviewed_hash.as_deref(), Some("hash_reviewed"));
        assert_eq!(
            resolved.findings.as_deref(),
            Some("missing assertion coverage")
        );

        let err = store
            .resolve_ticket(
                "rev_missing",
                ReviewVerdict::Approve,
                Some("hash_reviewed".to_string()),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown review ticket"));
    }

    #[tokio::test]
    async fn latest_fresh_approval_only_returns_newest_matching_approval() {
        let fixture = store().await;
        let store = &fixture.store;
        let first = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();
        store
            .resolve_ticket(
                &first.ticket_id,
                ReviewVerdict::Approve,
                Some("hash_a".to_string()),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .latest_fresh_approval("goal_1", Some("hash_a"))
                .await
                .unwrap()
                .map(|ticket| ticket.ticket_id),
            Some(first.ticket_id.clone())
        );
        assert!(store
            .latest_fresh_approval("goal_1", Some("hash_b"))
            .await
            .unwrap()
            .is_none());

        let rejected = store
            .mint_ticket("goal_1", Some("hash_b".to_string()))
            .await
            .unwrap();
        store
            .resolve_ticket(
                &rejected.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_b".to_string()),
                Some("still broken".to_string()),
            )
            .await
            .unwrap();
        assert!(store
            .latest_fresh_approval("goal_1", Some("hash_a"))
            .await
            .unwrap()
            .is_none());

        store
            .mint_ticket("goal_1", Some("hash_c".to_string()))
            .await
            .unwrap();
        assert!(store
            .latest_fresh_approval("goal_1", Some("hash_c"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn consecutive_stuck_count_counts_trailing_rejects_with_same_baseline_hash() {
        let fixture = store().await;
        let store = &fixture.store;

        assert_eq!(store.consecutive_stuck_count("goal_1").await.unwrap(), 0);

        let first = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();
        store
            .resolve_ticket(
                &first.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_a".to_string()),
                Some("first gap".to_string()),
            )
            .await
            .unwrap();
        let second = store
            .mint_ticket("goal_1", Some("hash_a".to_string()))
            .await
            .unwrap();
        store
            .resolve_ticket(
                &second.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_a".to_string()),
                Some("same gap".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(store.consecutive_stuck_count("goal_1").await.unwrap(), 2);

        let progressed = store
            .mint_ticket("goal_1", Some("hash_b".to_string()))
            .await
            .unwrap();
        store
            .resolve_ticket(
                &progressed.ticket_id,
                ReviewVerdict::Reject,
                Some("hash_b".to_string()),
                Some("new gap".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(store.consecutive_stuck_count("goal_1").await.unwrap(), 1);

        store
            .mint_ticket("goal_1", Some("hash_b".to_string()))
            .await
            .unwrap();
        assert_eq!(store.consecutive_stuck_count("goal_1").await.unwrap(), 0);
    }
}
