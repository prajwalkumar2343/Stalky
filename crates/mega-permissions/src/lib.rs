//! Typed, user-driven permission state machines for protected Stalky capabilities.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Privacy permissions that protect Stalky's active capabilities.
///
/// Launch at Login is intentionally not part of this enum. It is an optional
/// Service Management preference, not a TCC privacy permission.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCapability {
    ScreenRecording,
    Accessibility,
    Microphone,
}

pub const PRIVACY_CAPABILITIES: [PermissionCapability; 3] = [
    PermissionCapability::ScreenRecording,
    PermissionCapability::Accessibility,
    PermissionCapability::Microphone,
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    /// The app has not completed a trustworthy probe yet.
    #[default]
    Unknown,
    /// The OS explicitly exposes an unasked/undetermined state.
    NotDetermined,
    /// A user-initiated native request is in flight.
    Requesting,
    /// The app is checking again after returning from the OS UI.
    Rechecking,
    Granted,
    /// The user or OS has declined access. Boolean macOS probes are reported
    /// this way only after the app has enough history to distinguish it from
    /// an initial unknown result.
    Denied,
    /// Access is unavailable because of device, policy, or managed settings.
    Restricted,
    /// The current target cannot provide this capability.
    Unsupported,
    /// The app observed a loss of access after it had previously been granted.
    Revoked,
    /// A platform/runtime-specific relaunch is required before access can be
    /// used. This is never inferred from a bare false probe.
    RestartRequired,
}

pub type PermissionStatus = PermissionState;

impl PermissionState {
    pub fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }

    pub fn needs_user_action(self) -> bool {
        matches!(self, Self::NotDetermined | Self::Denied | Self::Revoked)
    }

    pub fn is_in_flight(self) -> bool {
        matches!(self, Self::Requesting | Self::Rechecking)
    }

    pub fn is_unavailable(self) -> bool {
        matches!(
            self,
            Self::Restricted | Self::Unsupported | Self::RestartRequired
        )
    }
}

