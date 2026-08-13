use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use thiserror::Error;

use crate::backend::{
    AudioBackend, AudioInputCallback, AudioSession, BackendCapabilities, CallbackDisposition,
};
use crate::types::{AudioSegment, AudioSource, AudioTimestamp, PcmBuffer, PcmFormat};
use crate::{AudioError, AudioProvenance};

/// A sink is the only output boundary of this crate. Production sinks should
/// encrypt a segment before handing it to durable storage; this service never
/// creates files or retains completed segments.
pub trait AudioSink: Send + Sync + 'static {
    fn store(&self, segment: AudioSegment) -> Result<(), SinkError>;
}

#[derive(Debug, Error)]
pub enum SinkError {
    #[error("sink rejected the segment: {0}")]
    Rejected(String),
    #[error("sink failed: {0}")]
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AudioMetrics {
    pub accepted_buffers: u64,
    pub rejected_buffers: u64,
    pub accepted_frames: u64,
    pub emitted_segments: u64,
    pub dropped_backpressure: u64,
    pub dropped_not_running: u64,
    pub dropped_invalid: u64,
    pub sink_failures: u64,
    pub queue_capacity: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AudioStatus {
    pub state: AudioState,
    pub source: Option<AudioSource>,
    pub generation: u64,
    pub metrics: AudioMetrics,
    pub last_error: Option<String>,
}

impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            state: AudioState::Stopped,
            source: None,
            generation: 0,
            metrics: AudioMetrics::default(),
            last_error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioServiceConfig {
    pub queue_depth: usize,
    pub segment_duration_millis: u32,
}

impl Default for AudioServiceConfig {
    fn default() -> Self {
        Self {
            queue_depth: 8,
            segment_duration_millis: 1_000,
        }
    }
}

impl AudioServiceConfig {
    fn validate(self) -> Result<(), AudioError> {
        if !(1..=64).contains(&self.queue_depth) {
            return Err(AudioError::InvalidConfig {
                detail: "queue depth must be between 1 and 64",
            });
        }
        if !(crate::MIN_SEGMENT_DURATION_MILLIS..=crate::MAX_SEGMENT_DURATION_MILLIS)
            .contains(&self.segment_duration_millis)
        {
            return Err(AudioError::InvalidConfig {
                detail: "segment duration must be between 100 ms and 10 seconds",
            });
        }
        Ok(())
    }
}

struct ServiceInner {
    status: AudioStatus,
    queue: Option<SyncSender<PcmBuffer>>,
    active: Option<Arc<AtomicBool>>,
    session: Option<Box<dyn AudioSession>>,
    worker: Option<JoinHandle<Result<(), WorkerError>>>,
}

struct ServiceCallback {
    inner: Weak<Mutex<ServiceInner>>,
}

pub struct AudioService {
    inner: Arc<Mutex<ServiceInner>>,
    backend: Arc<dyn AudioBackend>,
    sink: Arc<dyn AudioSink>,
    config: AudioServiceConfig,
    lifecycle: Mutex<()>,
}

impl std::fmt::Debug for AudioService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AudioService {
    pub fn new(
        backend: Arc<dyn AudioBackend>,
        sink: Arc<dyn AudioSink>,
        config: AudioServiceConfig,
    ) -> Result<Self, AudioError> {
        config.validate()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(ServiceInner {
                status: AudioStatus {
                    metrics: AudioMetrics {
                        queue_capacity: config.queue_depth,
                        ..AudioMetrics::default()
                    },
                    ..AudioStatus::default()
                },
                queue: None,
                active: None,
                session: None,
                worker: None,
            })),
            backend,
            sink,
            config,
            lifecycle: Mutex::new(()),
        })
    }

    pub fn capabilities(&self) -> BackendCapabilities {
        self.backend.capabilities()
    }

    pub fn status(&self) -> AudioStatus {
        self.inner
            .lock()
            .map(|inner| inner.status.clone())
            .unwrap_or_else(|_| AudioStatus {
                state: AudioState::Failed,
                last_error: Some("audio service state lock was poisoned".to_owned()),
                ..AudioStatus::default()
            })
    }

    pub fn start(&self, source: AudioSource) -> Result<AudioStatus, AudioError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| AudioError::InvalidStartState {
                state: AudioState::Failed,
            })?;
        let generation;
        let (sender, receiver) = mpsc::sync_channel(self.config.queue_depth);
        let active = Arc::new(AtomicBool::new(true));
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| AudioError::InvalidStartState {
                    state: AudioState::Failed,
                })?;
            match inner.status.state {
                AudioState::Stopped => {}
                AudioState::Running | AudioState::Starting => {
                    return Err(AudioError::AlreadyActive {
                        state: inner.status.state,
                    });
                }
                state => return Err(AudioError::InvalidStartState { state }),
            }
            if !self.capabilities().supports(source) {
                return Err(AudioError::UnsupportedSource {
                    audio_source: source,
                    detail: "the backend does not advertise this source",
                });
            }
            generation = inner.status.generation.saturating_add(1).max(1);
            inner.status.state = AudioState::Starting;
            inner.status.source = Some(source);
            inner.status.generation = generation;
            inner.status.last_error = None;
            inner.status.metrics = AudioMetrics {
                queue_capacity: self.config.queue_depth,
                ..AudioMetrics::default()
            };
            inner.queue = Some(sender);
            inner.active = Some(Arc::clone(&active));
        }

        let weak_inner = Arc::downgrade(&self.inner);
        let sink = Arc::clone(&self.sink);
        let config = self.config;
        let worker = match thread::Builder::new()
            .name("stalky-audio-ingest".to_owned())
            .spawn(move || run_worker(receiver, sink, config, source, generation, weak_inner))
        {
            Ok(worker) => worker,
            Err(error) => {
                active.store(false, Ordering::Release);
                if let Ok(mut inner) = self.inner.lock() {
                    inner.queue.take();
                }
                self.reset_after_start_failure(format!("could not start audio worker: {error}"));
                return Err(AudioError::BackendStart {
                    message: format!("could not start audio worker: {error}"),
                });
            }
        };

        let callback: Arc<dyn AudioInputCallback> = Arc::new(ServiceCallback {
            inner: Arc::downgrade(&self.inner),
        });
        let session = match self.backend.start(source, generation, callback) {
            Ok(session) => session,
            Err(error) => {
                active.store(false, Ordering::Release);
                if let Ok(mut inner) = self.inner.lock() {
                    inner.queue.take();
                }
                let _ = worker.join();
                self.reset_after_start_failure(error.to_string());
                return Err(error);
            }
        };

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AudioError::InvalidStartState {
                state: AudioState::Failed,
            })?;
        inner.session = Some(session);
        inner.worker = Some(worker);
        inner.status.state = AudioState::Running;
        Ok(inner.status.clone())
    }

    pub fn stop(&self) -> Result<AudioStatus, AudioError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| AudioError::InvalidStopState {
                state: AudioState::Failed,
            })?;
        let (session, worker, active) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| AudioError::InvalidStopState {
                    state: AudioState::Failed,
                })?;
            if inner.status.state == AudioState::Stopped {
                return Ok(inner.status.clone());
            }
            inner.status.state = AudioState::Stopping;
            if let Some(active) = &inner.active {
                active.store(false, Ordering::Release);
            }
            (
                inner.session.take(),
                inner.worker.take(),
                inner.active.take(),
            )
        };

        let backend_result = session.map(|session| session.stop());
        // Dropping the session releases native callback blocks; dropping the
        // queue sender allows the worker to flush a partial final segment.
        drop(active);
        if let Ok(mut inner) = self.inner.lock() {
            inner.queue.take();
        }

        let worker_result = worker.map(|worker| {
            worker
                .join()
                .map_err(|_| AudioError::WorkerJoin)
                .and_then(|result| result.map_err(worker_error_to_audio_error))
        });
        let worker_error = worker_result.and_then(Result::err);
        let backend_error = backend_result.and_then(|result| result.err());
        let error = worker_error.or_else(|| {
            backend_error.map(|error| AudioError::BackendStop {
                message: error.to_string(),
            })
        });
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AudioError::InvalidStopState {
                state: AudioState::Failed,
            })?;
        inner.status.state = if error.is_some() {
            AudioState::Failed
        } else {
            AudioState::Stopped
        };
        inner.status.source = None;
        inner.queue = None;
        inner.active = None;
        if let Some(error) = &error {
            inner.status.last_error = Some(error.to_string());
        }
        let status = inner.status.clone();
        match error {
            Some(error) => Err(error),
            None => Ok(status),
        }
    }

    /// Admits a validated callback buffer without blocking. This method is
    /// also useful to an externally owned backend that uses this service as
    /// its callback target.
    pub fn push(&self, buffer: PcmBuffer) -> CallbackDisposition {
        push_inner_arc(&self.inner, buffer)
    }

    fn reset_after_start_failure(&self, message: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.queue = None;
            inner.active = None;
            inner.status.state = AudioState::Stopped;
            inner.status.source = None;
            inner.status.last_error = Some(message);
        }
    }
}

