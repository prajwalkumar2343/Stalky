use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use mega_permissions::{
    PermissionCapability, PermissionRegistry, PermissionRequestDecision, PermissionState,
    permission_request_decision,
};
use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PermissionRequestError {
    #[error("permission is already granted")]
    AlreadyGranted,
    #[error("permission request is already in flight")]
    AlreadyRequesting,
    #[error("permission recovery must be completed in System Settings")]
    OpenSettings,
    #[error("permission is unsupported in this build")]
    Unsupported,
    #[error("permission state transition failed: {0}")]
    Transition(String),
}

#[derive(Debug)]
struct CoordinatorState {
    registry: PermissionRegistry,
    in_flight: BTreeSet<PermissionCapability>,
}

/// Single backend owner for permission state and request concurrency.
///
/// OS grants are never stored here as durable truth. Every status refresh
/// replaces the registry from fresh native observations; `in_flight` only
/// prevents duplicate prompts while an explicit request is running.
#[derive(Debug)]
pub struct PermissionCoordinator {
    state: Arc<Mutex<CoordinatorState>>,
}

impl Clone for PermissionCoordinator {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl Default for PermissionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionCoordinator {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CoordinatorState {
                registry: PermissionRegistry::new(),
                in_flight: BTreeSet::new(),
            })),
        }
    }

    pub fn observe(&self, capability: PermissionCapability, state: PermissionState) {
        if let Ok(mut inner) = self.state.lock() {
            let _ = inner.registry.observe(capability, state);
        }
    }

    pub fn snapshot(&self) -> BTreeMap<PermissionCapability, PermissionState> {
        self.state
            .lock()
            .map(|inner| {
                let mut snapshot: BTreeMap<_, _> = inner.registry.snapshot().iter().collect();
                for capability in &inner.in_flight {
                    snapshot.insert(*capability, PermissionState::Requesting);
                }
                snapshot
            })
            .unwrap_or_default()
    }

    pub fn begin_request(
        &self,
        capability: PermissionCapability,
        has_requested: bool,
    ) -> Result<(), PermissionRequestError> {
        let mut inner = self
            .state
            .lock()
            .map_err(|_| PermissionRequestError::AlreadyRequesting)?;
        if inner.in_flight.contains(&capability) {
            return Err(PermissionRequestError::AlreadyRequesting);
        }
        match permission_request_decision(inner.registry.state(capability), has_requested) {
            PermissionRequestDecision::Request => {
                inner
                    .registry
                    .begin_request(capability)
                    .map_err(|error| PermissionRequestError::Transition(error.to_string()))?;
                inner.in_flight.insert(capability);
                Ok(())
            }
            PermissionRequestDecision::AlreadyGranted => {
                Err(PermissionRequestError::AlreadyGranted)
            }
            PermissionRequestDecision::AlreadyRequesting => {
                Err(PermissionRequestError::AlreadyRequesting)
            }
            PermissionRequestDecision::OpenSettings => Err(PermissionRequestError::OpenSettings),
            PermissionRequestDecision::Unsupported => Err(PermissionRequestError::Unsupported),
        }
    }

    pub fn finish_request(&self, capability: PermissionCapability, state: PermissionState) {
        if let Ok(mut inner) = self.state.lock() {
            inner.in_flight.remove(&capability);
            let _ = inner.registry.observe(capability, state);
        }
    }

    pub fn mark_request_failed(&self, capability: PermissionCapability) {
        if let Ok(mut inner) = self.state.lock() {
            inner.in_flight.remove(&capability);
        }
    }
}

#[cfg(test)]
mod tests {
    use mega_permissions::{PermissionCapability, PermissionState};

    use super::{PermissionCoordinator, PermissionRequestError};

    #[test]
    fn repeated_request_is_rejected_until_os_observation_finishes_it() {
        let coordinator = PermissionCoordinator::new();
        let capability = PermissionCapability::Accessibility;
        coordinator.observe(capability, PermissionState::NotRequested);
        coordinator.begin_request(capability, false).unwrap();
        assert_eq!(
            coordinator.begin_request(capability, false),
            Err(PermissionRequestError::AlreadyRequesting)
        );
        coordinator.finish_request(capability, PermissionState::Denied);
        assert_eq!(
            coordinator.begin_request(capability, true),
            Err(PermissionRequestError::OpenSettings)
        );
    }

    #[test]
    fn granted_state_is_terminal_and_never_prompts() {
        let coordinator = PermissionCoordinator::new();
        let capability = PermissionCapability::Microphone;
        coordinator.observe(capability, PermissionState::Granted);
        assert_eq!(
            coordinator.begin_request(capability, false),
            Err(PermissionRequestError::AlreadyGranted)
        );
    }
}
