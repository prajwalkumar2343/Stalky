//! macOS permission probes, explicit requests, recovery, and normalized event contracts.
//!
//! This crate is the only owner of the macOS permission-framework boundary.
//! Probes remain read-only; explicit requests and System Settings recovery are
//! exposed separately. It does not observe accessibility trees, capture media,
//! or inject input. Event normalization is pure and can be used by a future
//! event source.

use mega_core::{PermissionCapability, PermissionState};
use thiserror::Error;

#[cfg(target_os = "macos")]
mod macos;
mod normalization;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use normalization::{
    CG_DISPLAY_ADDED_FLAG, CG_DISPLAY_CHANGED_FLAGS, CG_DISPLAY_REMOVED_FLAG, MacOsDisplayChange,
    MacOsLifecycleEvent, MacOsMicrophonePermission, MacOsPlatformEvent,
    normalize_boolean_permission, normalize_display_change, normalize_lifecycle_event,
    normalize_microphone_permission, normalize_platform_event,
};

/// A platform capability that this adapter can probe, request, or may reject.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformFeature {
    AccessibilityPermission,
    ScreenRecordingPermission,
    MicrophonePermission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformOperation {
    Probe,
    Request,
    OpenSettings,
}

/// Failure returned when a platform capability is unavailable in this adapter.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum PlatformError {
    #[error("{feature:?} is unsupported on this target")]
    Unsupported { feature: PlatformFeature },
    #[error("{operation:?} for {feature:?} failed: {message}")]
    Native {
        feature: PlatformFeature,
        operation: PlatformOperation,
        message: String,
    },
}

impl PlatformError {
    #[cfg(not(target_os = "macos"))]
    const fn unsupported(feature: PlatformFeature) -> Self {
        Self::Unsupported { feature }
    }

    fn native(
        feature: PlatformFeature,
        operation: PlatformOperation,
        message: impl Into<String>,
    ) -> Self {
        Self::Native {
            feature,
            operation,
            message: message.into(),
        }
    }
}

/// macOS permission adapter with separate probe, request, and recovery APIs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacOsPlatform;

impl MacOsPlatform {
    /// Creates a stateless adapter. Probes read OS state at call time.
    pub const fn new() -> Self {
        Self
    }

    /// Returns the current OS-reported state without prompting the user.
    pub fn permission_status(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError> {
        platform::permission_status(capability)
    }

    /// Reads Accessibility trust without setting the system prompt option.
    pub fn accessibility_permission_status(&self) -> Result<PermissionState, PlatformError> {
        self.permission_status(PermissionCapability::Accessibility)
    }

    /// Reads Screen Recording access using CoreGraphics preflight.
    pub fn screen_recording_permission_status(&self) -> Result<PermissionState, PlatformError> {
        self.permission_status(PermissionCapability::ScreenRecording)
    }

    /// Reads AVFAudio record permission without requesting it.
    pub fn microphone_permission_status(&self) -> Result<PermissionState, PlatformError> {
        self.permission_status(PermissionCapability::Microphone)
    }

    /// Reads all permission states supported by this adapter in one snapshot.
    pub fn permission_statuses(&self) -> Result<PermissionStatuses, PlatformError> {
        Ok(PermissionStatuses {
            accessibility: self.accessibility_permission_status()?,
            screen_recording: self.screen_recording_permission_status()?,
            microphone: self.microphone_permission_status()?,
        })
    }

    /// Performs one explicit, user-triggered native request. Probes never call
    /// these APIs, so startup and background rechecks cannot open a prompt.
    pub fn request_permission(
        &self,
        capability: PermissionCapability,
    ) -> Result<PermissionState, PlatformError> {
        platform::request_permission(capability)
    }

    /// Opens only the allowlisted macOS Privacy & Security pane for a privacy
    /// capability. The caller remains responsible for rechecking afterwards.
    pub fn open_permission_settings(
        &self,
        capability: PermissionCapability,
    ) -> Result<(), PlatformError> {
        platform::open_permission_settings(capability)
    }
}

/// The three privacy permission states exposed by this adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionStatuses {
    /// Accessibility trust for the current process.
    pub accessibility: PermissionState,
    /// Screen Recording access for the current process.
    pub screen_recording: PermissionState,
    /// Microphone record permission for the current application.
    pub microphone: PermissionState,
}

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

