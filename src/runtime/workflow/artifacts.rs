use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app_server::protocol::WorkflowArtifactSummary;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactRecord {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub payload: Value,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ArtifactRecord {
    pub fn summary(&self) -> WorkflowArtifactSummary {
        WorkflowArtifactSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            status: self.status.clone(),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ArtifactStore {
    records: Vec<ArtifactRecord>,
    next_artifact_index: usize,
}

impl ArtifactStore {
    pub fn record(
        &mut self,
        label: impl Into<String>,
        status: Option<String>,
        payload: Value,
    ) -> WorkflowArtifactSummary {
        let now = current_unix_ms();
        self.next_artifact_index += 1;
        let record = ArtifactRecord {
            id: format!("artifact_{}", self.next_artifact_index),
            label: label.into(),
            status,
            payload,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let summary = record.summary();
        self.records.push(record);
        summary
    }

    pub fn update(
        &mut self,
        id: &str,
        status: Option<String>,
        payload: Value,
    ) -> Option<WorkflowArtifactSummary> {
        let now = current_unix_ms();
        let record = self.records.iter_mut().find(|record| record.id == id)?;
        record.status = status;
        record.payload = payload;
        record.updated_at_ms = now.max(record.created_at_ms);
        Some(record.summary())
    }

    pub fn get(&self, id: &str) -> Option<&ArtifactRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    pub fn list_summaries(&self) -> Vec<WorkflowArtifactSummary> {
        self.records.iter().map(ArtifactRecord::summary).collect()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

fn current_unix_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_list_get_and_update_artifacts() {
        let mut store = ArtifactStore::default();

        let first = store.record("Angles", Some("draft".to_string()), json!({"items": [1]}));
        let second = store.record("Claims", None, json!({"items": [2]}));

        assert_eq!(first.id, "artifact_1");
        assert_eq!(second.id, "artifact_2");
        assert_eq!(
            store
                .list_summaries()
                .into_iter()
                .map(|summary| summary.id)
                .collect::<Vec<_>>(),
            vec!["artifact_1".to_string(), "artifact_2".to_string()]
        );
        assert_eq!(
            store.get("artifact_1").unwrap().payload,
            json!({"items": [1]})
        );

        let updated = store
            .update(
                "artifact_1",
                Some("final".to_string()),
                json!({"items": [1, 2]}),
            )
            .expect("artifact updated");

        assert_eq!(updated.status.as_deref(), Some("final"));
        assert!(updated.updated_at_ms >= updated.created_at_ms);
        assert_eq!(
            store.get("artifact_1").unwrap().payload,
            json!({"items": [1, 2]})
        );
        assert!(store
            .update("missing", Some("final".to_string()), json!({}))
            .is_none());
    }
}
