//! Typed, user-driven permission state machines for protected Stalky capabilities.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCapability {
    ScreenRecording,
    Accessibility,
    Microphone,
    LaunchAtLogin,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    #[default]
    Unknown,
    NotRequested,
    Requesting,
    Granted,
    Denied,
    Restricted,
    RestartRequired,
    Revoked,
    Unsupported,
}

pub type PermissionStatus = PermissionState;

impl PermissionState {
    pub fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }

    pub fn needs_user_action(self) -> bool {
        matches!(
            self,
            Self::NotRequested | Self::Denied | Self::Restricted | Self::Revoked
        )
    }
}

/// The only legal outcomes for an explicit permission request intent.
///
/// This policy deliberately distinguishes a first request from recovery after
/// a denial. The latter always routes the user to System Settings instead of
/// repeatedly invoking a prompt-capable native API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionRequestDecision {
    Request,
    AlreadyGranted,
    AlreadyRequesting,
    OpenSettings,
    Unsupported,
}

pub const fn permission_request_decision(
    state: PermissionState,
    has_requested: bool,
) -> PermissionRequestDecision {
    match state {
        PermissionState::Granted => PermissionRequestDecision::AlreadyGranted,
        PermissionState::Requesting => PermissionRequestDecision::AlreadyRequesting,
        PermissionState::Unsupported => PermissionRequestDecision::Unsupported,
        PermissionState::Restricted | PermissionState::Revoked => {
            PermissionRequestDecision::OpenSettings
        }
        PermissionState::Denied if has_requested => PermissionRequestDecision::OpenSettings,
        PermissionState::Unknown
        | PermissionState::NotRequested
        | PermissionState::Denied
        | PermissionState::RestartRequired => PermissionRequestDecision::Request,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChangeReason {
    InitialObservation,
    UserRequested,
    OsObservation,
    UserRevoked,
    RestartRequiredByOs,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionEvent {
    pub capability: PermissionCapability,
    pub from: PermissionState,
    pub to: PermissionState,
    pub reason: PermissionChangeReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub enum PermissionAction {
    BeginRequest,
    Observe(PermissionState),
    MarkRevoked,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PermissionTransitionError {
    #[error("cannot begin a permission request from {state:?}")]
    RequestNotAllowed { state: PermissionState },
    #[error("cannot revoke permission from {state:?}")]
    RevokeNotAllowed { state: PermissionState },
    #[error("the operating system cannot report Requesting as an observed permission state")]
    InvalidObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionMachine {
    capability: PermissionCapability,
    state: PermissionState,
}

impl PermissionMachine {
    pub fn new(capability: PermissionCapability) -> Self {
        Self {
            capability,
            state: PermissionState::Unknown,
        }
    }

    pub const fn capability(&self) -> PermissionCapability {
        self.capability
    }

    pub const fn state(&self) -> PermissionState {
        self.state
    }

    pub fn apply(
        &mut self,
        action: PermissionAction,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        let (next, reason) = match action {
            PermissionAction::BeginRequest => {
                if !matches!(
                    self.state,
                    PermissionState::Unknown
                        | PermissionState::NotRequested
                        | PermissionState::Denied
                        | PermissionState::Revoked
                ) {
                    return Err(PermissionTransitionError::RequestNotAllowed { state: self.state });
                }
                (
                    PermissionState::Requesting,
                    PermissionChangeReason::UserRequested,
                )
            }
            PermissionAction::Observe(next) => {
                if matches!(next, PermissionState::Requesting) {
                    return Err(PermissionTransitionError::InvalidObservation);
                }
                (next, PermissionChangeReason::OsObservation)
            }
            PermissionAction::MarkRevoked => {
                if !matches!(self.state, PermissionState::Granted) {
                    return Err(PermissionTransitionError::RevokeNotAllowed { state: self.state });
                }
                (
                    PermissionState::Revoked,
                    PermissionChangeReason::UserRevoked,
                )
            }
        };

        if self.state == next {
            return Ok(None);
        }

        let event = PermissionEvent {
            capability: self.capability,
            from: self.state,
            to: next,
            reason,
        };
        self.state = next;
        Ok(Some(event))
    }

    pub fn observe(
        &mut self,
        state: PermissionState,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.apply(PermissionAction::Observe(state))
    }

    pub fn begin_request(&mut self) -> Result<PermissionEvent, PermissionTransitionError> {
        self.apply(PermissionAction::BeginRequest)?
            .ok_or(PermissionTransitionError::RequestNotAllowed { state: self.state })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionSnapshot {
    states: BTreeMap<PermissionCapability, PermissionState>,
}

impl PermissionSnapshot {
    pub fn get(&self, capability: PermissionCapability) -> Option<PermissionState> {
        self.states.get(&capability).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (PermissionCapability, PermissionState)> + '_ {
        self.states
            .iter()
            .map(|(capability, state)| (*capability, *state))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRegistry {
    machines: BTreeMap<PermissionCapability, PermissionMachine>,
}

impl Default for PermissionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionRegistry {
    pub fn new() -> Self {
        let capabilities = [
            PermissionCapability::ScreenRecording,
            PermissionCapability::Accessibility,
            PermissionCapability::Microphone,
            PermissionCapability::LaunchAtLogin,
        ];
        let machines = capabilities
            .into_iter()
            .map(|capability| (capability, PermissionMachine::new(capability)))
            .collect();
        Self { machines }
    }

    pub fn state(&self, capability: PermissionCapability) -> PermissionState {
        self.machines
            .get(&capability)
            .map(PermissionMachine::state)
            .unwrap_or(PermissionState::Unknown)
    }

    pub fn observe(
        &mut self,
        capability: PermissionCapability,
        state: PermissionState,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.machine_mut(capability)?.observe(state)
    }

    pub fn begin_request(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<PermissionEvent, PermissionTransitionError> {
        self.machine_mut(capability)?.begin_request()
    }

    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            states: self
                .machines
                .iter()
                .map(|(capability, machine)| (*capability, machine.state()))
                .collect(),
        }
    }

    fn machine_mut(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<&mut PermissionMachine, PermissionTransitionError> {
        self.machines
            .get_mut(&capability)
            .ok_or(PermissionTransitionError::RequestNotAllowed {
                state: PermissionState::Unknown,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionCapability, PermissionRegistry, PermissionState, PermissionTransitionError,
    };

    #[test]
    fn request_and_os_observation_are_distinct_transitions() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Microphone;

        let requested = registry.begin_request(capability).unwrap();
        assert_eq!(requested.from, PermissionState::Unknown);
        assert_eq!(requested.to, PermissionState::Requesting);

        let observed = registry
            .observe(capability, PermissionState::Granted)
            .unwrap();
        assert_eq!(observed.unwrap().to, PermissionState::Granted);
        assert!(registry.state(capability).is_granted());
    }

    #[test]
    fn requesting_cannot_be_reentered_without_a_new_observation() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Accessibility;
        registry.begin_request(capability).unwrap();

        assert_eq!(
            registry.begin_request(capability),
            Err(PermissionTransitionError::RequestNotAllowed {
                state: PermissionState::Requesting,
            })
        );
    }

    #[test]
    fn snapshot_contains_only_known_protected_capabilities() {
        let snapshot = PermissionRegistry::new().snapshot();

        assert_eq!(snapshot.iter().count(), 4);
        assert_eq!(
            snapshot.get(PermissionCapability::ScreenRecording),
            Some(PermissionState::Unknown)
        );
    }

    #[test]
    fn denied_permission_is_requestable_once_then_routes_to_settings() {
        assert_eq!(
            super::permission_request_decision(PermissionState::Denied, false),
            super::PermissionRequestDecision::Request
        );
        assert_eq!(
            super::permission_request_decision(PermissionState::Denied, true),
            super::PermissionRequestDecision::OpenSettings
        );
    }

    #[test]
    fn restricted_and_revoked_permissions_always_route_to_settings() {
        for state in [PermissionState::Restricted, PermissionState::Revoked] {
            assert_eq!(
                super::permission_request_decision(state, false),
                super::PermissionRequestDecision::OpenSettings
            );
            assert_eq!(
                super::permission_request_decision(state, true),
                super::PermissionRequestDecision::OpenSettings
            );
        }
    }

    #[test]
    fn requesting_and_unsupported_capabilities_never_reenter_request_path() {
        assert_eq!(
            super::permission_request_decision(PermissionState::Requesting, false),
            super::PermissionRequestDecision::AlreadyRequesting
        );
        assert_eq!(
            super::permission_request_decision(PermissionState::Unsupported, false),
            super::PermissionRequestDecision::Unsupported
        );
    }
}