/// An operation being performed by the coordinator. This is kept separate
/// from the last trustworthy OS authorization so a recheck cannot make a
/// previously granted capability look unavailable while the probe is running.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOperation {
    #[default]
    Idle,
    Requesting,
    Rechecking,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChangeReason {
    InitialObservation,
    UserRequested,
    OsObservation,
    PermissionRevoked,
    RestartRequiredByOs,
    OperationFailed,
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
    BeginRecheck,
    Observe(PermissionState),
    Recover,
    MarkRevoked,
    MarkRestartRequired,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PermissionTransitionError {
    #[error("cannot begin a permission request from {state:?}")]
    RequestNotAllowed { state: PermissionState },
    #[error("cannot begin a permission recheck from {state:?}")]
    RecheckNotAllowed { state: PermissionState },
    #[error("cannot revoke permission from {state:?}")]
    RevokeNotAllowed { state: PermissionState },
    #[error("cannot mark permission restart-required from {state:?}")]
    RestartNotAllowed { state: PermissionState },
    #[error("the operating system cannot report {state:?} as an observed permission state")]
    InvalidObservation { state: PermissionState },
    #[error("cannot recover a permission operation from {state:?}")]
    RecoveryNotAllowed { state: PermissionState },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionMachine {
    capability: PermissionCapability,
    authorization: PermissionState,
    operation: PermissionOperation,
}

impl PermissionMachine {
    pub fn new(capability: PermissionCapability) -> Self {
        Self {
            capability,
            authorization: PermissionState::Unknown,
            operation: PermissionOperation::Idle,
        }
    }

    pub const fn capability(&self) -> PermissionCapability {
        self.capability
    }

    pub const fn state(&self) -> PermissionState {
        match self.operation {
            PermissionOperation::Idle => self.authorization,
            PermissionOperation::Requesting => PermissionState::Requesting,
            PermissionOperation::Rechecking => PermissionState::Rechecking,
        }
    }

    pub const fn authorization(&self) -> PermissionState {
        self.authorization
    }

    pub const fn operation(&self) -> PermissionOperation {
        self.operation
    }

    pub fn apply(
        &mut self,
        action: PermissionAction,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        let previous = self.state();
        let (next, reason) = match action {
            PermissionAction::BeginRequest => {
                if !matches!(
                    self.authorization,
                    PermissionState::Unknown
                        | PermissionState::NotDetermined
                        | PermissionState::Denied
                        | PermissionState::Revoked
                ) {
                    return Err(PermissionTransitionError::RequestNotAllowed {
                        state: self.state(),
                    });
                }
                self.operation = PermissionOperation::Requesting;
                (
                    PermissionState::Requesting,
                    PermissionChangeReason::UserRequested,
                )
            }
            PermissionAction::BeginRecheck => {
                if !matches!(
                    self.authorization,
                    PermissionState::Unknown
                        | PermissionState::NotDetermined
                        | PermissionState::Denied
                        | PermissionState::Granted
                        | PermissionState::Revoked
                ) || self.operation != PermissionOperation::Idle
                {
                    return Err(PermissionTransitionError::RecheckNotAllowed {
                        state: self.state(),
                    });
                }
                self.operation = PermissionOperation::Rechecking;
                (
                    PermissionState::Rechecking,
                    PermissionChangeReason::OsObservation,
                )
            }
            PermissionAction::Observe(next) => {
                if matches!(
                    next,
                    PermissionState::Requesting | PermissionState::Rechecking
                ) {
                    return Err(PermissionTransitionError::InvalidObservation { state: next });
                }
                // A boolean macOS probe can collapse denied and
                // not-determined. Preserve a previously trusted authorization
                // when the new observation is merely Unknown.
                let next = if next == PermissionState::Unknown
                    && self.authorization != PermissionState::Unknown
                {
                    self.authorization
                } else {
                    next
                };
                if self.authorization == PermissionState::Granted
                    && matches!(
                        next,
                        PermissionState::Denied
                            | PermissionState::Restricted
                            | PermissionState::NotDetermined
                    )
                {
                    self.authorization = PermissionState::Revoked;
                    self.operation = PermissionOperation::Idle;
                    (
                        PermissionState::Revoked,
                        PermissionChangeReason::PermissionRevoked,
                    )
                } else {
                    let reason = if self.authorization == PermissionState::Unknown {
                        PermissionChangeReason::InitialObservation
                    } else {
                        PermissionChangeReason::OsObservation
                    };
                    self.authorization = next;
                    self.operation = PermissionOperation::Idle;
                    (next, reason)
                }
            }
            PermissionAction::Recover => {
                if self.operation == PermissionOperation::Idle {
                    return Err(PermissionTransitionError::RecoveryNotAllowed {
                        state: self.state(),
                    });
                }
                let next = self.authorization;
                self.operation = PermissionOperation::Idle;
                (next, PermissionChangeReason::OperationFailed)
            }
            PermissionAction::MarkRevoked => {
                if !matches!(self.authorization, PermissionState::Granted)
                    || self.operation != PermissionOperation::Idle
                {
                    return Err(PermissionTransitionError::RevokeNotAllowed {
                        state: self.state(),
                    });
                }
                self.authorization = PermissionState::Revoked;
                (
                    PermissionState::Revoked,
                    PermissionChangeReason::PermissionRevoked,
                )
            }
            PermissionAction::MarkRestartRequired => {
                if !matches!(self.authorization, PermissionState::Granted)
                    || self.operation != PermissionOperation::Idle
                {
                    return Err(PermissionTransitionError::RestartNotAllowed {
                        state: self.state(),
                    });
                }
                self.authorization = PermissionState::RestartRequired;
                (
                    PermissionState::RestartRequired,
                    PermissionChangeReason::RestartRequiredByOs,
                )
            }
        };

        if previous == next {
            return Ok(None);
        }

        let event = PermissionEvent {
            capability: self.capability,
            from: previous,
            to: next,
            reason,
        };
        Ok(Some(event))
    }

    pub fn observe(
        &mut self,
        state: PermissionState,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.apply(PermissionAction::Observe(state))
    }

    pub fn recover(&mut self) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.apply(PermissionAction::Recover)
    }

    pub fn begin_request(&mut self) -> Result<PermissionEvent, PermissionTransitionError> {
        self.apply(PermissionAction::BeginRequest)?.ok_or(
            PermissionTransitionError::RequestNotAllowed {
                state: self.state(),
            },
        )
    }

    pub fn begin_recheck(&mut self) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.apply(PermissionAction::BeginRecheck)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionSnapshot {
    states: BTreeMap<PermissionCapability, PermissionState>,
    authorizations: BTreeMap<PermissionCapability, PermissionState>,
    operations: BTreeMap<PermissionCapability, PermissionOperation>,
}

impl PermissionSnapshot {
    pub fn get(&self, capability: PermissionCapability) -> Option<PermissionState> {
        self.states.get(&capability).copied()
    }

    pub fn authorization(&self, capability: PermissionCapability) -> Option<PermissionState> {
        self.authorizations.get(&capability).copied()
    }

    pub fn operation(&self, capability: PermissionCapability) -> Option<PermissionOperation> {
        self.operations.get(&capability).copied()
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
        let machines = PRIVACY_CAPABILITIES
            .into_iter()
            .map(|capability| (capability, PermissionMachine::new(capability)))
            .collect();
        Self { machines }
    }

    pub fn state(&self, capability: PermissionCapability) -> PermissionState {
        self.machines
            .get(&capability)
            .map(PermissionMachine::state)
            .unwrap_or(PermissionState::Unsupported)
    }

    pub fn authorization(&self, capability: PermissionCapability) -> PermissionState {
        self.machines
            .get(&capability)
            .map(PermissionMachine::authorization)
            .unwrap_or(PermissionState::Unsupported)
    }

    pub fn operation(&self, capability: PermissionCapability) -> PermissionOperation {
        self.machines
            .get(&capability)
            .map(PermissionMachine::operation)
            .unwrap_or(PermissionOperation::Idle)
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

    pub fn begin_recheck(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.machine_mut(capability)?.begin_recheck()
    }

    pub fn mark_restart_required(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.machine_mut(capability)?
            .apply(PermissionAction::MarkRestartRequired)
    }

    pub fn mark_revoked(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.machine_mut(capability)?
            .apply(PermissionAction::MarkRevoked)
    }

    pub fn recover(
        &mut self,
        capability: PermissionCapability,
    ) -> Result<Option<PermissionEvent>, PermissionTransitionError> {
        self.machine_mut(capability)?.recover()
    }

    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            states: self
                .machines
                .iter()
                .map(|(capability, machine)| (*capability, machine.state()))
                .collect(),
            authorizations: self
                .machines
                .iter()
                .map(|(capability, machine)| (*capability, machine.authorization()))
                .collect(),
            operations: self
                .machines
                .iter()
                .map(|(capability, machine)| (*capability, machine.operation()))
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
                state: PermissionState::Unsupported,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PermissionCapability, PermissionChangeReason, PermissionEvent, PermissionMachine,
        PermissionOperation, PermissionRegistry, PermissionState, PermissionTransitionError,
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
    fn granted_permission_loss_is_reported_as_revoked() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::ScreenRecording;
        registry
            .observe(capability, PermissionState::Granted)
            .unwrap();

        let event = registry
            .observe(capability, PermissionState::Denied)
            .unwrap();

        assert_eq!(event.unwrap().to, PermissionState::Revoked);
        assert_eq!(registry.state(capability), PermissionState::Revoked);
    }

    #[test]
    fn recheck_is_a_transient_flow_state() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Microphone;
        registry.begin_recheck(capability).unwrap();
        assert_eq!(registry.state(capability), PermissionState::Rechecking);
        assert_eq!(registry.authorization(capability), PermissionState::Unknown);

        registry
            .observe(capability, PermissionState::NotDetermined)
            .unwrap();
        assert_eq!(registry.state(capability), PermissionState::NotDetermined);
    }

    #[test]
    fn snapshot_contains_only_privacy_capabilities() {
        let snapshot = PermissionRegistry::new().snapshot();

        assert_eq!(snapshot.iter().count(), 3);
        assert_eq!(
            snapshot.get(PermissionCapability::ScreenRecording),
            Some(PermissionState::Unknown)
        );
    }

    #[test]
    fn restricted_and_unsupported_are_not_requestable_or_recheckable() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Microphone;

        registry
            .observe(capability, PermissionState::Restricted)
            .unwrap();
        assert!(!PermissionState::Restricted.needs_user_action());
        assert_eq!(
            registry.begin_request(capability),
            Err(PermissionTransitionError::RequestNotAllowed {
                state: PermissionState::Restricted,
            })
        );
        assert_eq!(
            registry.begin_recheck(capability),
            Err(PermissionTransitionError::RecheckNotAllowed {
                state: PermissionState::Restricted,
            })
        );

        assert_eq!(
            registry.begin_request(PermissionCapability::ScreenRecording),
            Ok(PermissionEvent {
                capability: PermissionCapability::ScreenRecording,
                from: PermissionState::Unknown,
                to: PermissionState::Requesting,
                reason: PermissionChangeReason::UserRequested,
            })
        );
    }

    #[test]
    fn recheck_preserves_last_authorization_until_observation() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Accessibility;
        registry
            .observe(capability, PermissionState::Granted)
            .unwrap();

        registry.begin_recheck(capability).unwrap();
        assert_eq!(registry.state(capability), PermissionState::Rechecking);
        assert_eq!(registry.authorization(capability), PermissionState::Granted);
        assert_eq!(
            registry.operation(capability),
            PermissionOperation::Rechecking
        );
    }

    #[test]
    fn restart_required_only_follows_a_granted_idle_state() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::ScreenRecording;

        assert_eq!(
            registry.mark_restart_required(capability),
            Err(PermissionTransitionError::RestartNotAllowed {
                state: PermissionState::Unknown,
            })
        );

        registry
            .observe(capability, PermissionState::Granted)
            .unwrap();
        let event = registry.mark_restart_required(capability).unwrap().unwrap();
        assert_eq!(event.from, PermissionState::Granted);
        assert_eq!(event.to, PermissionState::RestartRequired);
        assert_eq!(registry.state(capability), PermissionState::RestartRequired);
        assert_eq!(
            registry.begin_recheck(capability),
            Err(PermissionTransitionError::RecheckNotAllowed {
                state: PermissionState::RestartRequired,
            })
        );
    }

    #[test]
    fn terminal_and_transient_observations_follow_the_transition_table() {
        let mut registry = PermissionRegistry::new();
        let capability = PermissionCapability::Microphone;

        assert_eq!(
            registry.observe(capability, PermissionState::Requesting),
            Err(PermissionTransitionError::InvalidObservation {
                state: PermissionState::Requesting,
            })
        );
        assert_eq!(
            registry.observe(capability, PermissionState::Rechecking),
            Err(PermissionTransitionError::InvalidObservation {
                state: PermissionState::Rechecking,
            })
        );

        registry.begin_request(capability).unwrap();
        let denied = registry
            .observe(capability, PermissionState::Denied)
            .unwrap();
        assert_eq!(denied.unwrap().to, PermissionState::Denied);
        assert_eq!(
            registry.observe(capability, PermissionState::Denied),
            Ok(None)
        );

        registry.begin_request(capability).unwrap();
        assert_eq!(
            registry.recover(capability).unwrap().unwrap().to,
            PermissionState::Denied
        );
        assert_eq!(registry.operation(capability), PermissionOperation::Idle);

        registry
            .observe(capability, PermissionState::Granted)
            .unwrap();
        assert_eq!(
            registry.mark_revoked(capability).unwrap().unwrap().to,
            PermissionState::Revoked
        );

        let mut denied_registry = PermissionRegistry::new();
        denied_registry
            .observe(capability, PermissionState::Denied)
            .unwrap();
        assert_eq!(
            denied_registry.mark_restart_required(capability),
            Err(PermissionTransitionError::RestartNotAllowed {
                state: PermissionState::Denied,
            })
        );
    }

    #[test]
    fn authorization_loss_is_only_revocation_after_a_grant() {
        let mut machine = PermissionMachine::new(PermissionCapability::Accessibility);
        machine.observe(PermissionState::Unknown).unwrap();
        assert_eq!(machine.state(), PermissionState::Unknown);
        machine.observe(PermissionState::Denied).unwrap();
        assert_eq!(machine.state(), PermissionState::Denied);
        machine.observe(PermissionState::Granted).unwrap();
        assert_eq!(machine.observe(PermissionState::Unknown), Ok(None));
        assert_eq!(machine.state(), PermissionState::Granted);
    }
}
