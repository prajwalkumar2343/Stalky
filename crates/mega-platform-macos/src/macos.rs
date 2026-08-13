use block2::RcBlock;
use mega_core::{PermissionCapability, PermissionState};
use objc2::runtime::Bool;
use objc2_application_services::AXIsProcessTrusted;
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use std::ffi::c_void;
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
        PermissionCapability::ScreenRecording => Ok(screen_recording_permission_state()),
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

/// Screen Recording trust for the current process, distinguishing grants that
/// still need a relaunch and grants that were revoked after the app started.
///
/// Public Screen Recording trust for the current process.
///
/// A successful capture start remains the authoritative runtime signal. This
/// passive status query deliberately uses only Apple's supported preflight API;
/// production builds must not load or call the private TCC framework.
fn screen_recording_permission_state() -> PermissionState {
    normalize_boolean_permission(CGPreflightScreenCaptureAccess())
}

/// One-shot ScreenCaptureKit enumeration used as a live capture probe.
///
/// `SCShareableContent.current` requires Screen Recording permission to
/// succeed: without it, ScreenCaptureKit fails the request with its
/// permission-denied error, and with it the enumeration completes. The probe
/// never starts a stream, so it has no side effects beyond a brief
/// enumeration, and it reports the strongest possible evidence: can capture
/// start right now?
fn screen_capture_probe() -> bool {
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_foundation::NSError;
    use objc2_screen_capture_kit::SCShareableContent;

    let (sender, receiver) = mpsc::sync_channel(1);
    let completion: RcBlock<dyn Fn(*mut SCShareableContent, *mut NSError)> = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let succeeded = if let Some(content) = unsafe { content.as_ref() } {
                // SAFETY: ScreenCaptureKit owns the block for the duration of
                // the call; the content object is retained only to keep it
                // alive until the probe verdict is delivered.
                unsafe { Retained::retain(content as *const _ as *mut SCShareableContent) }
                    .is_some()
            } else {
                if let Some(error) = unsafe { error.as_ref() } {
                    eprintln!(
                        "Stalky screen capture probe failed: {}",
                        error.localizedDescription()
                    );
                }
                false
            };
            let _ = sender.send(succeeded);
        },
    );
    // SAFETY: The completion block is heap-backed and remains alive for the
    // duration of the Objective-C asynchronous request.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
    receiver.recv_timeout(PROBE_TIMEOUT).unwrap_or(false)
}

/// Live Accessibility trust for the current process.
///
/// `AXIsProcessTrusted` caches its answer in-process on recent macOS
/// versions, so a grant made in System Settings while Stalky is running keeps
/// reading as denied until relaunch. Any of the following live signals is
/// accepted as granted:
///
/// - `AXIsProcessTrusted` — the in-process cache (true after a prompt grant
///   or a relaunch);
/// - an active event tap — only creatable when tccd grants Accessibility at
///   call time.
///
/// Creating an active tap can surface the system prompt and enroll the app in
/// the Accessibility pane, so this probe must only be called while the user
/// is actively being asked for the permission, never from passive polling.
pub(crate) fn accessibility_permission_status_live() -> PermissionState {
    if unsafe { AXIsProcessTrusted() } || event_tap_probe() {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

extern "C" fn noop_event_callback(
    _proxy: *mut c_void,
    _event_type: u32,
    event: *mut c_void,
    _user_info: *mut c_void,
) -> *mut c_void {
    event
}

/// Creates (then immediately disables and releases) an active event tap.
/// Creating an *active* tap requires kTCCServiceAccessibility specifically —
/// a listen-only tap would succeed with Input Monitoring alone and report a
/// false grant.
fn event_tap_probe() -> bool {
    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
    const K_CG_EVENT_KEY_DOWN: u64 = 10;

    // SAFETY: The event tap is created with a no-op callback and no user
    // info, never attached to a run loop, then disabled, invalidated, and
    // released before it can receive or forward any events.
    unsafe {
        let tap = CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            1u64 << K_CG_EVENT_KEY_DOWN,
            noop_event_callback,
            std::ptr::null_mut(),
        );
        if tap.is_null() {
            return false;
        }
        CGEventTapEnable(tap, false);
        CFMachPortInvalidate(tap);
        CFRelease(tap as *const c_void);
        true
    }
}

unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CFRelease(cf: *const c_void);
    fn CFMachPortInvalidate(port: *mut c_void);
}

pub(crate) fn request_permission(
    capability: PermissionCapability,
) -> Result<PermissionState, PlatformError> {
    match capability {
        PermissionCapability::ScreenRecording => {
            let granted = CGRequestScreenCaptureAccess();
            if !granted {
                return Ok(PermissionState::Denied);
            }
            let observed = screen_recording_permission_state();
            Ok(if observed.is_granted() {
                observed
            } else {
                PermissionState::RestartRequired
            })
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

#[doc(hidden)]
pub mod diagnostics {
    use super::*;

    /// Raw probe values for the current process, used by the `probe` example
    /// to verify what macOS reports for the app's own code identity.
    pub struct ProbeDiagnostics {
        pub ax_is_process_trusted: bool,
        pub event_tap_probe: bool,
        pub cg_preflight_screen: bool,
        pub sck_probe: bool,
        pub screen_state: PermissionState,
        pub accessibility_live_state: PermissionState,
        pub microphone_state: PermissionState,
    }

    impl ProbeDiagnostics {
        pub fn capture() -> Self {
            let cg_preflight_screen = CGPreflightScreenCaptureAccess();
            let sck_probe = cg_preflight_screen && screen_capture_probe();
            Self {
                ax_is_process_trusted: unsafe { AXIsProcessTrusted() },
                event_tap_probe: event_tap_probe(),
                cg_preflight_screen,
                sck_probe,
                screen_state: screen_recording_permission_state(),
                accessibility_live_state: accessibility_permission_status_live(),
                microphone_state: super::permission_status(PermissionCapability::Microphone)
                    .unwrap_or(PermissionState::Unknown),
            }
        }
    }
}
