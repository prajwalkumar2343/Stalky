use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use mega_permissions::PermissionState;
use serde::Serialize;
use thiserror::Error;

use crate::model::{
    AccessibilityActionRequest, AccessibilityActionResult, AccessibilityElementId,
    AccessibilityEvent, AccessibilityEventKind, AccessibilityMetrics, AccessibilitySnapshot,
    AccessibilityState, AccessibilityStatus, MAX_DIAGNOSTIC_CHARS, MAX_RECENT_EVENTS,
};
use crate::policy::ActionPolicyError;

#[derive(Debug, Error, Serialize)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum AccessibilityError {
    #[error("Accessibility permission is not trusted")]
    NotTrusted,
    #[error("Accessibility observation is unsupported on this target")]
    UnsupportedTarget,
    #[error("Accessibility observation is already running")]
    AlreadyRunning,
    #[error("Accessibility observation is not running")]
    NotRunning,
    #[error("Accessibility operation {operation} failed with OS status {code}")]
    Native { operation: &'static str, code: i32 },
    #[error("Accessibility operation {operation} timed out")]
    Timeout { operation: &'static str },
    #[error("Accessibility action was rejected: {reason:?}")]
    ActionRejected { reason: ActionPolicyError },
    #[error("Accessibility worker stopped unexpectedly")]
    WorkerStopped,
    #[error("Accessibility worker failed to start")]
    WorkerStart,
}

impl AccessibilityError {
    pub(crate) fn diagnostic(&self) -> String {
        self.to_string()
            .chars()
            .take(MAX_DIAGNOSTIC_CHARS)
            .collect()
    }
}

pub trait AccessibilitySession: Send {
    fn stop(&mut self) -> Result<(), AccessibilityError>;
    fn execute(
        &mut self,
        request: AccessibilityActionRequest,
    ) -> Result<AccessibilityActionResult, AccessibilityError>;
}

pub trait AccessibilityBackend: Send + Sync {
    fn start(
        &self,
        events: Arc<dyn AccessibilityEventSink>,
    ) -> Result<Box<dyn AccessibilitySession>, AccessibilityError>;
}

#[allow(dead_code)]
pub trait AccessibilityEventSink: Send + Sync {
    fn publish_snapshot(&self, snapshot: AccessibilitySnapshot);
    fn record_event(&self, kind: AccessibilityEventKind, element: Option<AccessibilityElementId>);
    fn record_error(&self, error: AccessibilityError);
    fn record_stale(&self);
    fn record_unsupported(&self);
}

#[allow(dead_code)]
struct ServiceInner {
    state: AccessibilityState,
    permission: PermissionState,
    snapshot: Option<AccessibilitySnapshot>,
    recent_events: VecDeque<AccessibilityEvent>,
    metrics: AccessibilityMetrics,
    last_error: Option<String>,
    sequence: u64,
    session: Option<Box<dyn AccessibilitySession>>,
    callback_metrics: Arc<CallbackMetrics>,
}

impl Default for ServiceInner {
    fn default() -> Self {
        Self {
            state: AccessibilityState::Stopped,
            permission: PermissionState::Unknown,
            snapshot: None,
            recent_events: VecDeque::with_capacity(MAX_RECENT_EVENTS),
            metrics: AccessibilityMetrics::default(),
            last_error: None,
            sequence: 0,
            session: None,
            callback_metrics: Arc::new(CallbackMetrics::default()),
        }
    }
}

pub struct AccessibilityService {
    inner: Arc<Mutex<ServiceInner>>,
    backend: Arc<dyn AccessibilityBackend>,
    lifecycle: Arc<Mutex<()>>,
}

impl Clone for AccessibilityService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            backend: Arc::clone(&self.backend),
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }
}

