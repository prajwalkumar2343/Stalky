use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mega_ipc::{
    PERMISSION_SCHEMA_VERSION, PermissionCapability, PermissionError, PermissionOperation,
    PermissionSnapshot, PermissionStatus,
};
use mega_permissions::{PRIVACY_CAPABILITIES, PermissionRegistry, PermissionState};
use mega_platform_macos::{MacOsPlatform, PlatformError, PlatformOperation};

const MAIN_QUEUE_CAPABILITY: PermissionCapability = PermissionCapability::Accessibility;

trait PermissionPlatform: Send + Sync {
    fn permission_status(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError>;
    fn request_permission(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError>;
    fn open_permission_settings(
        &self,
        capability: PermissionCapability,
    ) -> Result<(), PlatformError>;
}

impl PermissionPlatform for MacOsPlatform {
    fn permission_status(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError> {
        MacOsPlatform::permission_status(self, capability)
    }

    fn request_permission(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError> {
        MacOsPlatform::request_permission(self, capability)
    }

    fn open_permission_settings(
        &self,
        capability: PermissionCapability,
    ) -> Result<(), PlatformError> {
        MacOsPlatform::open_permission_settings(self, capability)
    }
}

#[derive(Clone)]
pub struct PermissionCoordinator {
    inner: Arc<PermissionCoordinatorInner>,
}

struct PermissionCoordinatorInner {
    platform: Arc<dyn PermissionPlatform>,
    registry: Mutex<PermissionRegistry>,
    errors: Mutex<BTreeMap<PermissionCapability, PermissionError>>,
    last_snapshot: Mutex<Option<PermissionSnapshot>>,
    sequence: AtomicU64,
    operation_lock: Mutex<()>,
}

impl Default for PermissionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionCoordinator {
    pub fn new() -> Self {
        Self::with_platform(Arc::new(MacOsPlatform::new()))
    }

    fn with_platform(platform: Arc<dyn PermissionPlatform>) -> Self {
        Self {
            inner: Arc::new(PermissionCoordinatorInner {
                platform,
                registry: Mutex::new(PermissionRegistry::new()),
                errors: Mutex::new(BTreeMap::new()),
                last_snapshot: Mutex::new(None),
                sequence: AtomicU64::new(0),
                operation_lock: Mutex::new(()),
            }),
        }
    }

    /// Returns only coordinator state. This does not call a native probe and
    /// can therefore be used to paint the UI without causing TCC activity.
    pub fn snapshot(&self) -> Result<PermissionSnapshot, PermissionError> {
        let registry = self.lock_registry(MAIN_QUEUE_CAPABILITY)?;
        let errors = self.lock_errors(MAIN_QUEUE_CAPABILITY)?;
        self.snapshot_from(&registry, &errors)
    }

    /// Rechecks every privacy capability without requesting anything. A
    /// boolean false from a first probe remains Unknown because macOS does not
    /// expose enough information to call it a denial honestly.
    pub fn recheck_with_notify<F>(
        &self,
        mut notify: F,
    ) -> Result<PermissionSnapshot, PermissionError>
    where
        F: FnMut(PermissionSnapshot),
    {
        let _operation = self.lock_operations(MAIN_QUEUE_CAPABILITY)?;

        let mut capabilities = Vec::new();
        for capability in PRIVACY_CAPABILITIES {
            let can_recheck = {
                let mut registry = self.lock_registry(capability)?;
                if registry.operation(capability) == PermissionOperation::Rechecking {
                    true
                } else {
                    registry.begin_recheck(capability).is_ok()
                }
            };
            if !can_recheck {
                continue;
            }
            capabilities.push(capability);
        }
        if !capabilities.is_empty() {
            notify(self.snapshot()?);
        }
        for capability in capabilities {
            let observed = self.inner.platform.permission_status(capability);
            let _ = self.finish_probe(capability, observed);
        }

        // Per-capability native failures are already retained in the status
        // for the affected capability. Keep returning the complete snapshot
        // so one unavailable probe cannot hide the other two.
        self.snapshot()
    }

    /// Runs one explicit native request. The callback receives the transient
    /// Requesting snapshot before the OS call and the settled snapshot after
    /// it, allowing Tauri to emit both states without prompting on startup.
    pub fn request<F>(
        &self,
        capability: PermissionCapability,
        mut notify: F,
    ) -> Result<PermissionSnapshot, PermissionError>
    where
        F: FnMut(PermissionSnapshot),
    {
        let _operation = self.lock_operations(capability)?;
        {
            let mut registry = self.lock_registry(capability)?;
            registry
                .begin_request(capability)
                .map_err(|error| transition_error(capability, error, registry.state(capability)))?;
        }
        notify(self.snapshot()?);

        let observed = self.inner.platform.request_permission(capability);
        let result = self.finish_request(capability, observed);
        let snapshot = self.snapshot()?;
        notify(snapshot.clone());
        result?;
        Ok(snapshot)
    }

    /// Opens an allowlisted System Settings destination. The native adapter
    /// falls back to the general Privacy & Security pane when an anchor is
    /// unavailable; success leaves this capability in Rechecking so the UI
    /// can poll after the user returns.
    pub fn open_settings(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionSnapshot, PermissionError> {
        let _operation = self.lock_operations(capability)?;
        self.inner
            .platform
            .open_permission_settings(capability)
            .map_err(|error| platform_error(capability, PlatformOperation::OpenSettings, error))?;

        {
            let mut registry = self.lock_registry(capability)?;
            let _ = registry.begin_recheck(capability);
        }
        self.clear_error(capability)?;
        self.snapshot()
    }

    fn finish_probe(
        &self,
        capability: PermissionCapability,
        observed: Result<PermissionState, PlatformError>,
    ) -> Result<(), PermissionError> {
        match observed {
            Ok(observed) => {
                let mut registry = self.lock_registry(capability)?;
                let previous = registry.authorization(capability);
                let operation = registry.operation(capability);
                let observed = normalize_probe(previous, operation, observed);
                registry.observe(capability, observed).map_err(|error| {
                    transition_error(capability, error, registry.state(capability))
                })?;
                drop(registry);
                self.clear_error(capability)
            }
            Err(PlatformError::Unsupported { .. }) => {
                let mut registry = self.lock_registry(capability)?;
                registry
                    .observe(capability, PermissionState::Unsupported)
                    .map_err(|error| {
                        transition_error(capability, error, registry.state(capability))
                    })?;
                drop(registry);
                self.set_error(capability, PermissionError::Unsupported { capability })?;
                Ok(())
            }
            Err(error) => {
                self.recover(capability)?;
                let mapped = platform_error(capability, PlatformOperation::Probe, error);
                self.set_error(capability, mapped.clone())?;
                Err(mapped)
            }
        }
    }

    fn finish_request(
        &self,
        capability: PermissionCapability,
        observed: Result<PermissionState, PlatformError>,
    ) -> Result<(), PermissionError> {
        match observed {
            Ok(observed) => {
                let mut registry = self.lock_registry(capability)?;
                registry.observe(capability, observed).map_err(|error| {
                    transition_error(capability, error, registry.state(capability))
                })?;
                drop(registry);
                self.clear_error(capability)
            }
            Err(error) => {
                self.recover(capability)?;
                let mapped = platform_error(capability, PlatformOperation::Request, error);
                self.set_error(capability, mapped.clone())?;
                Err(mapped)
            }
        }
    }

    fn recover(&self, capability: PermissionCapability) -> Result<(), PermissionError> {
        let mut registry = self.lock_registry(capability)?;
        registry
            .recover(capability)
            .map_err(|error| transition_error(capability, error, registry.state(capability)))?;
        Ok(())
    }

    fn snapshot_from(
        &self,
        registry: &PermissionRegistry,
        errors: &BTreeMap<PermissionCapability, PermissionError>,
    ) -> Result<PermissionSnapshot, PermissionError> {
        let statuses = PRIVACY_CAPABILITIES
            .into_iter()
            .map(|capability| {
                let state = registry.state(capability);
                let authorization = registry.authorization(capability);
                let operation = registry.operation(capability);
                PermissionStatus {
                    capability,
                    state,
                    authorization,
                    operation,
                    last_error: errors.get(&capability).cloned(),
                    can_request: can_request(authorization, operation),
                    can_open_settings: !matches!(authorization, PermissionState::Unsupported),
                }
            })
            .collect();

        let mut last_snapshot = self.lock_last_snapshot(MAIN_QUEUE_CAPABILITY)?;
        if let Some(last_snapshot) = last_snapshot.as_ref()
            && last_snapshot.statuses == statuses
        {
            return Ok(last_snapshot.clone());
        }

        let snapshot = PermissionSnapshot {
            schema_version: PERMISSION_SCHEMA_VERSION,
            sequence: self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            statuses,
        };
        *last_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    fn lock_registry(
        &self,
        capability: PermissionCapability,
    ) -> Result<std::sync::MutexGuard<'_, PermissionRegistry>, PermissionError> {
        self.inner
            .registry
            .lock()
            .map_err(|_| PermissionError::InvalidTransition {
                capability,
                message: "permission registry is unavailable".to_owned(),
            })
    }

    fn lock_errors(
        &self,
        capability: PermissionCapability,
    ) -> Result<
        std::sync::MutexGuard<'_, BTreeMap<PermissionCapability, PermissionError>>,
        PermissionError,
    > {
        self.inner
            .errors
            .lock()
            .map_err(|_| PermissionError::InvalidTransition {
                capability,
                message: "permission error state is unavailable".to_owned(),
            })
    }

    fn lock_last_snapshot(
        &self,
        capability: PermissionCapability,
    ) -> Result<std::sync::MutexGuard<'_, Option<PermissionSnapshot>>, PermissionError> {
        self.inner
            .last_snapshot
            .lock()
            .map_err(|_| PermissionError::InvalidTransition {
                capability,
                message: "permission snapshot state is unavailable".to_owned(),
            })
    }

    fn lock_operations(
        &self,
        capability: PermissionCapability,
    ) -> Result<std::sync::MutexGuard<'_, ()>, PermissionError> {
        self.inner
            .operation_lock
            .try_lock()
            .map_err(|error| match error {
                std::sync::TryLockError::Poisoned(_) | std::sync::TryLockError::WouldBlock => {
                    PermissionError::Busy { capability }
                }
            })
    }

    fn set_error(
        &self,
        capability: PermissionCapability,
        error: PermissionError,
    ) -> Result<(), PermissionError> {
        self.lock_errors(capability)?.insert(capability, error);
        Ok(())
    }

    fn clear_error(&self, capability: PermissionCapability) -> Result<(), PermissionError> {
        self.lock_errors(capability)?.remove(&capability);
        Ok(())
    }
}

fn can_request(authorization: PermissionState, operation: PermissionOperation) -> bool {
    operation == PermissionOperation::Idle
        && matches!(
            authorization,
            PermissionState::Unknown
                | PermissionState::NotDetermined
                | PermissionState::Denied
                | PermissionState::Revoked
        )
}

fn normalize_probe(
    previous: PermissionState,
    operation: PermissionOperation,
    observed: PermissionState,
) -> PermissionState {
    if observed == PermissionState::Denied
        && operation != PermissionOperation::Requesting
        && matches!(
            previous,
            PermissionState::Unknown | PermissionState::NotDetermined
        )
    {
        PermissionState::Unknown
    } else {
        observed
    }
}

fn transition_error(
    capability: PermissionCapability,
    error: mega_permissions::PermissionTransitionError,
    state: PermissionState,
) -> PermissionError {
    if state.is_in_flight() {
        PermissionError::Busy { capability }
    } else {
        PermissionError::InvalidTransition {
            capability,
            message: error.to_string(),
        }
    }
}

fn platform_error(
    capability: PermissionCapability,
    operation: PlatformOperation,
    error: PlatformError,
) -> PermissionError {
    match error {
        PlatformError::Unsupported { .. } => PermissionError::Unsupported { capability },
        PlatformError::Native { message, .. } => match operation {
            PlatformOperation::Probe => PermissionError::ProbeFailed {
                capability,
                message,
            },
            PlatformOperation::Request => PermissionError::RequestFailed {
                capability,
                message,
            },
            PlatformOperation::OpenSettings => PermissionError::SettingsFailed {
                capability,
                message,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mega_platform_macos::PlatformFeature;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    struct FakePlatform {
        probes: Mutex<VecDeque<Result<PermissionState, PlatformError>>>,
        requests: Mutex<VecDeque<Result<PermissionState, PlatformError>>>,
        settings: Mutex<VecDeque<Result<(), PlatformError>>>,
        request_started: Option<Arc<AtomicBool>>,
        request_release: Option<Arc<AtomicBool>>,
    }

    impl FakePlatform {
        fn new(probes: Vec<Result<PermissionState, PlatformError>>) -> Self {
            Self {
                probes: Mutex::new(probes.into()),
                requests: Mutex::new(VecDeque::from([Ok(PermissionState::Granted)])),
                settings: Mutex::new(VecDeque::from([Ok(())])),
                request_started: None,
                request_release: None,
            }
        }

        fn blocking_request(
            probes: Vec<Result<PermissionState, PlatformError>>,
            started: Arc<AtomicBool>,
            release: Arc<AtomicBool>,
        ) -> Self {
            let mut platform = Self::new(probes);
            platform.request_started = Some(started);
            platform.request_release = Some(release);
            platform
        }
    }

    impl PermissionPlatform for FakePlatform {
        fn permission_status(
            &self,
            _capability: PermissionCapability,
        ) -> Result<PermissionState, PlatformError> {
            self.probes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(PermissionState::Granted))
        }

        fn request_permission(
            &self,
            _capability: PermissionCapability,
        ) -> Result<PermissionState, PlatformError> {
            if let (Some(started), Some(release)) = (&self.request_started, &self.request_release) {
                started.store(true, Ordering::Release);
                while !release.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(2));
                }
            }
            self.requests
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(PermissionState::Granted))
        }

        fn open_permission_settings(
            &self,
            _capability: PermissionCapability,
        ) -> Result<(), PlatformError> {
            self.settings.lock().unwrap().pop_front().unwrap_or(Ok(()))
        }
    }

    fn error(message: &str) -> PlatformError {
        PlatformError::Native {
            feature: PlatformFeature::ScreenRecordingPermission,
            operation: PlatformOperation::Probe,
            message: message.to_owned(),
        }
    }

    #[test]
    fn one_global_request_blocks_a_different_capability() {
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let coordinator = PermissionCoordinator::with_platform(Arc::new(
            FakePlatform::blocking_request(Vec::new(), started.clone(), release.clone()),
        ));
        let first = coordinator.clone();
        let request =
            thread::spawn(move || first.request(PermissionCapability::ScreenRecording, |_| {}));
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let second = coordinator.request(PermissionCapability::Microphone, |_| {});
        assert_eq!(
            second,
            Err(PermissionError::Busy {
                capability: PermissionCapability::Microphone,
            })
        );
        release.store(true, Ordering::Release);
        assert!(request.join().unwrap().is_ok());
    }

    #[test]
    fn settings_return_recheck_settles_an_existing_rechecking_operation() {
        let coordinator =
            PermissionCoordinator::with_platform(Arc::new(FakePlatform::new(vec![Ok(
                PermissionState::Granted,
            )])));
        let opened = coordinator
            .open_settings(PermissionCapability::Accessibility)
            .unwrap();
        let accessibility = opened
            .statuses
            .iter()
            .find(|status| status.capability == PermissionCapability::Accessibility)
            .unwrap();
        assert_eq!(accessibility.operation, PermissionOperation::Rechecking);

        let settled = coordinator.recheck_with_notify(|_| {}).unwrap();
        let accessibility = settled
            .statuses
            .iter()
            .find(|status| status.capability == PermissionCapability::Accessibility)
            .unwrap();
        assert_eq!(accessibility.authorization, PermissionState::Granted);
        assert_eq!(accessibility.operation, PermissionOperation::Idle);
    }

    #[test]
    fn unchanged_snapshots_reuse_their_sequence() {
        let coordinator =
            PermissionCoordinator::with_platform(Arc::new(FakePlatform::new(Vec::new())));
        let first = coordinator.snapshot().unwrap();
        let second = coordinator.snapshot().unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn one_probe_failure_does_not_hide_later_capabilities() {
        let coordinator = PermissionCoordinator::with_platform(Arc::new(FakePlatform::new(vec![
            Err(error("screen probe unavailable")),
            Ok(PermissionState::Granted),
            Ok(PermissionState::Granted),
        ])));
        let snapshot = coordinator.recheck_with_notify(|_| {}).unwrap();

        let screen = snapshot
            .statuses
            .iter()
            .find(|status| status.capability == PermissionCapability::ScreenRecording)
            .unwrap();
        let accessibility = snapshot
            .statuses
            .iter()
            .find(|status| status.capability == PermissionCapability::Accessibility)
            .unwrap();
        let microphone = snapshot
            .statuses
            .iter()
            .find(|status| status.capability == PermissionCapability::Microphone)
            .unwrap();
        assert!(matches!(
            screen.last_error,
            Some(PermissionError::ProbeFailed { .. })
        ));
        assert_eq!(accessibility.authorization, PermissionState::Granted);
        assert_eq!(microphone.authorization, PermissionState::Granted);
    }

    #[test]
    fn recheck_notifies_the_transient_state_once() {
        let coordinator = PermissionCoordinator::with_platform(Arc::new(FakePlatform::new(vec![
            Ok(PermissionState::Granted),
            Ok(PermissionState::Granted),
            Ok(PermissionState::Granted),
        ])));
        let mut notifications = Vec::new();
        let settled = coordinator
            .recheck_with_notify(|snapshot| notifications.push(snapshot))
            .unwrap();

        assert_eq!(notifications.len(), 1);
        assert!(
            notifications[0]
                .statuses
                .iter()
                .all(|status| status.operation == PermissionOperation::Rechecking)
        );
        assert!(
            settled
                .statuses
                .iter()
                .all(|status| status.operation == PermissionOperation::Idle)
        );
    }

    #[test]
    fn coarse_false_probe_is_not_presented_as_a_precise_denial() {
        assert_eq!(
            normalize_probe(
                PermissionState::Unknown,
                PermissionOperation::Rechecking,
                PermissionState::Denied,
            ),
            PermissionState::Unknown
        );
        assert_eq!(
            normalize_probe(
                PermissionState::NotDetermined,
                PermissionOperation::Rechecking,
                PermissionState::Denied,
            ),
            PermissionState::Unknown
        );
        assert_eq!(
            normalize_probe(
                PermissionState::NotDetermined,
                PermissionOperation::Requesting,
                PermissionState::Denied,
            ),
            PermissionState::Denied
        );
    }
}
