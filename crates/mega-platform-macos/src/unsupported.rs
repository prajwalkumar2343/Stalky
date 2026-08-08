use mega_core::{PermissionCapability, PermissionState};

use crate::{PlatformError, PlatformFeature, PlatformOperation};

pub(crate) fn permission_status(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    let feature = match capability {
        PermissionCapability::Accessibility => PlatformFeature::AccessibilityPermission,
        PermissionCapability::ScreenRecording => PlatformFeature::ScreenRecordingPermission,
        PermissionCapability::Microphone => PlatformFeature::MicrophonePermission,
    };
    Err(PlatformError::unsupported(feature))
}

pub(crate) fn request_permission(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    let feature = feature_for(capability);
    Err(PlatformError::Native {
        feature,
        operation: PlatformOperation::Request,
        message: "privacy permissions are available only on macOS".to_owned(),
    })
}

pub(crate) fn open_permission_settings(
    capability: PermissionCapability,
) -> Result<(), PlatformError> {
    let feature = feature_for(capability);
    Err(PlatformError::Native {
        feature,
        operation: PlatformOperation::OpenSettings,
        message: "System Settings privacy panes are available only on macOS".to_owned(),
    })
}

fn feature_for(capability: PermissionCapability) -> PlatformFeature {
    match capability {
        PermissionCapability::Accessibility => PlatformFeature::AccessibilityPermission,
        PermissionCapability::ScreenRecording => PlatformFeature::ScreenRecordingPermission,
        PermissionCapability::Microphone => PlatformFeature::MicrophonePermission,
    }
}