impl AccessibilityService {
    pub(crate) fn with_backend<B: AccessibilityBackend + 'static>(backend: B) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServiceInner::default())),
            backend: Arc::new(backend),
            lifecycle: Arc::new(Mutex::new(())),
        }
    }

    pub fn start(&self) -> Result<AccessibilityStatus, AccessibilityError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| AccessibilityError::WorkerStopped)?;
            if inner.session.is_some() || inner.state == AccessibilityState::Running {
                return Err(AccessibilityError::AlreadyRunning);
            }
            inner.state = AccessibilityState::Starting;
            inner.last_error = None;
        }
        let sink: Arc<dyn AccessibilityEventSink> = Arc::new(ServiceEventSink {
            inner: Arc::downgrade(&self.inner),
            callback_metrics: Arc::clone(
                &self
                    .inner
                    .lock()
                    .map_err(|_| AccessibilityError::WorkerStopped)?
                    .callback_metrics,
            ),
        });
        match self.backend.start(sink) {
            Ok(session) => {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|_| AccessibilityError::WorkerStopped)?;
                inner.session = Some(session);
                inner.state = AccessibilityState::Running;
                inner.permission = PermissionState::Granted;
                Ok(status_from_inner(&inner))
            }
            Err(error) => {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|_| AccessibilityError::WorkerStopped)?;
                if matches!(error, AccessibilityError::NotTrusted) {
                    inner.permission = PermissionState::Denied;
                }
                inner.state = AccessibilityState::Failed;
                inner.last_error = Some(error.diagnostic());
                inner.metrics.errors = inner.metrics.errors.saturating_add(1);
                Err(error)
            }
        }
    }

    pub fn stop(&self) -> Result<AccessibilityStatus, AccessibilityError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        let mut session = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| AccessibilityError::WorkerStopped)?;
            inner.session.take()
        };
        let result = session
            .as_mut()
            .map(|session| session.stop())
            .unwrap_or(Ok(()));
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        inner.snapshot = None;
        inner.recent_events.clear();
        inner.state = if result.is_ok() {
            AccessibilityState::Stopped
        } else {
            AccessibilityState::Failed
        };
        if let Err(ref error) = result {
            inner.last_error = Some(error.diagnostic());
            inner.metrics.errors = inner.metrics.errors.saturating_add(1);
        }
        result.map(|()| status_from_inner(&inner))
    }

    pub fn status(&self) -> Result<AccessibilityStatus, AccessibilityError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        Ok(status_from_inner(&inner))
    }

    pub fn execute(
        &self,
        request: AccessibilityActionRequest,
    ) -> Result<AccessibilityActionResult, AccessibilityError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AccessibilityError::WorkerStopped)?;
        if inner.state != AccessibilityState::Running {
            return Err(AccessibilityError::NotRunning);
        }
        let session = inner
            .session
            .as_mut()
            .ok_or(AccessibilityError::NotRunning)?;
        match session.execute(request) {
            Ok(result) => Ok(result),
            Err(error) => {
                inner.last_error = Some(error.diagnostic());
                if matches!(error, AccessibilityError::NotTrusted) {
                    inner.permission = PermissionState::Denied;
                }
                Err(error)
            }
        }
    }
}

impl Drop for AccessibilityService {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.stop();
        }
    }
}

fn status_from_inner(inner: &ServiceInner) -> AccessibilityStatus {
    let mut metrics = inner.metrics.clone();
    let callback_metrics = &inner.callback_metrics;
    metrics.observed_events = metrics
        .observed_events
        .saturating_add(callback_metrics.observed.load(Ordering::Relaxed));
    metrics.dropped_events = metrics
        .dropped_events
        .saturating_add(callback_metrics.dropped.load(Ordering::Relaxed));
    metrics.errors = metrics
        .errors
        .saturating_add(callback_metrics.errors.load(Ordering::Relaxed));
    metrics.stale_events = metrics
        .stale_events
        .saturating_add(callback_metrics.stale.load(Ordering::Relaxed));
    metrics.unsupported_notifications = metrics
        .unsupported_notifications
        .saturating_add(callback_metrics.unsupported.load(Ordering::Relaxed));
    AccessibilityStatus {
        state: inner.state,
        permission: inner.permission,
        snapshot: inner.snapshot.clone(),
        recent_events: inner.recent_events.iter().cloned().collect(),
        metrics,
        last_error: inner.last_error.clone(),
    }
}

#[derive(Default)]
struct CallbackMetrics {
    observed: AtomicU64,
    dropped: AtomicU64,
    errors: AtomicU64,
    stale: AtomicU64,
    unsupported: AtomicU64,
}

#[allow(dead_code)]
struct ServiceEventSink {
    inner: Weak<Mutex<ServiceInner>>,
    callback_metrics: Arc<CallbackMetrics>,
}

#[allow(dead_code)]
impl ServiceEventSink {
    fn with_inner(&self, update: impl FnOnce(&mut ServiceInner)) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            self.callback_metrics
                .dropped
                .fetch_add(1, Ordering::Relaxed);
            return false;
        };
        let Ok(mut inner) = inner.try_lock() else {
            self.callback_metrics
                .dropped
                .fetch_add(1, Ordering::Relaxed);
            return false;
        };
        update(&mut inner);
        true
    }
}

impl AccessibilityEventSink for ServiceEventSink {
    fn publish_snapshot(&self, snapshot: AccessibilitySnapshot) {
        let _ = self.with_inner(|inner| inner.snapshot = Some(snapshot));
    }