impl AudioInputCallback for ServiceCallback {
    fn push(&self, buffer: PcmBuffer) -> CallbackDisposition {
        let Some(inner) = self.inner.upgrade() else {
            return CallbackDisposition::DroppedNotRunning;
        };
        push_inner_arc(&inner, buffer)
    }
}

impl Drop for AudioService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn push_inner_arc(inner: &Arc<Mutex<ServiceInner>>, buffer: PcmBuffer) -> CallbackDisposition {
    let frame_count = buffer.frame_count() as u64;
    let (queue, active) = match inner.lock() {
        Ok(inner)
            if matches!(
                inner.status.state,
                AudioState::Starting | AudioState::Running
            ) =>
        {
            (inner.queue.clone(), inner.active.clone())
        }
        _ => (None, None),
    };
    let Some(queue) = queue else {
        increment_metric(inner, |metrics| metrics.dropped_not_running += 1);
        return CallbackDisposition::DroppedNotRunning;
    };
    if !active.is_some_and(|active| active.load(Ordering::Acquire)) {
        increment_metric(inner, |metrics| metrics.dropped_not_running += 1);
        return CallbackDisposition::DroppedNotRunning;
    }
    match queue.try_send(buffer) {
        Ok(()) => {
            increment_metric(inner, |metrics| {
                metrics.accepted_buffers += 1;
                metrics.accepted_frames += frame_count;
            });
            CallbackDisposition::Accepted
        }
        Err(TrySendError::Full(buffer)) => {
            drop(buffer);
            increment_metric(inner, |metrics| metrics.dropped_backpressure += 1);
            CallbackDisposition::DroppedBackpressure
        }
        Err(TrySendError::Disconnected(buffer)) => {
            drop(buffer);
            increment_metric(inner, |metrics| metrics.dropped_not_running += 1);
            CallbackDisposition::DroppedNotRunning
        }
    }
}

