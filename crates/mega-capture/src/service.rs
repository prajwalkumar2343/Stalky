use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

use crate::{CaptureError, CaptureSource, FrameIngest, FrameInput, FrameMetrics};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub source: Option<CaptureSource>,
    pub metrics: FrameMetrics,
    pub last_error: Option<String>,
}

pub(crate) trait CaptureEvents: Send + Sync {
    fn ingest(&self, input: FrameInput<'_>);
    fn ingest_owned(&self, frame: crate::BgraFrame);
    fn record_drop(&self);
    fn record_stream_error(&self, message: String);
}

pub(crate) trait CaptureSession: Send {
    fn stop(&mut self) -> Result<(), CaptureError>;
}

pub(crate) trait CaptureBackend: Send + Sync {
    fn start(
        &self,
        source: CaptureSource,
        events: Arc<dyn CaptureEvents>,
    ) -> Result<Box<dyn CaptureSession>, CaptureError>;
}

struct ServiceInner {
    state: CaptureState,
    source: Option<CaptureSource>,
    metrics: FrameIngest,
    session: Option<Box<dyn CaptureSession>>,
    last_error: Option<String>,
    callback_drops: Arc<AtomicU64>,
}

struct ServiceEvents {
    inner: Weak<Mutex<ServiceInner>>,
    callback_drops: Arc<AtomicU64>,
}

impl CaptureEvents for ServiceEvents {
    fn ingest(&self, input: FrameInput<'_>) {
        let Some(inner_mutex) = self.inner.upgrade() else {
            self.record_drop();
            return;
        };
        let Ok(mut inner) = inner_mutex.try_lock() else {
            // The native worker is bounded. If a status/stop operation owns
            // the short state lock, reject this frame rather than blocking it.
            self.record_drop();
            return;
        };
        if inner.state != CaptureState::Running {
            self.record_drop();
            return;
        }
        if let Err(error) = inner.metrics.ingest(input) {
            inner.last_error = Some(bounded_diagnostic(error.to_string()));
        }
    }

    fn ingest_owned(&self, frame: crate::BgraFrame) {
        let Some(inner_mutex) = self.inner.upgrade() else {
            self.record_drop();
            return;
        };
        let Ok(mut inner) = inner_mutex.try_lock() else {
            self.record_drop();
            return;
        };
        if inner.state != CaptureState::Running {
            self.record_drop();
            return;
        }
        if let Err(error) = inner.metrics.ingest_owned(frame) {
            inner.last_error = Some(bounded_diagnostic(error.to_string()));
        }
    }

    fn record_drop(&self) {
        self.callback_drops.fetch_add(1, Ordering::Relaxed);
    }

    fn record_stream_error(&self, message: String) {
        let Some(inner_mutex) = self.inner.upgrade() else {
            self.record_drop();
            return;
        };
        let mut inner = inner_mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.metrics.record_stream_error();
        inner.last_error = Some(bounded_diagnostic(message));
        if inner.state == CaptureState::Running {
            inner.state = CaptureState::Failed;
        }
    }
}

pub struct CaptureService {
    inner: Arc<Mutex<ServiceInner>>,
    backend: Arc<dyn CaptureBackend>,
    lifecycle: Arc<Mutex<()>>,
}

impl Clone for CaptureService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            backend: Arc::clone(&self.backend),
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }
}

impl std::fmt::Debug for CaptureService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureService").finish_non_exhaustive()
    }
}

