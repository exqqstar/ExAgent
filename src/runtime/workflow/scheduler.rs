use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;

#[derive(Debug, Clone, Default)]
pub struct WorkflowCancellation {
    inner: Arc<WorkflowCancellationInner>,
}

#[derive(Debug, Default)]
struct WorkflowCancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl WorkflowCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            self.inner.notify.notified().await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledAgentOutput<T> {
    pub value: T,
    pub tokens_used: Option<i64>,
}

impl<T> ScheduledAgentOutput<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            tokens_used: None,
        }
    }

    pub fn with_tokens(value: T, tokens_used: i64) -> Self {
        Self {
            value,
            tokens_used: Some(tokens_used),
        }
    }
}

type ScheduledTaskFuture<T> =
    Pin<Box<dyn Future<Output = anyhow::Result<ScheduledAgentOutput<T>>> + Send>>;

pub struct ScheduledAgentTask<T> {
    task: Box<dyn FnOnce(WorkflowCancellation) -> ScheduledTaskFuture<T> + Send>,
}

impl<T> ScheduledAgentTask<T> {
    pub fn new<F, Fut>(task: F) -> Self
    where
        F: FnOnce(WorkflowCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<ScheduledAgentOutput<T>>> + Send + 'static,
    {
        Self {
            task: Box::new(move |cancellation| Box::pin(task(cancellation))),
        }
    }

    fn run(self, cancellation: WorkflowCancellation) -> ScheduledTaskFuture<T> {
        (self.task)(cancellation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowScheduleReport<T> {
    pub outputs: Vec<T>,
    pub agent_calls: usize,
    pub failed_agent_calls: usize,
    pub skipped_agent_calls: usize,
    pub tokens_used: Option<i64>,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone)]
pub struct WorkflowScheduler {
    max_concurrency: usize,
}

impl WorkflowScheduler {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency: max_concurrency.max(1),
        }
    }

    pub async fn run<T>(
        &self,
        tasks: Vec<ScheduledAgentTask<T>>,
        cancellation: WorkflowCancellation,
    ) -> WorkflowScheduleReport<T>
    where
        T: Send + 'static,
    {
        let started_at = Instant::now();
        let total_tasks = tasks.len();
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency));
        let mut join_set = JoinSet::new();
        let mut agent_calls = 0;
        let mut skipped_agent_calls = 0;

        for (index, task) in tasks.into_iter().enumerate() {
            if cancellation.is_cancelled() {
                skipped_agent_calls += total_tasks - index;
                break;
            }

            let permit = tokio::select! {
                permit = semaphore.clone().acquire_owned() => {
                    match permit {
                        Ok(permit) => permit,
                        Err(_) => {
                            skipped_agent_calls += total_tasks - index;
                            break;
                        }
                    }
                }
                _ = cancellation.cancelled() => {
                    skipped_agent_calls += total_tasks - index;
                    break;
                }
            };

            agent_calls += 1;
            let task_cancellation = cancellation.clone();
            join_set.spawn(async move {
                let result = task.run(task_cancellation).await;
                drop(permit);
                result
            });
        }

        let mut outputs = Vec::new();
        let mut failed_agent_calls = 0;
        let mut tokens_used = None;

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok(Ok(output)) => {
                    if let Some(tokens) = output.tokens_used {
                        tokens_used = Some(tokens_used.unwrap_or(0) + tokens);
                    }
                    outputs.push(output.value);
                }
                Ok(Err(_)) | Err(_) => {
                    failed_agent_calls += 1;
                }
            }
        }

        WorkflowScheduleReport {
            outputs,
            agent_calls,
            failed_agent_calls,
            skipped_agent_calls,
            tokens_used,
            elapsed_ms: started_at.elapsed().as_millis() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn scheduler_enforces_max_concurrency() {
        let scheduler = WorkflowScheduler::new(2);
        let running = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let running = Arc::clone(&running);
            let observed = Arc::clone(&observed);
            tasks.push(ScheduledAgentTask::new(move |_| async move {
                let now_running = running.fetch_add(1, Ordering::SeqCst) + 1;
                observed.fetch_max(now_running, Ordering::SeqCst);
                sleep(Duration::from_millis(10)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                Ok(ScheduledAgentOutput::new(()))
            }));
        }

        let report = scheduler.run(tasks, WorkflowCancellation::new()).await;

        assert_eq!(report.agent_calls, 8);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 0);
        assert_eq!(observed.load(Ordering::SeqCst), 2);
        assert_eq!(report.outputs.len(), 8);
    }

    #[tokio::test]
    async fn cancellation_skips_tasks_waiting_for_capacity() {
        let scheduler = WorkflowScheduler::new(1);
        let cancellation = WorkflowCancellation::new();
        let started = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for index in 0..5 {
            let cancellation = cancellation.clone();
            let started = Arc::clone(&started);
            tasks.push(ScheduledAgentTask::new(move |_| async move {
                started.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    cancellation.cancel();
                    sleep(Duration::from_millis(20)).await;
                }
                Ok(ScheduledAgentOutput::new(index))
            }));
        }

        let report = scheduler.run(tasks, cancellation).await;

        assert_eq!(report.agent_calls, 1);
        assert_eq!(report.skipped_agent_calls, 4);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(report.outputs, vec![0]);
    }
}
