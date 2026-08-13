use std::{
    fmt::Display,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mega_memory::ExtractionBatch;
use mega_store::{
    ExtractionJob, ExtractionJobCompletion, ExtractionJobFailure, MemoryStore, StoreError,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Controls what happens to a job already being processed when shutdown is
/// requested. In both modes, no new job is claimed after cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerShutdown {
    /// Finish the current job, commit its completion, then stop.
    Drain,
    /// Cancel the current handler and return the leased job to durable queue.
    Cancel,
}

#[derive(Clone, Debug)]
pub struct ExtractionWorkerConfig {
    pub worker_id: String,
    pub lease_millis: i64,
    pub poll_interval: Duration,
    pub retry_delay: Duration,
}

impl ExtractionWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_millis: 15 * 60 * 1_000,
            poll_interval: DEFAULT_POLL_INTERVAL,
            retry_delay: DEFAULT_RETRY_DELAY,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExtractionWorkerReport {
    pub completed: u32,
    pub failed: u32,
    pub cancelled: bool,
    pub drained: bool,
}

#[derive(Debug, Error)]
pub enum ExtractionWorkerError {
    #[error("invalid extraction worker configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("extraction store lock is poisoned")]
    StoreLockPoisoned,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("system clock is outside the supported Unix millisecond range")]
    Clock,
}

/// Async boundary implemented by the extraction/reconciliation layer.
///
/// The worker owns queue leasing and durable completion. A handler owns model
/// calls and candidate validation, and must observe the supplied token when it
/// performs cancellable work. Handler errors are recorded as bounded metadata
/// by the worker and retried through the store lease policy.
pub trait ExtractionJobHandler: Send + Sync {
    type Error: Display + Send + Sync + 'static;

    fn process(
        &self,
        job: ExtractionJob,
        batches: Vec<ExtractionBatch>,
        cancellation: CancellationToken,
    ) -> impl Future<Output = Result<ExtractionJobCompletion, Self::Error>> + Send;
}

pub struct ExtractionWorker<H> {
    store: Arc<Mutex<MemoryStore>>,
    handler: H,
    config: ExtractionWorkerConfig,
}

