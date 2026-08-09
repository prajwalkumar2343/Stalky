use block2::RcBlock;
use mega_core::{PermissionCapability, PermissionState};
use objc2::runtime::Bool;
use objc2_application_services::AXIsProcessTrusted;
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use std::sync::mpsc;
use std::time::Duration;

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

pub(crate) fn request_permission(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    match capability {
        PermissionCapability::ScreenRecording => {
            // CGRequestScreenCaptureAccess is the explicit native request. A
            // subsequent preflight remains the source of truth for the result.
            let _ = CGRequestScreenCaptureAccess();
            Ok(normalize_boolean_permission(
                CGPreflightScreenCaptureAccess(),
            ))
        }
        PermissionCapability::Microphone => {
            let (sender, receiver) = mpsc::sync_channel(1);
            let response = RcBlock::<dyn Fn(Bool)>::new(move |granted: Bool| {
                let _ = sender.send(granted.as_bool());
            });
            // SAFETY: The AVFAudio singleton owns and invokes the retained
            // completion block; the result is copied across a bounded channel.
            unsafe {
                AVAudioApplication::requestRecordPermissionWithCompletionHandler(&response);
            }
            let granted = receiver
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| {
                    PlatformError::request_timeout(PlatformFeature::MicrophonePermission)
                })?;
            Ok(normalize_boolean_permission(granted))
        }
        PermissionCapability::Accessibility | PermissionCapability::LaunchAtLogin => {
            Err(PlatformError::unsupported(match capability {
                PermissionCapability::Accessibility => PlatformFeature::AccessibilityPermission,
                PermissionCapability::LaunchAtLogin => PlatformFeature::LaunchAtLoginPermission,
                PermissionCapability::ScreenRecording | PermissionCapability::Microphone => {
                    unreachable!()
                }
            }))
        }
    }
}