    fn record_event(&self, kind: AccessibilityEventKind, element: Option<AccessibilityElementId>) {
        self.callback_metrics
            .observed
            .fetch_add(1, Ordering::Relaxed);
        let _ = self.with_inner(|inner| {
            inner.sequence = inner.sequence.saturating_add(1);
            if inner.recent_events.len() == MAX_RECENT_EVENTS {
                inner.recent_events.pop_front();
            }
            inner.recent_events.push_back(AccessibilityEvent {
                sequence: inner.sequence,
                kind,
                element,
                observed_at_millis: now_millis(),
            });
        });
    }

    fn record_error(&self, error: AccessibilityError) {
        self.callback_metrics.errors.fetch_add(1, Ordering::Relaxed);
        let diagnostic = error.diagnostic();
        let _ = self.with_inner(|inner| inner.last_error = Some(diagnostic));
    }

    fn record_stale(&self) {
        self.callback_metrics.stale.fetch_add(1, Ordering::Relaxed);
    }

    fn record_unsupported(&self) {
        self.callback_metrics
            .unsupported
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[allow(dead_code)]
fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FakeBackend {
        starts: Arc<Mutex<usize>>,
        trusted: bool,
        emit_events: bool,
    }
    struct FakeSession;

    impl AccessibilitySession for FakeSession {
        fn stop(&mut self) -> Result<(), AccessibilityError> {
            Ok(())
        }
        fn execute(
            &mut self,
            request: AccessibilityActionRequest,
        ) -> Result<AccessibilityActionResult, AccessibilityError> {
            Ok(AccessibilityActionResult {
                executed: true,
                element: request.element,
                action: request.action,
            })
        }
    }

    impl AccessibilityBackend for FakeBackend {
        fn start(
            &self,
            events: Arc<dyn AccessibilityEventSink>,
        ) -> Result<Box<dyn AccessibilitySession>, AccessibilityError> {
            *self.starts.lock().unwrap() += 1;
            if self.trusted {
                if self.emit_events {
                    for _ in 0..(MAX_RECENT_EVENTS + 1) {
                        events.record_event(AccessibilityEventKind::TitleChanged, None);
                    }
                    events.publish_snapshot(AccessibilitySnapshot {
                        generation: 1,
                        observed_at_millis: 1,
                        application: None,
                        focused_window: None,
                        focused_element: None,
                        tree: None,
                    });
                }
                Ok(Box::new(FakeSession))
            } else {
                Err(AccessibilityError::NotTrusted)
            }
        }
    }

    #[test]
    fn fake_backend_transitions_and_repeated_stop_are_deterministic() {
        let starts = Arc::new(Mutex::new(0));
        let service = AccessibilityService::with_backend(FakeBackend {
            starts: Arc::clone(&starts),
            trusted: true,
            emit_events: false,
        });
        assert_eq!(service.status().unwrap().state, AccessibilityState::Stopped);
        assert_eq!(service.start().unwrap().state, AccessibilityState::Running);
        assert_eq!(*starts.lock().unwrap(), 1);
        assert!(matches!(
            service.start(),
            Err(AccessibilityError::AlreadyRunning)
        ));
        assert_eq!(service.stop().unwrap().state, AccessibilityState::Stopped);
        assert_eq!(service.stop().unwrap().state, AccessibilityState::Stopped);
    }

    #[test]
    fn denied_start_does_not_create_a_session() {
        let service = AccessibilityService::with_backend(FakeBackend {
            starts: Arc::new(Mutex::new(0)),
            trusted: false,
            emit_events: false,
        });
        assert!(matches!(
            service.start(),
            Err(AccessibilityError::NotTrusted)
        ));
        let status = service.status().unwrap();
        assert_eq!(status.state, AccessibilityState::Failed);
        assert_eq!(status.permission, PermissionState::Denied);
    }

    #[test]
    fn fake_events_are_bounded_and_stop_clears_snapshot_and_ring() {
        let service = AccessibilityService::with_backend(FakeBackend {
            starts: Arc::new(Mutex::new(0)),
            trusted: true,
            emit_events: true,
        });
        let running = service.start().unwrap();
        assert_eq!(running.recent_events.len(), MAX_RECENT_EVENTS);
        assert_eq!(
            running.metrics.observed_events,
            (MAX_RECENT_EVENTS + 1) as u64
        );
        assert!(running.snapshot.is_some());
        let stopped = service.stop().unwrap();
        assert!(stopped.snapshot.is_none());
        assert!(stopped.recent_events.is_empty());
    }
}
