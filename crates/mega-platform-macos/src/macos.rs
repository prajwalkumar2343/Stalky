use mega_core::{PermissionCapability, PermissionState};
use objc2_application_services::AXIsProcessTrusted;
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

use crate::{
    MacOsMicrophonePermission, PlatformError, PlatformFeature, normalize_boolean_permission,
    normalize_microphone_permission,
};

pub(crate) fn permission_status(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    match capability {
        PermissionCapability::Accessibility => Ok(normalize_boolean_permission(
            // SAFETY: `AXIsProcessTrusted` is a nullary OS query and the
            // maintained binding does not expose caller-owned pointers.
            unsafe { AXIsProcessTrusted() },
        )),
        PermissionCapability::ScreenRecording => Ok(normalize_boolean_permission(
            CGPreflightScreenCaptureAccess(),
        )),
        PermissionCapability::Microphone => {
            // SAFETY: Both calls are read-only Objective-C messages on the
            // AVFAudio singleton and take no raw pointers or caller-owned data.
            let permission = unsafe {
                let application = AVAudioApplication::sharedInstance();
                application.recordPermission()
            };
            let permission = if permission == AVAudioApplicationRecordPermission::Undetermined {
                MacOsMicrophonePermission::Undetermined
            } else if permission == AVAudioApplicationRecordPermission::Denied {
                MacOsMicrophonePermission::Denied
            } else if permission == AVAudioApplicationRecordPermission::Granted {
                MacOsMicrophonePermission::Granted
            } else {
                MacOsMicrophonePermission::Unknown
            };
            Ok(normalize_microphone_permission(permission))
        }
        PermissionCapability::LaunchAtLogin => Err(PlatformError::unsupported(
            PlatformFeature::LaunchAtLoginPermission,
        )),
    }
}