#[cfg(test)]
mod tests {
    use super::*;
    use mega_core::{DisplayEvent, LifecycleEvent, PlatformEvent};

    #[test]
    fn boolean_permission_normalization_is_explicit() {
        assert_eq!(normalize_boolean_permission(true), PermissionState::Granted);
        assert_eq!(normalize_boolean_permission(false), PermissionState::Denied);
    }

    #[test]
    fn microphone_permission_normalization_preserves_three_os_states() {
        assert_eq!(
            normalize_microphone_permission(MacOsMicrophonePermission::Undetermined),
            PermissionState::NotDetermined
        );
        assert_eq!(
            normalize_microphone_permission(MacOsMicrophonePermission::Denied),
            PermissionState::Denied
        );
        assert_eq!(
            normalize_microphone_permission(MacOsMicrophonePermission::Granted),
            PermissionState::Granted
        );
        assert_eq!(
            normalize_microphone_permission(MacOsMicrophonePermission::Unknown),
            PermissionState::Unknown
        );
    }

    #[test]
    fn lifecycle_events_normalize_to_core_events() {
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::WillSleep),
            LifecycleEvent::WillSleep
        );
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::DidWake),
            LifecycleEvent::DidWake
        );
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::ScreenLocked),
            LifecycleEvent::ScreenLocked
        );
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::ScreenUnlocked),
            LifecycleEvent::ScreenUnlocked
        );
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::WillLogout),
            LifecycleEvent::WillLogout
        );
        assert_eq!(
            normalize_lifecycle_event(MacOsLifecycleEvent::DidLogout),
            LifecycleEvent::DidLogout
        );
    }

    #[test]
    fn display_flags_normalize_topology_changes_and_ignore_unknown_flags() {
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(7, CG_DISPLAY_ADDED_FLAG)),
            Some(DisplayEvent::Added { display_id: 7 })
        );
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(8, CG_DISPLAY_REMOVED_FLAG)),
            Some(DisplayEvent::Removed { display_id: 8 })
        );
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(9, CG_DISPLAY_CHANGED_FLAGS)),
            Some(DisplayEvent::Changed { display_id: 9 })
        );
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(10, u32::MAX)),
            Some(DisplayEvent::Changed { display_id: 10 })
        );
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(11, 1 << 31)),
            None
        );
        assert_eq!(
            normalize_display_change(MacOsDisplayChange::new(12, 0)),
            None
        );
    }

    #[test]
    fn platform_event_normalization_keeps_event_kind() {
        assert_eq!(
            normalize_platform_event(MacOsPlatformEvent::Lifecycle(MacOsLifecycleEvent::DidWake,)),
            Some(PlatformEvent::Lifecycle(LifecycleEvent::DidWake))
        );
        assert_eq!(
            normalize_platform_event(MacOsPlatformEvent::Display(MacOsDisplayChange::new(
                42,
                CG_DISPLAY_REMOVED_FLAG,
            ))),
            Some(PlatformEvent::Display(DisplayEvent::Removed {
                display_id: 42
            }))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_binding_probe_surface_compiles_without_calling_permission_apis() {
        let _probe: fn(PermissionCapability) -> Result<PermissionState, PlatformError> =
            platform::permission_status;
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_permission_probes_are_explicitly_unsupported() {
        let platform = MacOsPlatform::new();
        for capability in [
            PermissionCapability::Accessibility,
            PermissionCapability::ScreenRecording,
            PermissionCapability::Microphone,
        ] {
            assert_eq!(
                platform.permission_status(capability),
                Err(PlatformError::Unsupported {
                    feature: match capability {
                        PermissionCapability::Accessibility => {
                            PlatformFeature::AccessibilityPermission
                        }
                        PermissionCapability::ScreenRecording => {
                            PlatformFeature::ScreenRecordingPermission
                        }
                        PermissionCapability::Microphone => PlatformFeature::MicrophonePermission,
                    },
                })
            );
        }
    }
}