fn increment_metric(inner: &Arc<Mutex<ServiceInner>>, update: impl FnOnce(&mut AudioMetrics)) {
    if let Ok(mut inner) = inner.lock() {
        update(&mut inner.status.metrics);
    }
}

#[derive(Debug)]
enum WorkerError {
    Sink(SinkError),
}

fn run_worker(
    receiver: mpsc::Receiver<PcmBuffer>,
    sink: Arc<dyn AudioSink>,
    config: AudioServiceConfig,
    source: AudioSource,
    generation: u64,
    weak_inner: Weak<Mutex<ServiceInner>>,
) -> Result<(), WorkerError> {
    let mut assembler = SegmentAssembler::new(config.segment_duration_millis, source, generation);
    while let Ok(buffer) = receiver.recv() {
        match assembler.push(buffer) {
            Ok(segments) => {
                for segment in segments {
                    if let Err(error) = sink.store(segment) {
                        record_sink_failure(&weak_inner, error.to_string());
                        return Err(WorkerError::Sink(error));
                    }
                    record_segment(&weak_inner);
                }
            }
            Err(error) => {
                record_rejected(&weak_inner, error.to_string());
            }
        }
    }
    if let Some(segment) = assembler.finish() {
        if let Err(error) = sink.store(segment) {
            record_sink_failure(&weak_inner, error.to_string());
            return Err(WorkerError::Sink(error));
        }
        record_segment(&weak_inner);
    }
    Ok(())
}

