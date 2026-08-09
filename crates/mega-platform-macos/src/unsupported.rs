use mega_core::{PermissionCapability, PermissionState};

use crate::{PlatformError, PlatformFeature};

pub(crate) fn permission_status(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    let feature = match capability {
        PermissionCapability::Accessibility => PlatformFeature::AccessibilityPermission,
        PermissionCapability::ScreenRecording => PlatformFeature::ScreenRecordingPermission,
        PermissionCapability::Microphone => PlatformFeature::MicrophonePermission,
        PermissionCapability::LaunchAtLogin => PlatformFeature::LaunchAtLoginPermission,
    };
    Err(PlatformError::unsupported(feature))
}

pub(crate) fn request_permission(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    permission_status(capability)
}
