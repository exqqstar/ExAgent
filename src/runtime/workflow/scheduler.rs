use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Notify;
use tokio::task::JoinSet;

use super::progress::WorkflowProgressSink;

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
    input_index: usize,
    pub value: T,
    pub tokens_used: Option<i64>,
}

impl<T> ScheduledAgentOutput<T> {
    pub fn new(value: T) -> Self {
        Self {
            input_index: 0,
            value,
            tokens_used: None,
        }
    }

    pub fn with_tokens(value: T, tokens_used: i64) -> Self {
        Self {
            input_index: 0,
            value,
            tokens_used: Some(tokens_used),
        }
    }

    fn with_input_index(mut self, input_index: usize) -> Self {
        self.input_index = input_index;
        self
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
    pub control_stopped: bool,
    pub tokens_used: Option<i64>,
    pub elapsed_ms: i64,
}

pub trait WorkflowScheduleController: Send + Sync {
    fn should_schedule(&self) -> bool;

    fn record_task_tokens(&self, tokens_used: Option<i64>) -> bool;
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
        self.run_inner(tasks, cancellation, None, None).await
    }

    pub async fn run_phase<T>(
        &self,
        phase_id: &str,
        phase_label: &str,
        tasks: Vec<ScheduledAgentTask<T>>,
        cancellation: WorkflowCancellation,
        progress_sink: &(dyn WorkflowProgressSink + '_),
    ) -> WorkflowScheduleReport<T>
    where
        T: Send + 'static,
    {
        self.run_inner(
            tasks,
            cancellation,
            Some(PhaseProgress {
                id: phase_id,
                label: phase_label,
                sink: progress_sink,
            }),
            None,
        )
        .await
    }

    pub async fn run_phase_controlled<T>(
        &self,
        phase_id: &str,
        phase_label: &str,
        tasks: Vec<ScheduledAgentTask<T>>,
        cancellation: WorkflowCancellation,
        progress_sink: &(dyn WorkflowProgressSink + '_),
        controller: &(dyn WorkflowScheduleController + '_),
    ) -> WorkflowScheduleReport<T>
    where
        T: Send + 'static,
    {
        self.run_inner(
            tasks,
            cancellation,
            Some(PhaseProgress {
                id: phase_id,
                label: phase_label,
                sink: progress_sink,
            }),
            Some(controller),
        )
        .await
    }

    async fn run_inner<T>(
        &self,
        tasks: Vec<ScheduledAgentTask<T>>,
        cancellation: WorkflowCancellation,
        progress: Option<PhaseProgress<'_>>,
        controller: Option<&(dyn WorkflowScheduleController + '_)>,
    ) -> WorkflowScheduleReport<T>
    where
        T: Send + 'static,
    {
        let started_at = Instant::now();
        let total_tasks = tasks.len();
        let mut join_set = JoinSet::new();
        let mut agent_calls = 0;
        let mut skipped_agent_calls = 0;
        let mut completed_agent_calls = 0;
        let mut failed_agent_calls = 0;
        let mut tokens_used = None;
        let mut outputs = Vec::new();
        let mut task_iter = tasks.into_iter().enumerate();
        let mut scheduling_finished = false;
        let mut control_stopped = false;

        if let Some(progress) = progress.as_ref() {
            progress
                .sink
                .start_phase(progress.id, progress.label, total_tasks)
                .await;
        }

        loop {
            while !scheduling_finished && join_set.len() < self.max_concurrency {
                if cancellation.is_cancelled() {
                    skipped_agent_calls += task_iter.len();
                    scheduling_finished = true;
                    if let Some(progress) = progress.as_ref() {
                        progress
                            .sink
                            .update_phase_counts(
                                progress.id,
                                completed_agent_calls,
                                failed_agent_calls,
                                skipped_agent_calls,
                            )
                            .await;
                    }
                    break;
                }

                let Some((index, task)) = task_iter.next() else {
                    scheduling_finished = true;
                    break;
                };

                if let Some(controller) = controller {
                    if !controller.should_schedule() {
                        skipped_agent_calls += task_iter.len() + 1;
                        scheduling_finished = true;
                        control_stopped = true;
                        if let Some(progress) = progress.as_ref() {
                            progress
                                .sink
                                .update_phase_counts(
                                    progress.id,
                                    completed_agent_calls,
                                    failed_agent_calls,
                                    skipped_agent_calls,
                                )
                                .await;
                        }
                        break;
                    }
                }

                agent_calls += 1;
                let task_cancellation = cancellation.clone();
                join_set.spawn(async move {
                    task.run(task_cancellation)
                        .await
                        .map(|output| output.with_input_index(index))
                });
            }

            if join_set.is_empty() {
                break;
            }

            let join_result = if scheduling_finished {
                join_set.join_next().await
            } else {
                tokio::select! {
                    join_result = join_set.join_next() => join_result,
                    _ = cancellation.cancelled() => {
                        skipped_agent_calls += task_iter.len();
                        scheduling_finished = true;
                        if let Some(progress) = progress.as_ref() {
                            progress
                                .sink
                                .update_phase_counts(
                                    progress.id,
                                    completed_agent_calls,
                                    failed_agent_calls,
                                    skipped_agent_calls,
                                )
                                .await;
                        }
                        continue;
                    }
                }
            };

            match join_result {
                Some(Ok(Ok(output))) => {
                    let tokens_used_for_output = output.tokens_used;
                    if let Some(tokens) = tokens_used_for_output {
                        tokens_used = Some(tokens_used.unwrap_or(0) + tokens);
                    }
                    outputs.push(output);
                    completed_agent_calls += 1;
                    if let Some(controller) = controller {
                        let should_continue = controller.record_task_tokens(tokens_used_for_output);
                        if !scheduling_finished && !should_continue {
                            skipped_agent_calls += task_iter.len();
                            scheduling_finished = true;
                            control_stopped = true;
                        }
                    }
                }
                Some(Ok(Err(_))) | Some(Err(_)) => {
                    failed_agent_calls += 1;
                }
                None => {}
            }

            if let Some(progress) = progress.as_ref() {
                progress
                    .sink
                    .update_phase_counts(
                        progress.id,
                        completed_agent_calls,
                        failed_agent_calls,
                        skipped_agent_calls,
                    )
                    .await;
            }
        }

        outputs.sort_by_key(|output| output.input_index);
        let outputs = outputs.into_iter().map(|output| output.value).collect();

        if let Some(progress) = progress.as_ref() {
            if cancellation.is_cancelled() {
                progress.sink.cancel_phase(progress.id).await;
            } else if control_stopped {
                progress.sink.fail_phase(progress.id).await;
            } else if total_tasks == 0 && skipped_agent_calls == 0 && agent_calls == 0 {
                progress.sink.skip_phase(progress.id).await;
            } else if failed_agent_calls > 0 {
                progress.sink.fail_phase(progress.id).await;
            } else {
                progress.sink.complete_phase(progress.id).await;
            }
        }

        WorkflowScheduleReport {
            outputs,
            agent_calls,
            failed_agent_calls,
            skipped_agent_calls,
            control_stopped,
            tokens_used,
            elapsed_ms: started_at.elapsed().as_millis() as i64,
        }
    }
}

struct PhaseProgress<'a> {
    id: &'a str,
    label: &'a str,
    sink: &'a dyn WorkflowProgressSink,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::workflow::progress::{
        RecordingWorkflowProgressSink, WorkflowProgressEvent,
    };
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::oneshot;
    use tokio::time::{sleep, timeout, Duration};

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

    #[tokio::test]
    async fn scheduler_preserves_input_order_when_tasks_finish_out_of_order() {
        let scheduler = WorkflowScheduler::new(3);

        let mut tasks = Vec::new();
        for (value, delay_ms) in [(0, 30), (1, 5), (2, 10)] {
            tasks.push(ScheduledAgentTask::new(move |_| async move {
                sleep(Duration::from_millis(delay_ms)).await;
                Ok(ScheduledAgentOutput::new(value))
            }));
        }

        let report = scheduler.run(tasks, WorkflowCancellation::new()).await;

        assert_eq!(report.outputs, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn run_phase_updates_counts_as_tasks_finish() {
        let scheduler = WorkflowScheduler::new(2);
        let progress = RecordingWorkflowProgressSink::new();

        let mut tasks = Vec::new();
        for delay_ms in [20, 5, 10] {
            tasks.push(ScheduledAgentTask::new(move |_| async move {
                sleep(Duration::from_millis(delay_ms)).await;
                Ok(ScheduledAgentOutput::new(delay_ms))
            }));
        }

        let report = scheduler
            .run_phase(
                "search",
                "Search",
                tasks,
                WorkflowCancellation::new(),
                &progress,
            )
            .await;

        assert_eq!(report.outputs, vec![20, 5, 10]);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 0);
        assert_eq!(
            progress.events().await,
            vec![
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
                WorkflowProgressEvent::CountsUpdated {
                    id: "search".to_string(),
                    completed_count: 2,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::CountsUpdated {
                    id: "search".to_string(),
                    completed_count: 3,
                    failed_count: 0,
                    skipped_count: 0,
                },
                WorkflowProgressEvent::Completed {
                    id: "search".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn run_phase_counts_failures_without_dropping_successes() {
        let scheduler = WorkflowScheduler::new(3);
        let progress = RecordingWorkflowProgressSink::new();

        let tasks = vec![
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::new("first")) }),
            ScheduledAgentTask::new(|_| async { Err(anyhow::anyhow!("failed")) }),
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::new("third")) }),
        ];

        let report = scheduler
            .run_phase(
                "extract",
                "Extract",
                tasks,
                WorkflowCancellation::new(),
                &progress,
            )
            .await;

        assert_eq!(report.outputs, vec!["first", "third"]);
        assert_eq!(report.agent_calls, 3);
        assert_eq!(report.failed_agent_calls, 1);
        assert_eq!(report.skipped_agent_calls, 0);
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::CountsUpdated {
                id: "extract".to_string(),
                completed_count: 2,
                failed_count: 1,
                skipped_count: 0,
            }));
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::Failed {
                id: "extract".to_string(),
            }));
    }

    #[tokio::test]
    async fn run_phase_controlled_stops_scheduling_queued_work() {
        let scheduler = WorkflowScheduler::new(1);
        let progress = RecordingWorkflowProgressSink::new();
        let controller = StopAfterFirstTask::default();

        let tasks = vec![
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::with_tokens(0, 10)) }),
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::with_tokens(1, 10)) }),
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::with_tokens(2, 10)) }),
        ];

        let report = scheduler
            .run_phase_controlled(
                "verify",
                "Verify",
                tasks,
                WorkflowCancellation::new(),
                &progress,
                &controller,
            )
            .await;

        assert_eq!(report.outputs, vec![0]);
        assert_eq!(report.agent_calls, 1);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 2);
        assert!(report.control_stopped);
        assert_eq!(report.tokens_used, Some(10));
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::Failed {
                id: "verify".to_string(),
            }));
    }

    #[tokio::test]
    async fn run_phase_controlled_records_tokens_from_in_flight_work_after_stop() {
        let scheduler = WorkflowScheduler::new(2);
        let progress = RecordingWorkflowProgressSink::new();
        let controller = StopAfterFirstTask::default();

        let tasks = vec![
            ScheduledAgentTask::new(|_| async {
                sleep(Duration::from_millis(5)).await;
                Ok(ScheduledAgentOutput::with_tokens(0, 10))
            }),
            ScheduledAgentTask::new(|_| async {
                sleep(Duration::from_millis(20)).await;
                Ok(ScheduledAgentOutput::with_tokens(1, 20))
            }),
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::with_tokens(2, 30)) }),
        ];

        let report = scheduler
            .run_phase_controlled(
                "verify",
                "Verify",
                tasks,
                WorkflowCancellation::new(),
                &progress,
                &controller,
            )
            .await;

        assert_eq!(report.outputs, vec![0, 1]);
        assert_eq!(report.agent_calls, 2);
        assert_eq!(report.skipped_agent_calls, 1);
        assert!(report.control_stopped);
        assert_eq!(report.tokens_used, Some(30));
        assert_eq!(controller.tokens_used.load(Ordering::SeqCst), 30);
    }

    #[tokio::test]
    async fn run_phase_cancellation_skips_queued_work_and_marks_progress() {
        let scheduler = WorkflowScheduler::new(1);
        let cancellation = WorkflowCancellation::new();
        let progress = RecordingWorkflowProgressSink::new();
        let (finish_tx, finish_rx) = oneshot::channel::<()>();
        let finish_rx = Arc::new(tokio::sync::Mutex::new(Some(finish_rx)));

        let mut tasks = Vec::new();
        for index in 0..4 {
            let cancellation = cancellation.clone();
            let finish_rx = Arc::clone(&finish_rx);
            tasks.push(ScheduledAgentTask::new(move |_| async move {
                if index == 0 {
                    cancellation.cancel();
                    if let Some(rx) = finish_rx.lock().await.take() {
                        let _ = rx.await;
                    }
                }
                Ok(ScheduledAgentOutput::new(index))
            }));
        }

        let run = scheduler.run_phase("verify", "Verify", tasks, cancellation, &progress);
        tokio::pin!(run);

        timeout(Duration::from_secs(1), async {
            loop {
                tokio::select! {
                    _ = &mut run => panic!("phase finished before queued work was released"),
                    _ = sleep(Duration::from_millis(1)) => {
                        if progress.events().await.iter().any(|event| {
                            matches!(
                                event,
                                WorkflowProgressEvent::CountsUpdated {
                                    id,
                                    completed_count: 0,
                                    failed_count: 0,
                                    skipped_count: 3,
                                } if id == "verify"
                            )
                        }) {
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("queued work was not marked skipped");

        finish_tx.send(()).unwrap();
        let report = run.await;

        assert_eq!(report.agent_calls, 1);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 3);
        assert_eq!(report.outputs, vec![0]);
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::Cancelled {
                id: "verify".to_string(),
            }));
    }

    #[tokio::test]
    async fn run_phase_marks_cancelled_even_when_no_tasks_were_skipped() {
        let scheduler = WorkflowScheduler::new(2);
        let cancellation = WorkflowCancellation::new();
        let progress = RecordingWorkflowProgressSink::new();

        let tasks = vec![
            ScheduledAgentTask::new({
                let cancellation = cancellation.clone();
                move |_| async move {
                    cancellation.cancel();
                    Ok(ScheduledAgentOutput::new("first"))
                }
            }),
            ScheduledAgentTask::new(|_| async { Ok(ScheduledAgentOutput::new("second")) }),
        ];

        let report = scheduler
            .run_phase("verify", "Verify", tasks, cancellation, &progress)
            .await;

        assert_eq!(report.agent_calls, 2);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 0);
        assert_eq!(report.outputs, vec!["first", "second"]);
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::Cancelled {
                id: "verify".to_string(),
            }));
    }

    #[tokio::test]
    async fn run_phase_marks_empty_phase_cancelled_when_token_is_cancelled() {
        let scheduler = WorkflowScheduler::new(2);
        let cancellation = WorkflowCancellation::new();
        cancellation.cancel();
        let progress = RecordingWorkflowProgressSink::new();

        let report = scheduler
            .run_phase::<()>("search", "Search", Vec::new(), cancellation, &progress)
            .await;

        assert_eq!(report.agent_calls, 0);
        assert_eq!(report.failed_agent_calls, 0);
        assert_eq!(report.skipped_agent_calls, 0);
        assert!(progress
            .events()
            .await
            .contains(&WorkflowProgressEvent::Cancelled {
                id: "search".to_string(),
            }));
    }

    #[derive(Default)]
    struct StopAfterFirstTask {
        completed: AtomicUsize,
        tokens_used: AtomicI64,
    }

    impl WorkflowScheduleController for StopAfterFirstTask {
        fn should_schedule(&self) -> bool {
            true
        }

        fn record_task_tokens(&self, tokens_used: Option<i64>) -> bool {
            if let Some(tokens_used) = tokens_used {
                self.tokens_used.fetch_add(tokens_used, Ordering::SeqCst);
            }
            self.completed.fetch_add(1, Ordering::SeqCst);
            false
        }
    }
}