impl Default for CaptureService {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServiceInner {
                state: CaptureState::Stopped,
                source: None,
                metrics: FrameIngest::new(),
                session: None,
                last_error: None,
                callback_drops: Arc::new(AtomicU64::new(0)),
            })),
            backend: Arc::new(crate::platform_backend()),
            lifecycle: Arc::new(Mutex::new(())),
        }
    }

    pub fn start(&self, source: CaptureSource) -> Result<CaptureStatus, CaptureError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| CaptureError::InvalidStartState {
                state: CaptureState::Failed,
            })?;
        let callback_drops;
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| CaptureError::InvalidStartState {
                    state: CaptureState::Failed,
                })?;
            match inner.state {
                CaptureState::Stopped => {
                    inner.state = CaptureState::Starting;
                    inner.source = Some(source);
                    inner.last_error = None;
                    inner.callback_drops = Arc::new(AtomicU64::new(0));
                    callback_drops = Arc::clone(&inner.callback_drops);
                }
                CaptureState::Running | CaptureState::Starting => {
                    return Err(CaptureError::AlreadyActive { state: inner.state });
                }
                state => return Err(CaptureError::InvalidStartState { state }),
            }
        }

        let events = Arc::new(ServiceEvents {
            inner: Arc::downgrade(&self.inner),
            callback_drops,
        });

        let session = match self.backend.start(source, events) {
            Ok(session) => session,
            Err(error) => {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.state = CaptureState::Stopped;
                    inner.source = None;
                }
                return Err(error);
            }
        };

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CaptureError::InvalidStartState {
                state: CaptureState::Failed,
            })?;
        inner.session = Some(session);
        inner.state = CaptureState::Running;
        Ok(status_from_inner(&inner))
    }

    pub fn stop(&self) -> Result<CaptureStatus, CaptureError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| CaptureError::InvalidStopState {
                state: CaptureState::Failed,
            })?;
        let mut session = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| CaptureError::InvalidStopState {
                    state: CaptureState::Failed,
                })?;
            if inner.state == CaptureState::Stopped {
                return Ok(status_from_inner(&inner));
            }
            if inner.state == CaptureState::Stopping {
                return Err(CaptureError::InvalidStopState {
                    state: CaptureState::Stopping,
                });
            }
            inner.state = CaptureState::Stopping;
            inner.session.take()
        };

        let stop_result = session.as_mut().map_or(Ok(()), |session| session.stop());
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| CaptureError::InvalidStopState {
                state: CaptureState::Failed,
            })?;
        inner.metrics.clear_latest();
        inner.source = None;
        inner.state = if stop_result.is_ok() {
            CaptureState::Stopped
        } else {
            CaptureState::Failed
        };
        if let Err(error) = stop_result {
            inner.last_error = Some(error.to_string());
            return Err(error);
        }
        Ok(status_from_inner(&inner))
    }

    pub fn status(&self) -> Result<CaptureStatus, CaptureError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| CaptureError::InvalidStartState {
                state: CaptureState::Failed,
            })?;
        Ok(status_from_inner(&inner))
    }

    #[cfg(test)]
    fn with_backend(backend: Arc<dyn CaptureBackend>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServiceInner {
                state: CaptureState::Stopped,
                source: None,
                metrics: FrameIngest::new(),
                session: None,
                last_error: None,
                callback_drops: Arc::new(AtomicU64::new(0)),
            })),
            backend,
            lifecycle: Arc::new(Mutex::new(())),
        }
    }
}

fn status_from_inner(inner: &ServiceInner) -> CaptureStatus {
    let mut metrics = inner.metrics.metrics();
    metrics.dropped_frames = metrics
        .dropped_frames
        .saturating_add(inner.callback_drops.load(Ordering::Relaxed));
    CaptureStatus {
        state: inner.state,
        source: inner.source,
        metrics,
        last_error: inner.last_error.clone(),
    }
}

fn bounded_diagnostic(message: String) -> String {
    message.chars().take(256).collect()
}

impl Drop for CaptureService {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use super::{CaptureBackend, CaptureError, CaptureEvents, CaptureService, CaptureSession};
    use crate::{CaptureSource, FrameInput, FrameStatus};

    struct FakeSession {
        stopped: std::sync::Arc<AtomicBool>,
        _events: std::sync::Arc<dyn CaptureEvents>,
    }

    impl CaptureSession for FakeSession {
        fn stop(&mut self) -> Result<(), CaptureError> {
            self.stopped.store(true, Ordering::Release);
            Ok(())
        }
    }

    struct FakeBackend {
        permission_denied: bool,
        stopped: Option<std::sync::Arc<AtomicBool>>,
    }

    struct BlockingBackend {
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl CaptureBackend for BlockingBackend {
        fn start(
            &self,
            _source: CaptureSource,
            events: std::sync::Arc<dyn CaptureEvents>,
        ) -> Result<Box<dyn CaptureSession>, CaptureError> {
            let _ = self.entered.send(());
            let _ = self
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv();
            Ok(Box::new(FakeSession {
                stopped: std::sync::Arc::new(AtomicBool::new(false)),
                _events: events,
            }))
        }
    }

