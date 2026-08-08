use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use mega_core::{PermissionCapability, PermissionState};
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
use objc2_core_foundation::CFDictionary;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::{MainThreadMarker, NSString, NSURL};

use crate::{
    MacOsMicrophonePermission, PlatformError, PlatformFeature, PlatformOperation,
    normalize_boolean_permission, normalize_microphone_permission,
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
    }
}

pub(crate) fn request_permission(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    match capability {
        PermissionCapability::Accessibility => {
            let trusted = run_on_main(
                PlatformFeature::AccessibilityPermission,
                PlatformOperation::Request,
                || {
                    let key = unsafe { kAXTrustedCheckOptionPrompt };
                    let value =
                        unsafe { objc2_core_foundation::kCFBooleanTrue }.ok_or_else(|| {
                            PlatformError::native(
                                PlatformFeature::AccessibilityPermission,
                                PlatformOperation::Request,
                                "Accessibility prompt value is unavailable",
                            )
                        })?;
                    let options = CFDictionary::<
                        objc2_core_foundation::CFString,
                        objc2_core_foundation::CFBoolean,
                    >::from_slices(&[key], &[value]);
                    let options: &CFDictionary = options.as_ref();
                    Ok::<bool, PlatformError>(unsafe {
                        AXIsProcessTrustedWithOptions(Some(options))
                    })
                },
            )?;
            Ok(if trusted {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            })
        }
        PermissionCapability::ScreenRecording => {
            let granted = run_on_main(
                PlatformFeature::ScreenRecordingPermission,
                PlatformOperation::Request,
                || Ok::<bool, PlatformError>(CGRequestScreenCaptureAccess()),
            )?;
            Ok(normalize_boolean_permission(granted))
        }
        PermissionCapability::Microphone => request_microphone_permission(),
    }
}

fn request_microphone_permission() -> Result<PermissionState, PlatformError> {
    if MainThreadMarker::new().is_some() {
        return Err(PlatformError::native(
            PlatformFeature::MicrophonePermission,
            PlatformOperation::Request,
            "microphone permission requests must run off the macOS main thread",
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    run_on_main(
        PlatformFeature::MicrophonePermission,
        PlatformOperation::Request,
        move || {
            let completion: RcBlock<dyn Fn(Bool)> = RcBlock::new(move |granted: Bool| {
                let _ = sender.send(granted.as_bool());
            });
            unsafe {
                AVAudioApplication::requestRecordPermissionWithCompletionHandler(&completion);
            }
            Ok::<(), PlatformError>(())
        },
    )?;

    let granted = receiver
        .recv_timeout(Duration::from_secs(60))
        .map_err(|_| {
            PlatformError::native(
                PlatformFeature::MicrophonePermission,
                PlatformOperation::Request,
                "Microphone permission request timed out",
            )
        })?;
    Ok(if granted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    })
}

pub(crate) fn open_permission_settings(
    capability: PermissionCapability,
) -> Result<(), PlatformError> {
    let url = settings_url(capability);
    run_on_main(
        feature_for(capability),
        PlatformOperation::OpenSettings,
        move || {
            let url_string = NSString::from_str(url);
            let url = NSURL::URLWithString(&url_string).ok_or_else(|| {
                PlatformError::native(
                    feature_for(capability),
                    PlatformOperation::OpenSettings,
                    "could not construct the System Settings URL",
                )
            })?;
            let workspace = NSWorkspace::sharedWorkspace();
            if workspace.openURL(&url) {
                return Ok(());
            }

            // The privacy anchors are not a stable public API across macOS
            // releases. Fall back to the general Privacy & Security pane, then
            // to System Settings itself so the user always has a recovery path.
            for fallback in [
                "x-apple.systempreferences:com.apple.preference.security",
                "x-apple.systempreferences:",
            ] {
                let fallback_string = NSString::from_str(fallback);
                let Some(fallback_url) = NSURL::URLWithString(&fallback_string) else {
                    continue;
                };
                if workspace.openURL(&fallback_url) {
                    return Ok(());
                }
            }

            Err(PlatformError::native(
                feature_for(capability),
                PlatformOperation::OpenSettings,
                "System Settings could not be opened; open System Settings > Privacy & Security and select the requested capability",
            ))
        },
    )
}

fn settings_url(capability: PermissionCapability) -> &'static str {
    match capability {
        PermissionCapability::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionCapability::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PermissionCapability::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
    }
}

fn feature_for(capability: PermissionCapability) -> PlatformFeature {
    match capability {
        PermissionCapability::Accessibility => PlatformFeature::AccessibilityPermission,
        PermissionCapability::ScreenRecording => PlatformFeature::ScreenRecordingPermission,
        PermissionCapability::Microphone => PlatformFeature::MicrophonePermission,
    }
}

fn run_on_main<T: Send + 'static>(
    feature: PlatformFeature,
    operation: PlatformOperation,
    work: impl FnOnce() -> Result<T, PlatformError> + Send + 'static,
) -> Result<T, PlatformError> {
    if MainThreadMarker::new().is_some() {
        return work();
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| {
            PlatformError::native(
                feature,
                operation,
                "the macOS main queue did not complete the permission operation",
            )
        })?
}