fn record_segment(inner: &Weak<Mutex<ServiceInner>>) {
    if let Some(inner) = inner.upgrade()
        && let Ok(mut inner) = inner.lock()
    {
        inner.status.metrics.emitted_segments += 1;
    }
}

fn record_rejected(inner: &Weak<Mutex<ServiceInner>>, message: String) {
    if let Some(inner) = inner.upgrade()
        && let Ok(mut inner) = inner.lock()
    {
        inner.status.metrics.rejected_buffers += 1;
        inner.status.metrics.dropped_invalid += 1;
        inner.status.last_error = Some(message);
    }
}

fn record_sink_failure(inner: &Weak<Mutex<ServiceInner>>, message: String) {
    if let Some(inner) = inner.upgrade()
        && let Ok(mut inner) = inner.lock()
    {
        inner.status.metrics.sink_failures += 1;
        inner.status.state = AudioState::Failed;
        inner.status.last_error = Some(message);
    }
}

fn worker_error_to_audio_error(error: WorkerError) -> AudioError {
    match error {
        WorkerError::Sink(error) => AudioError::SinkFailed {
            message: error.to_string(),
        },
    }
}

struct SegmentAssembler {
    segment_frames: usize,
    segment_duration_millis: u32,
    source: AudioSource,
    generation: u64,
    format: Option<PcmFormat>,
    provenance: Option<AudioProvenance>,
    timestamp: Option<AudioTimestamp>,
    samples: Vec<i16>,
    next_sequence: u64,
}

impl SegmentAssembler {
    fn new(duration_millis: u32, source: AudioSource, generation: u64) -> Self {
        Self {
            segment_frames: 0,
            segment_duration_millis: duration_millis,
            source,
            generation,
            format: None,
            provenance: None,
            timestamp: None,
            samples: Vec::new(),
            next_sequence: 0,
        }
    }

    fn push(&mut self, buffer: PcmBuffer) -> Result<Vec<AudioSegment>, AudioError> {
        if buffer.provenance().source != self.source {
            return Err(AudioError::ProvenanceSourceMismatch {
                expected: self.source,
                observed: buffer.provenance().source,
            });
        }
        if buffer.provenance().generation != self.generation {
            return Err(AudioError::InvalidPcm {
                detail: "buffer generation does not match the active stream",
            });
        }
        if let Some(expected) = &self.provenance {
            if expected != buffer.provenance() {
                return Err(AudioError::InvalidPcm {
                    detail: "buffer provenance changed during an active stream",
                });
            }
        } else {
            self.provenance = Some(buffer.provenance().clone());
        }
        let format = buffer.format();
        let timestamp = buffer.timestamp();
        if let Some(expected) = self.format {
            if expected != format {
                return Err(AudioError::FormatChanged {
                    expected,
                    observed: format,
                });
            }
        } else {
            self.format = Some(format);
            self.segment_frames = format.segment_frames(self.segment_duration_millis);
        }
        if self.timestamp.is_none() {
            self.timestamp = Some(timestamp);
        }
        let (_, _, provenance, samples) = buffer.into_parts();
        self.samples.extend(samples);
        let mut segments = Vec::new();
        let frame_width = usize::from(format.channels);
        let segment_samples = self.segment_frames.saturating_mul(frame_width);
        while self.samples.len() >= segment_samples {
            let segment_samples_vec = self.samples.drain(..segment_samples).collect();
            let segment_timestamp = self.timestamp.expect("timestamp set above");
            segments.push(AudioSegment::new(
                self.next_sequence,
                segment_timestamp,
                format,
                provenance.clone(),
                segment_samples_vec,
                false,
            ));
            self.next_sequence = self.next_sequence.saturating_add(1);
            self.timestamp =
                Some(segment_timestamp.offset_frames(self.segment_frames, format.sample_rate_hz));
        }
        Ok(segments)
    }

    fn finish(&mut self) -> Option<AudioSegment> {
        let format = self.format?;
        if self.samples.is_empty() {
            return None;
        }
        let samples = std::mem::take(&mut self.samples);
        let timestamp = self.timestamp?;
        let provenance = self.provenance.clone()?;
        Some(AudioSegment::new(
            self.next_sequence,
            timestamp,
            format,
            provenance,
            samples,
            true,
        ))
    }
}