    impl CaptureBackend for FakeBackend {
        fn start(
            &self,
            _source: CaptureSource,
            events: std::sync::Arc<dyn CaptureEvents>,
        ) -> Result<Box<dyn CaptureSession>, CaptureError> {
            if self.permission_denied {
                return Err(CaptureError::PermissionNotGranted {
                    observed: mega_permissions::PermissionState::Denied,
                });
            }
            let stopped = self
                .stopped
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(AtomicBool::new(false)));
            Ok(Box::new(FakeSession {
                stopped,
                _events: events,
            }))
        }
    }

    #[test]
    fn start_and_stop_transition_without_capture_at_construction() {
        let service = CaptureService::with_backend(std::sync::Arc::new(FakeBackend {
            permission_denied: false,
            stopped: None,
        }));
        assert_eq!(
            service.status().unwrap().state,
            super::CaptureState::Stopped
        );
        assert_eq!(
            service.start(CaptureSource::PrimaryDisplay).unwrap().state,
            super::CaptureState::Running
        );
        assert_eq!(service.stop().unwrap().state, super::CaptureState::Stopped);
        assert_eq!(service.stop().unwrap().state, super::CaptureState::Stopped);
    }

    #[test]
    fn permission_denial_does_not_enumerate_or_leave_starting_state() {
        let service = CaptureService::with_backend(std::sync::Arc::new(FakeBackend {
            permission_denied: true,
            stopped: None,
        }));
        let error = service.start(CaptureSource::PrimaryDisplay).unwrap_err();

        assert!(matches!(error, CaptureError::PermissionNotGranted { .. }));
        assert_eq!(
            service.status().unwrap().state,
            super::CaptureState::Stopped
        );
    }

    #[test]
    fn events_are_accepted_only_while_running() {
        let service = CaptureService::with_backend(std::sync::Arc::new(FakeBackend {
            permission_denied: false,
            stopped: None,
        }));
        let events = super::ServiceEvents {
            inner: std::sync::Arc::downgrade(&service.inner),
            callback_drops: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let data = [1; 16];
        events.ingest(FrameInput {
            status: FrameStatus::Complete,
            width: 2,
            height: 2,
            bytes_per_row: 8,
            data_size: 16,
            timestamp_millis: Some(1),
            data: &data,
        });
        assert_eq!(service.status().unwrap().metrics.accepted_frames, 0);
    }

    #[test]
    fn dropping_last_service_owner_stops_session_even_when_callbacks_retain_events() {
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let service = CaptureService::with_backend(std::sync::Arc::new(FakeBackend {
            permission_denied: false,
            stopped: Some(std::sync::Arc::clone(&stopped)),
        }));
        service.start(CaptureSource::PrimaryDisplay).unwrap();

        drop(service);

        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn stop_waits_for_an_in_progress_start_before_stopping_the_session() {
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let service = CaptureService::with_backend(std::sync::Arc::new(BlockingBackend {
            entered: entered_sender,
            release: Mutex::new(release_receiver),
        }));

        let start_service = service.clone();
        let start = std::thread::spawn(move || start_service.start(CaptureSource::PrimaryDisplay));
        entered_receiver.recv().unwrap();

        let stop_service = service.clone();
        let (stop_done_sender, stop_done_receiver) = mpsc::sync_channel(1);
        let stop = std::thread::spawn(move || {
            let result = stop_service.stop();
            let _ = stop_done_sender.send(());
            result
        });
        assert!(
            stop_done_receiver
                .recv_timeout(Duration::from_millis(25))
                .is_err()
        );

        release_sender.send(()).unwrap();
        assert_eq!(
            start.join().unwrap().unwrap().state,
            super::CaptureState::Running
        );
        assert_eq!(
            stop.join().unwrap().unwrap().state,
            super::CaptureState::Stopped
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_target_is_explicit_and_does_not_change_state() {
        let service = CaptureService::new();
        let error = service.start(CaptureSource::PrimaryDisplay).unwrap_err();

        assert!(matches!(error, CaptureError::UnsupportedTarget));
        assert_eq!(
            service.status().unwrap().state,
            super::CaptureState::Stopped
        );
    }
}
