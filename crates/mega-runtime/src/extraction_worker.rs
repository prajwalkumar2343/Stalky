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
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_HANDLER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LEASE_MILLIS: i64 = 15 * 60 * 1_000;
const MIN_LEASE_MILLIS: i64 = 1_000;

/// Controls what happens to a job already being processed when shutdown is
/// requested. In both modes, no new job is claimed after cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerShutdown {
    /// Finish the current job, subject to the drain deadline, then stop.
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
    /// How often a running job renews its owner-checked lease.
    pub heartbeat_interval: Duration,
    /// Maximum time allowed for one handler invocation.
    pub handler_timeout: Duration,
    /// Maximum time allowed to finish the current job after drain begins.
    pub drain_timeout: Duration,
}

impl ExtractionWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_millis: MAX_LEASE_MILLIS,
            poll_interval: DEFAULT_POLL_INTERVAL,
            retry_delay: DEFAULT_RETRY_DELAY,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            handler_timeout: DEFAULT_HANDLER_TIMEOUT,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExtractionWorkerReport {
    pub completed: u32,
    pub failed: u32,
    pub timed_out: u32,
    pub lease_lost: u32,
    pub cancelled: bool,
    pub drained: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionWorkerHealthStatus {
    Starting,
    Healthy,
    Degraded,
    Stopping,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionWorkerPhase {
    Idle,
    Processing,
    Draining,
}

/// A bounded, metadata-only view of the worker state suitable for a health
/// endpoint or watch channel. It intentionally contains no extraction input,
/// handler output, or error text supplied by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionWorkerHealthSnapshot {
    pub worker_id: String,
    pub status: ExtractionWorkerHealthStatus,
    pub phase: ExtractionWorkerPhase,
    pub current_job_id: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub last_heartbeat_at: Option<i64>,
    pub completed: u64,
    pub failed: u64,
    pub timed_out: u64,
    pub lease_lost: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Error)]
pub enum ExtractionWorkerError {
    #[error("invalid extraction worker configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("extraction store lock is poisoned")]
    StoreLockPoisoned,
    #[error("extraction worker health lock is poisoned")]
    HealthLockPoisoned,
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
    health: Arc<Mutex<ExtractionWorkerHealthSnapshot>>,
}

#[derive(Debug)]
enum JobExecution {
    Completed(ExtractionJobCompletion),
    HandlerFailed,
    Cancelled,
    HandlerTimeout,
    DrainTimeout,
    LeaseLost,
}