impl<H> ExtractionWorker<H>
where
    H: ExtractionJobHandler,
{
    pub fn new(store: Arc<Mutex<MemoryStore>>, handler: H, config: ExtractionWorkerConfig) -> Self {
        Self {
            store,
            handler,
            config,
        }
    }

    /// Runs until cancellation. The default shutdown policy cancels an
    /// in-flight handler and returns its durable lease for retry.
    pub async fn run(
        &self,
        cancellation: CancellationToken,
    ) -> Result<ExtractionWorkerReport, ExtractionWorkerError> {
        self.run_with_shutdown(cancellation, WorkerShutdown::Cancel)
            .await
    }

    pub async fn run_with_shutdown(
        &self,
        cancellation: CancellationToken,
        shutdown: WorkerShutdown,
    ) -> Result<ExtractionWorkerReport, ExtractionWorkerError> {
        self.validate_config()?;
        let mut report = ExtractionWorkerReport::default();

        loop {
            if cancellation.is_cancelled() {
                report.cancelled = true;
                report.drained = shutdown == WorkerShutdown::Drain;
                return Ok(report);
            }

            let now_ms = now_millis()?;
            let job = with_store(&self.store, |store| {
                store.claim_extraction(&self.config.worker_id, now_ms, self.config.lease_millis)
            })?;
            let Some(job) = job else {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        report.cancelled = true;
                        report.drained = shutdown == WorkerShutdown::Drain;
                        return Ok(report);
                    }
                    _ = tokio::time::sleep(self.config.poll_interval) => {}
                }
                continue;
            };

            let batches = match with_store(&self.store, |store| {
                store.load_extraction_batches(&job.id, &job.segment_id)
            }) {
                Ok(batches) => batches,
                Err(error) => {
                    self.fail_job(&job, "batch_load_failed")?;
                    report.failed += 1;
                    if cancellation.is_cancelled() {
                        report.cancelled = true;
                        return Ok(report);
                    }
                    let _ = error;
                    continue;
                }
            };

            // A drain lets the current handler finish even though the
            // application token is cancelled. Cancellation mode gets a child
            // token so handlers can stop their own provider work promptly.
            let job_cancellation = if shutdown == WorkerShutdown::Drain {
                CancellationToken::new()
            } else {
                cancellation.child_token()
            };
            let result = if shutdown == WorkerShutdown::Cancel {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        job_cancellation.cancel();
                        self.cancel_job(&job)?;
                        report.cancelled = true;
                        return Ok(report);
                    }
                    result = self.handler.process(job.clone(), batches, job_cancellation.clone()) => result,
                }
            } else {
                self.handler
                    .process(job.clone(), batches, job_cancellation.clone())
                    .await
            };

            match result {
                Ok(completion) => {
                    let completed_at = now_millis()?;
                    with_store(&self.store, |store| {
                        store.complete_extraction(
                            &job.id,
                            &self.config.worker_id,
                            &completion,
                            completed_at,
                        )
                    })?;
                    report.completed += 1;
                }
                Err(_error) => {
                    self.fail_job(&job, "handler_failed")?;
                    report.failed += 1;
                }
            }
        }
    }

    fn validate_config(&self) -> Result<(), ExtractionWorkerError> {
        if self.config.worker_id.trim().is_empty() || self.config.worker_id.chars().count() > 128 {
            return Err(ExtractionWorkerError::InvalidConfiguration(
                "worker ID must be 1..=128 characters",
            ));
        }
        if self.config.poll_interval.is_zero() || self.config.retry_delay.is_zero() {
            return Err(ExtractionWorkerError::InvalidConfiguration(
                "poll and retry intervals must be non-zero",
            ));
        }
        Ok(())
    }

    fn fail_job(&self, job: &ExtractionJob, error_code: &str) -> Result<(), ExtractionWorkerError> {
        let now_ms = now_millis()?;
        let retry_at = now_ms.saturating_add(duration_millis(self.config.retry_delay)?);
        with_store(&self.store, |store| {
            store
                .fail_extraction(
                    &job.id,
                    &self.config.worker_id,
                    &ExtractionJobFailure {
                        error_code: error_code.to_owned(),
                        retry_at,
                    },
                    now_ms,
                )
                .map(|_| ())
        })
    }

    fn cancel_job(&self, job: &ExtractionJob) -> Result<(), ExtractionWorkerError> {
        let now_ms = now_millis()?;
        with_store(&self.store, |store| {
            store
                .fail_extraction(
                    &job.id,
                    &self.config.worker_id,
                    &ExtractionJobFailure {
                        error_code: "cancelled".to_owned(),
                        retry_at: now_ms,
                    },
                    now_ms,
                )
                .map(|_| ())
        })
    }
}

fn with_store<T>(
    store: &Arc<Mutex<MemoryStore>>,
    operation: impl FnOnce(&mut MemoryStore) -> Result<T, StoreError>,
) -> Result<T, ExtractionWorkerError> {
    let mut store = store
        .lock()
        .map_err(|_| ExtractionWorkerError::StoreLockPoisoned)?;
    operation(&mut store).map_err(ExtractionWorkerError::Store)
}

fn now_millis() -> Result<i64, ExtractionWorkerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ExtractionWorkerError::Clock)?
        .as_millis();
    i64::try_from(millis).map_err(|_| ExtractionWorkerError::Clock)
}

