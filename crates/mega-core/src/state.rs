use std::collections::BTreeMap;

use mega_permissions::{PermissionCapability, PermissionRegistry, PermissionSnapshot};
use serde::{Deserialize, Serialize};

use crate::{CaptureHealth, LifecycleState, Subsystem, SubsystemHealth};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InfrastructureState {
    pub lifecycle: LifecycleState,
    pub subsystems: BTreeMap<Subsystem, SubsystemHealth>,
    pub permissions: PermissionSnapshot,
    pub capture: CaptureHealth,
    pub audio: crate::AudioHealth,
}

impl Default for InfrastructureState {
    fn default() -> Self {
        Self::new()
    }
}

impl InfrastructureState {
    pub fn new() -> Self {
        let mut subsystems = BTreeMap::new();
        for subsystem in [
            Subsystem::Runtime,
            Subsystem::ScreenCapture,
            Subsystem::Accessibility,
            Subsystem::Audio,
            Subsystem::Permissions,
            Subsystem::Ipc,
        ] {
            subsystems.insert(subsystem, SubsystemHealth::default());
        }

        let permissions = PermissionRegistry::new().snapshot();
        Self {
            lifecycle: LifecycleState::Stopped,
            subsystems,
            permissions,
            capture: CaptureHealth::with_capacity(5),
            audio: crate::AudioHealth::with_capacity(48_000 * 3),
        }
    }

    pub fn permission(
        &self,
        capability: PermissionCapability,
    ) -> mega_permissions::PermissionState {
        self.permissions
            .get(capability)
            .unwrap_or(mega_permissions::PermissionState::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::InfrastructureState;
    use crate::{CaptureState, PermissionCapability, PermissionState, Subsystem};

    #[test]
    fn default_state_is_stopped_and_declares_all_infrastructure_subsystems() {
        let state = InfrastructureState::new();

        assert_eq!(state.lifecycle, crate::LifecycleState::Stopped);
        assert_eq!(state.capture.state, CaptureState::Stopped);
        assert_eq!(
            state.permission(PermissionCapability::Microphone),
            PermissionState::Unknown
        );
        assert!(state.subsystems.contains_key(&Subsystem::Audio));
        assert!(state.subsystems.contains_key(&Subsystem::Ipc));
    }
}