impl<H> ExtractionWorker<H>
where
    H: ExtractionJobHandler,
{
    pub fn new(store: Arc<Mutex<MemoryStore>>, handler: H, config: ExtractionWorkerConfig) -> Self {
        let health = ExtractionWorkerHealthSnapshot {
            worker_id: config.worker_id.clone(),
            status: ExtractionWorkerHealthStatus::Starting,
            phase: ExtractionWorkerPhase::Idle,
            current_job_id: None,
            lease_expires_at: None,
            last_heartbeat_at: None,
            completed: 0,
            failed: 0,
            timed_out: 0,
            lease_lost: 0,
            last_error: None,
        };
        Self {
            store,
            handler,
            config,
            health: Arc::new(Mutex::new(health)),
        }
    }

    pub fn health_snapshot(&self) -> Result<ExtractionWorkerHealthSnapshot, ExtractionWorkerError> {
        self.health
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| ExtractionWorkerError::HealthLockPoisoned)
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
        self.update_health(|health| {
            health.status = ExtractionWorkerHealthStatus::Healthy;
            health.phase = ExtractionWorkerPhase::Idle;
            health.last_error = None;
        })?;
        let mut report = ExtractionWorkerReport::default();

        loop {
            if cancellation.is_cancelled() {
                report.cancelled = true;
                report.drained = shutdown == WorkerShutdown::Drain;
                return self.finish_report(report);
            }

            let now_ms = now_millis()?;
            let job = match with_store(&self.store, |store| {
                store.claim_extraction(&self.config.worker_id, now_ms, self.config.lease_millis)
            }) {
                Ok(job) => job,
                Err(error) => return self.fail_run(error),
            };
            let Some(job) = job else {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        report.cancelled = true;
                        report.drained = shutdown == WorkerShutdown::Drain;
                        return self.finish_report(report);
                    }
                    _ = tokio::time::sleep(self.config.poll_interval) => {}
                }
                continue;
            };

            self.begin_job(&job)?;
            let batches = match with_store(&self.store, |store| {
                store.load_extraction_batches(&job.id, &job.segment_id)
            }) {
                Ok(batches) => batches,
                Err(error) => {
                    let lease_lost =
                        self.release_or_record_loss(&job, "batch_load_failed", true)?;
                    if lease_lost {
                        report.lease_lost += 1;
                    } else {
                        report.failed += 1;
                        self.record_failure("batch_load_failed")?;
                    }
                    if cancellation.is_cancelled() {
                        report.cancelled = true;
                        return self.finish_report(report);
                    }
                    let _ = error;
                    continue;
                }
            };

            let execution = match self
                .process_job(job.clone(), batches, cancellation.clone(), shutdown)
                .await
            {
                Ok(execution) => execution,
                Err(error) => return self.fail_run(error),
            };

            match execution {
                JobExecution::Completed(completion) => {
                    let completed_at = now_millis()?;
                    match with_store(&self.store, |store| {
                        store.complete_extraction(
                            &job.id,
                            &self.config.worker_id,
                            &completion,
                            completed_at,
                        )
                    }) {
                        Ok(()) => {
                            report.completed += 1;
                            self.record_completed()?;
                        }
                        Err(error) if is_lease_lost(&error) => {
                            report.lease_lost += 1;
                            self.record_lease_lost()?;
                        }
                        Err(error) => {
                            return self.fail_run(error);
                        }
                    }
                }
                JobExecution::HandlerFailed => {
                    if shutdown == WorkerShutdown::Cancel && cancellation.is_cancelled() {
                        let lease_lost = self.release_or_record_loss(&job, "cancelled", true)?;
                        if lease_lost {
                            report.lease_lost += 1;
                        } else {
                            self.record_cancelled()?;
                        }
                        report.cancelled = true;
                        return self.finish_report(report);
                    }
                    let lease_lost = self.release_or_record_loss(&job, "handler_failed", false)?;
                    if lease_lost {
                        report.lease_lost += 1;
                    } else {
                        report.failed += 1;
                        self.record_failure("handler_failed")?;
                    }
                }
                JobExecution::Cancelled => {
                    let lease_lost = self.release_or_record_loss(&job, "cancelled", true)?;
                    if lease_lost {
                        report.lease_lost += 1;
                    } else {
                        self.record_cancelled()?;
                    }
                    report.cancelled = true;
                    return self.finish_report(report);
                }
                JobExecution::HandlerTimeout => {
                    let lease_lost = self.release_or_record_loss(&job, "handler_timeout", false)?;
                    report.timed_out += 1;
                    if lease_lost {
                        report.lease_lost += 1;
                    } else {
                        report.failed += 1;
                        self.record_timeout("handler_timeout")?;
                    }
                    if cancellation.is_cancelled() {
                        report.cancelled = true;
                        return self.finish_report(report);
                    }
                }
                JobExecution::DrainTimeout => {
                    let lease_lost = self.release_or_record_loss(&job, "drain_timeout", false)?;
                    report.timed_out += 1;
                    if lease_lost {
                        report.lease_lost += 1;
                    } else {
                        report.failed += 1;
                        self.record_timeout("drain_timeout")?;
                    }
                    report.cancelled = true;
                    report.drained = false;
                    return self.finish_report(report);
                }
                JobExecution::LeaseLost => {
                    report.lease_lost += 1;
                    self.record_lease_lost()?;
                }
            }
        }
    }

    async fn process_job(
        &self,
        job: ExtractionJob,
        batches: Vec<ExtractionBatch>,
        application_cancellation: CancellationToken,
        shutdown: WorkerShutdown,
    ) -> Result<JobExecution, ExtractionWorkerError> {
        let handler_cancellation = if shutdown == WorkerShutdown::Drain {
            CancellationToken::new()
        } else {
            application_cancellation.child_token()
        };
        let heartbeat_stop = CancellationToken::new();
        let heartbeat_job_id = job.id.clone();
        let mut heartbeat =
            Box::pin(self.heartbeat_loop(&heartbeat_job_id, heartbeat_stop.clone()));
        let mut handler = Box::pin(self.handler.process(
            job,
            batches,
            handler_cancellation.clone(),
        ));
        let mut handler_deadline = Box::pin(tokio::time::sleep(self.config.handler_timeout));
        let mut drain_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(24 * 60 * 60)));
        let mut drain_started =
            shutdown == WorkerShutdown::Drain && application_cancellation.is_cancelled();
        if drain_started {
            drain_deadline
                .as_mut()
                .reset(Instant::now() + self.config.drain_timeout);
            self.begin_drain()?;
        }

        let execution = loop {
            tokio::select! {
                biased;
                heartbeat_result = &mut heartbeat => {
                    match heartbeat_result {
                        Ok(()) => break JobExecution::LeaseLost,
                        Err(error) if is_lease_lost(&error) => break JobExecution::LeaseLost,
                        Err(error) => return Err(error),
                    }
                }
                result = &mut handler => {
                    break match result {
                        Ok(completion) => JobExecution::Completed(completion),
                        Err(_error) => JobExecution::HandlerFailed,
                    };
                }
                _ = &mut handler_deadline => {
                    handler_cancellation.cancel();
                    break JobExecution::HandlerTimeout;
                }
                _ = application_cancellation.cancelled(), if shutdown == WorkerShutdown::Cancel => {
                    handler_cancellation.cancel();
                    break JobExecution::Cancelled;
                }
                _ = application_cancellation.cancelled(), if shutdown == WorkerShutdown::Drain && !drain_started => {
                    drain_started = true;
                    drain_deadline
                        .as_mut()
                        .reset(Instant::now() + self.config.drain_timeout);
                    self.begin_drain()?;
                }
                _ = &mut drain_deadline, if drain_started => {
                    handler_cancellation.cancel();
                    break JobExecution::DrainTimeout;
                }
            }
        };

        handler_cancellation.cancel();
        heartbeat_stop.cancel();
        if let Err(error) = heartbeat.await {
            if is_lease_lost(&error) {
                return Ok(JobExecution::LeaseLost);
            }
            return Err(error);
        }
        Ok(execution)
    }

    async fn heartbeat_loop(
        &self,
        job_id: &str,
        stop: CancellationToken,
    ) -> Result<(), ExtractionWorkerError> {
        let mut interval = tokio::time::interval(self.config.heartbeat_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                _ = interval.tick() => {
                    let now_ms = now_millis()?;
                    with_store(&self.store, |store| {
                        store.renew_extraction_lease(
                            job_id,
                            &self.config.worker_id,
                            now_ms,
                            self.config.lease_millis,
                        )
                    })?;
                    let lease_expires_at = now_ms
                        .checked_add(self.config.lease_millis)
                        .ok_or(ExtractionWorkerError::Clock)?;
                    self.update_health(|health| {
                        health.last_heartbeat_at = Some(now_ms);
                        health.lease_expires_at = Some(lease_expires_at);
                        health.status = ExtractionWorkerHealthStatus::Healthy;
                        health.last_error = None;
                    })?;
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
        if !(MIN_LEASE_MILLIS..=MAX_LEASE_MILLIS).contains(&self.config.lease_millis) {
            return Err(ExtractionWorkerError::InvalidConfiguration(
                "lease must be between one second and fifteen minutes",
            ));
        }
        if self.config.poll_interval.is_zero()
            || self.config.retry_delay.is_zero()
            || self.config.heartbeat_interval.is_zero()
            || self.config.handler_timeout.is_zero()
            || self.config.drain_timeout.is_zero()
        {
            return Err(ExtractionWorkerError::InvalidConfiguration(
                "poll, retry, heartbeat, handler, and drain intervals must be non-zero",
            ));
        }
        let lease_duration = Duration::from_millis(
            u64::try_from(self.config.lease_millis)
                .map_err(|_| ExtractionWorkerError::InvalidConfiguration("invalid lease"))?,
        );
        if self.config.heartbeat_interval >= lease_duration {
            return Err(ExtractionWorkerError::InvalidConfiguration(
                "heartbeat interval must be shorter than the lease",
            ));
        }
        let _ = duration_millis(self.config.retry_delay)?;
        Ok(())
    }

    fn begin_job(&self, job: &ExtractionJob) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.status = ExtractionWorkerHealthStatus::Healthy;
            health.phase = ExtractionWorkerPhase::Processing;
            health.current_job_id = Some(job.id.clone());
            health.lease_expires_at = job.lease_expires_at;
            health.last_heartbeat_at = None;
            health.last_error = None;
        })
    }

    fn begin_drain(&self) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.phase = ExtractionWorkerPhase::Draining;
        })
    }

    fn record_completed(&self) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.completed = health.completed.saturating_add(1);
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
            health.status = ExtractionWorkerHealthStatus::Healthy;
            health.last_error = None;
        })
    }

    fn record_failure(&self, error_code: &str) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.failed = health.failed.saturating_add(1);
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
            health.status = ExtractionWorkerHealthStatus::Degraded;
            health.last_error = Some(error_code.to_owned());
        })
    }

    fn record_timeout(&self, error_code: &str) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.failed = health.failed.saturating_add(1);
            health.timed_out = health.timed_out.saturating_add(1);
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
            health.status = ExtractionWorkerHealthStatus::Degraded;
            health.last_error = Some(error_code.to_owned());
        })
    }

    fn record_cancelled(&self) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
        })
    }

    fn record_lease_lost(&self) -> Result<(), ExtractionWorkerError> {
        self.update_health(|health| {
            health.lease_lost = health.lease_lost.saturating_add(1);
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
            health.status = ExtractionWorkerHealthStatus::Degraded;
            health.last_error = Some("lease_lost".to_owned());
        })
    }

    fn release_or_record_loss(
        &self,
        job: &ExtractionJob,
        error_code: &str,
        immediate: bool,
    ) -> Result<bool, ExtractionWorkerError> {
        match self.release_job(job, error_code, immediate) {
            Ok(()) => Ok(false),
            Err(error) if is_lease_lost(&error) => {
                self.record_lease_lost()?;
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    fn release_job(
        &self,
        job: &ExtractionJob,
        error_code: &str,
        immediate: bool,
    ) -> Result<(), ExtractionWorkerError> {
        let now_ms = now_millis()?;
        let retry_at = if immediate {
            now_ms
        } else {
            now_ms.saturating_add(duration_millis(self.config.retry_delay)?)
        };
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

    fn finish_report(
        &self,
        report: ExtractionWorkerReport,
    ) -> Result<ExtractionWorkerReport, ExtractionWorkerError> {
        self.update_health(|health| {
            health.status = ExtractionWorkerHealthStatus::Stopping;
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
        })?;
        self.update_health(|health| {
            health.status = ExtractionWorkerHealthStatus::Stopped;
        })?;
        Ok(report)
    }

    fn fail_run<T>(&self, error: ExtractionWorkerError) -> Result<T, ExtractionWorkerError> {
        self.update_health(|health| {
            health.status = ExtractionWorkerHealthStatus::Degraded;
            health.phase = ExtractionWorkerPhase::Idle;
            health.current_job_id = None;
            health.lease_expires_at = None;
            health.last_heartbeat_at = None;
            health.last_error = Some("worker_error".to_owned());
        })?;
        Err(error)
    }

    fn update_health(
        &self,
        update: impl FnOnce(&mut ExtractionWorkerHealthSnapshot),
    ) -> Result<(), ExtractionWorkerError> {
        let mut health = self
            .health
            .lock()
            .map_err(|_| ExtractionWorkerError::HealthLockPoisoned)?;
        update(&mut health);
        Ok(())
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

fn is_lease_lost(error: &ExtractionWorkerError) -> bool {
    matches!(error, ExtractionWorkerError::Store(StoreError::LeaseLost))
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

    struct PendingHandler {
        started: Arc<Notify>,
    }

    impl ExtractionJobHandler for PendingHandler {
        type Error = std::io::Error;

        fn process(
            &self,
            _job: ExtractionJob,
            _batches: Vec<ExtractionBatch>,
            _cancellation: CancellationToken,
        ) -> impl Future<Output = Result<ExtractionJobCompletion, Self::Error>> + Send {
            let started = Arc::clone(&self.started);
            async move {
                started.notify_one();
                std::future::pending::<Result<ExtractionJobCompletion, std::io::Error>>().await
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

    fn worker_config() -> ExtractionWorkerConfig {
        ExtractionWorkerConfig {
            worker_id: "runtime-worker".into(),
            lease_millis: 5_000,
            poll_interval: Duration::from_millis(1),
            retry_delay: Duration::from_millis(1),
            heartbeat_interval: Duration::from_millis(1),
            handler_timeout: Duration::from_secs(5),
            drain_timeout: Duration::from_secs(5),
        }
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
            worker_config(),
        )
    }

    #[tokio::test]
    async fn heartbeat_keeps_a_long_handler_lease_alive() {
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
        tokio::time::sleep(Duration::from_millis(25)).await;
        release.notify_one();
        cancellation.cancel();
        let report = task.await.unwrap();

        assert_eq!(report.completed, 1);
        assert_eq!(report.lease_lost, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn handler_timeout_releases_job_for_recovery() {
        tokio::time::pause();
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let worker = ExtractionWorker::new(
            Arc::clone(&store),
            PendingHandler {
                started: Arc::clone(&started),
            },
            ExtractionWorkerConfig {
                handler_timeout: Duration::from_millis(20),
                ..worker_config()
            },
        );
        let cancellation = CancellationToken::new();
        let cancel_for_task = cancellation.clone();
        let task = tokio::spawn(async move { worker.run(cancel_for_task).await.unwrap() });
        started.notified().await;
        tokio::time::advance(Duration::from_millis(21)).await;
        cancellation.cancel();
        let report = task.await.unwrap();

        assert_eq!(report.timed_out, 1);
        assert_eq!(report.failed, 1);
        let retry = store
            .lock()
            .unwrap()
            .claim_extraction("recovery-worker", now_millis().unwrap(), 5_000)
            .unwrap();
        assert!(retry.is_some());
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
    async fn drain_timeout_releases_job_and_reports_not_drained() {
        tokio::time::pause();
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let worker = ExtractionWorker::new(
            Arc::clone(&store),
            PendingHandler {
                started: Arc::clone(&started),
            },
            ExtractionWorkerConfig {
                handler_timeout: Duration::from_secs(30),
                drain_timeout: Duration::from_millis(20),
                ..worker_config()
            },
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
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(21)).await;
        let report = task.await.unwrap();

        assert!(report.cancelled);
        assert!(!report.drained);
        assert_eq!(report.timed_out, 1);
        let retry = store
            .lock()
            .unwrap()
            .claim_extraction("recovery-worker", now_millis().unwrap(), 5_000)
            .unwrap();
        assert!(retry.is_some());
    }

    #[tokio::test]
    async fn cancellation_returns_the_claimed_job_for_retry() {
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker = Arc::new(worker(
            Arc::clone(&store),
            Arc::clone(&started),
            Arc::clone(&release),
            Arc::clone(&calls),
        ));
        let cancellation = CancellationToken::new();
        let cancel_for_task = cancellation.clone();
        let worker_for_task = Arc::clone(&worker);
        let task = tokio::spawn(async move { worker_for_task.run(cancel_for_task).await.unwrap() });

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

    #[tokio::test]
    async fn health_snapshot_recovers_after_successful_retry() {
        let (store, _directory) = store_with_job();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker = Arc::new(worker(
            Arc::clone(&store),
            Arc::clone(&started),
            Arc::clone(&release),
            Arc::clone(&calls),
        ));
        let cancellation = CancellationToken::new();
        let cancel_for_task = cancellation.clone();
        let worker_for_task = Arc::clone(&worker);
        let task = tokio::spawn(async move { worker_for_task.run(cancel_for_task).await.unwrap() });
        started.notified().await;
        cancellation.cancel();
        let report = task.await.unwrap();
        assert!(report.cancelled);

        let health = worker.health_snapshot().unwrap();
        assert_eq!(health.status, ExtractionWorkerHealthStatus::Stopped);
        assert_eq!(health.lease_lost, 0);
        assert_eq!(health.failed, 0);
    }
}