fn duration_millis(duration: Duration) -> Result<i64, ExtractionWorkerError> {
    i64::try_from(duration.as_millis())
        .map_err(|_| ExtractionWorkerError::InvalidConfiguration("interval is too large"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mega_memory::Sensitivity;
    use mega_store::{
        ActivitySegmentInput, MemoryStoreConfig, SegmentCloseReason, SourceEventAdmission,
        SourceEventInput, SourceKind,
    };
    use tempfile::TempDir;
    use tokio::sync::Notify;

    use super::*;

    struct GateHandler {
        started: Arc<Notify>,
        release: Arc<Notify>,
        calls: Arc<AtomicUsize>,
    }

    impl ExtractionJobHandler for GateHandler {
        type Error = std::io::Error;

        fn process(
            &self,
            _job: ExtractionJob,
            _batches: Vec<ExtractionBatch>,
            cancellation: CancellationToken,
        ) -> impl Future<Output = Result<ExtractionJobCompletion, Self::Error>> + Send {
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            let calls = Arc::clone(&self.calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                tokio::select! {
                    _ = release.notified() => Ok(ExtractionJobCompletion {
                        provider: "fixture".into(),
                        model: "test".into(),
                        latency_ms: 1,
                        input_tokens: Some(1),
                        output_tokens: Some(1),
                        private_content_left_device: false,
                    }),
                    _ = cancellation.cancelled() => Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cancelled",
                    )),
                }
            }
        }
    }

    fn store_with_job() -> (Arc<Mutex<MemoryStore>>, TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.sqlite3");
        let mut store = MemoryStore::open(MemoryStoreConfig::encrypted(path, [7; 32])).unwrap();
        let source_id = mega_memory::SourceEventId::new("worker-source");
        assert!(matches!(
            store
                .insert_source_event(&SourceEventInput {
                    id: source_id.clone(),
                    correlation_id: "worker-correlation".into(),
                    source_kind: SourceKind::AssistantConversation,
                    app_id: None,
                    started_at: 1,
                    ended_at: 2,
                    redacted_title: None,
                    evidence_text: "User selected local memory.".into(),
                    sensitivity: Sensitivity::Private,
                    redaction_flags: vec![],
                    capture_sequence: None,
                    ax_sequence: None,
                    created_at: 2,
                })
                .unwrap(),
            SourceEventAdmission::Inserted(_)
        ));
        store
            .insert_activity_segment(&ActivitySegmentInput {
                id: "worker-segment".into(),
                app_id: None,
                scope_id: None,
                started_at: 1,
                ended_at: 2,
                close_reason: SegmentCloseReason::SessionEnded,
                source_event_ids: vec![source_id],
            })
            .unwrap();
        store
            .enqueue_extraction("worker-segment", "memory-v1", "worker-correlation", 3)
            .unwrap();
        (Arc::new(Mutex::new(store)), directory)
    }

    fn worker(
        store: Arc<Mutex<MemoryStore>>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        calls: Arc<AtomicUsize>,
    ) -> ExtractionWorker<GateHandler> {
        ExtractionWorker::new(
            store,
            GateHandler {
                started,
                release,
                calls,
            },
            ExtractionWorkerConfig {
                worker_id: "runtime-worker".into(),
                lease_millis: 5_000,
                poll_interval: Duration::from_millis(1),
                retry_delay: Duration::from_millis(1),
            },
        )
    }

    #[tokio::test]
    async fn drain_finishes_the_claimed_job_before_stopping() {
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker = worker(
            Arc::clone(&store),
            Arc::clone(&started),
            Arc::clone(&release),
            Arc::clone(&calls),
        );
        let cancellation = CancellationToken::new();
        let cancel_for_task = cancellation.clone();
        let task = tokio::spawn(async move {
            worker
                .run_with_shutdown(cancel_for_task, WorkerShutdown::Drain)
                .await
                .unwrap()
        });

        started.notified().await;
        cancellation.cancel();
        release.notify_one();
        let report = task.await.unwrap();
        assert!(report.cancelled);
        assert!(report.drained);
        assert_eq!(report.completed, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_returns_the_claimed_job_for_retry() {
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker = worker(
            Arc::clone(&store),
            Arc::clone(&started),
            Arc::clone(&release),
            Arc::clone(&calls),
        );
        let cancellation = CancellationToken::new();
        let cancel_for_task = cancellation.clone();
        let task = tokio::spawn(async move { worker.run(cancel_for_task).await.unwrap() });

        started.notified().await;
        cancellation.cancel();
        let report = task.await.unwrap();
        assert!(report.cancelled);
        assert_eq!(report.completed, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let retry_check_at = now_millis().unwrap();
        let next_job = store
            .lock()
            .unwrap()
            .claim_extraction("next-worker", retry_check_at, 5_000)
            .unwrap();
        assert!(next_job.is_some());
        drop(release);
    }
}
