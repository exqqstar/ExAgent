use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

#[async_trait]
pub trait WorkflowProgressSink: Send + Sync {
    async fn declare_phase(&self, id: &str, label: &str, planned_count: usize);
    async fn start_phase(&self, id: &str, label: &str, planned_count: usize);
    async fn update_phase_counts(
        &self,
        id: &str,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    );
    async fn complete_phase(&self, id: &str);
    async fn fail_phase(&self, id: &str);
    async fn cancel_phase(&self, id: &str);
    async fn skip_phase(&self, id: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowProgressEvent {
    Declared {
        id: String,
        label: String,
        planned_count: usize,
    },
    Started {
        id: String,
        label: String,
        planned_count: usize,
    },
    CountsUpdated {
        id: String,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    },
    Completed {
        id: String,
    },
    Failed {
        id: String,
    },
    Cancelled {
        id: String,
    },
    Skipped {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopWorkflowProgressSink;

#[async_trait]
impl WorkflowProgressSink for NoopWorkflowProgressSink {
    async fn declare_phase(&self, _id: &str, _label: &str, _planned_count: usize) {}

    async fn start_phase(&self, _id: &str, _label: &str, _planned_count: usize) {}

    async fn update_phase_counts(
        &self,
        _id: &str,
        _completed_count: usize,
        _failed_count: usize,
        _skipped_count: usize,
    ) {
    }

    async fn complete_phase(&self, _id: &str) {}

    async fn fail_phase(&self, _id: &str) {}

    async fn cancel_phase(&self, _id: &str) {}

    async fn skip_phase(&self, _id: &str) {}
}

#[derive(Debug, Clone)]
pub struct RecordingWorkflowProgressSink {
    events: Arc<Mutex<Vec<WorkflowProgressEvent>>>,
}

impl RecordingWorkflowProgressSink {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn events(&self) -> Vec<WorkflowProgressEvent> {
        self.events.lock().await.clone()
    }

    pub async fn take_events(&self) -> Vec<WorkflowProgressEvent> {
        let mut events = self.events.lock().await;
        std::mem::take(&mut *events)
    }

    async fn push_event(&self, event: WorkflowProgressEvent) {
        self.events.lock().await.push(event);
    }
}

impl Default for RecordingWorkflowProgressSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WorkflowProgressSink for RecordingWorkflowProgressSink {
    async fn declare_phase(&self, id: &str, label: &str, planned_count: usize) {
        self.push_event(WorkflowProgressEvent::Declared {
            id: id.to_string(),
            label: label.to_string(),
            planned_count,
        })
        .await;
    }

    async fn start_phase(&self, id: &str, label: &str, planned_count: usize) {
        self.push_event(WorkflowProgressEvent::Started {
            id: id.to_string(),
            label: label.to_string(),
            planned_count,
        })
        .await;
    }

    async fn update_phase_counts(
        &self,
        id: &str,
        completed_count: usize,
        failed_count: usize,
        skipped_count: usize,
    ) {
        self.push_event(WorkflowProgressEvent::CountsUpdated {
            id: id.to_string(),
            completed_count,
            failed_count,
            skipped_count,
        })
        .await;
    }

    async fn complete_phase(&self, id: &str) {
        self.push_event(WorkflowProgressEvent::Completed { id: id.to_string() })
            .await;
    }

    async fn fail_phase(&self, id: &str) {
        self.push_event(WorkflowProgressEvent::Failed { id: id.to_string() })
            .await;
    }

    async fn cancel_phase(&self, id: &str) {
        self.push_event(WorkflowProgressEvent::Cancelled { id: id.to_string() })
            .await;
    }

    async fn skip_phase(&self, id: &str) {
        self.push_event(WorkflowProgressEvent::Skipped { id: id.to_string() })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NoopWorkflowProgressSink, RecordingWorkflowProgressSink, WorkflowProgressEvent,
        WorkflowProgressSink,
    };

    #[tokio::test]
    async fn recording_sink_records_progress_events_in_order() {
        let sink = RecordingWorkflowProgressSink::new();

        sink.declare_phase("scope", "Scope", 1).await;
        sink.start_phase("search", "Search", 3).await;
        sink.update_phase_counts("search", 1, 0, 0).await;
        sink.complete_phase("search").await;
        sink.fail_phase("extract").await;
        sink.cancel_phase("synthesis").await;
        sink.skip_phase("verify").await;

        assert_eq!(
            sink.events().await,
            vec![
                WorkflowProgressEvent::Declared {
                    id: "scope".to_string(),
                    label: "Scope".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Started {
                    id: "search".to_string(),
                    label: "Search".to_string(),
                    planned_count: 3,
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "search".to_string(),
                    completed_count: 1,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Completed {
                    id: "search".to_string(),
                },
                WorkflowProgressEvent::Failed {
                    id: "extract".to_string(),
                },
                WorkflowProgressEvent::Cancelled {
                    id: "synthesis".to_string(),
                },
                WorkflowProgressEvent::Skipped {
                    id: "verify".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn noop_sink_accepts_progress_events() {
        let sink = NoopWorkflowProgressSink;

        sink.declare_phase("scope", "Scope", 1).await;
        sink.start_phase("search", "Search", 3).await;
        sink.update_phase_counts("search", 1, 0, 0).await;
        sink.complete_phase("search").await;
        sink.fail_phase("extract").await;
        sink.cancel_phase("synthesis").await;
        sink.skip_phase("verify").await;
    }

    #[tokio::test]
    async fn recording_sink_take_events_drains_shared_events() {
        let sink = RecordingWorkflowProgressSink::new();
        let cloned_sink = sink.clone();

        sink.declare_phase("scope", "Scope", 1).await;
        cloned_sink.cancel_phase("search").await;

        assert_eq!(
            cloned_sink.take_events().await,
            vec![
                WorkflowProgressEvent::Declared {
                    id: "scope".to_string(),
                    label: "Scope".to_string(),
                    planned_count: 1,
                },
                WorkflowProgressEvent::Cancelled {
                    id: "search".to_string(),
                },
            ]
        );
        assert!(sink.events().await.is_empty());
    }
}
