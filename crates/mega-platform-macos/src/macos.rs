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
/// `CGPreflightScreenCaptureAccess` answers from the SkyLight cache recorded
/// when this process last requested permission, so it goes stale in both
/// directions. The private `TCCAccessPreflight` call asks tccd at call time:
/// a live grant with a stale cached denial means the app must restart before
/// capture works, and a live denial with a cached grant is a lapsed or
/// revoked permission.
fn screen_recording_permission_state() -> PermissionState {
    let cached = CGPreflightScreenCaptureAccess();
    match live_screen_recording_preflight() {
        Some(live) if live && cached => PermissionState::Granted,
        Some(live) if live && !cached => PermissionState::RestartRequired,
        Some(live) if !live && cached => PermissionState::Revoked,
        _ => PermissionState::Denied,
    }
}

/// Asks tccd directly for the current Screen Recording verdict via the
/// private TCC framework, bypassing the SkyLight cache used by
/// `CGPreflightScreenCaptureAccess`. Side-effect free: this is a preflight,
/// not a request. `None` means the probe could not be performed.
fn live_screen_recording_preflight() -> Option<bool> {
    type TccAccessPreflight = unsafe extern "C" fn(*const c_void) -> u32;
    const TCC_FRAMEWORK: &[u8] = b"/System/Library/PrivateFrameworks/TCC.framework/TCC\0";
    const PREFLIGHT_SYMBOL: &[u8] = b"TCCAccessPreflight\0";
    const SCREEN_CAPTURE_SERVICE_SYMBOL: &[u8] = b"kTCCServiceScreenCapture\0";
    const TCC_PREFLIGHT_GRANTED: u32 = 0;

    // SAFETY: dlopen/dlsym load the private TCC preflight entry point. The
    // function pointer is called with the kTCCServiceScreenCapture service
    // constant, matching how the public preflight helpers are implemented.
    unsafe {
        let handle = libc::dlopen(
            TCC_FRAMEWORK.as_ptr().cast(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        if handle.is_null() {
            return None;
        }
        let preflight_symbol = libc::dlsym(handle, PREFLIGHT_SYMBOL.as_ptr().cast());
        let service_symbol = libc::dlsym(handle, SCREEN_CAPTURE_SERVICE_SYMBOL.as_ptr().cast());
        if preflight_symbol.is_null() || service_symbol.is_null() {
            libc::dlclose(handle);
            return None;
        }
        let preflight: TccAccessPreflight = std::mem::transmute(preflight_symbol);
        let service = *(service_symbol as *const *const c_void);
        let granted = preflight(service) == TCC_PREFLIGHT_GRANTED;
        libc::dlclose(handle);
        Some(granted)
    }
}

/// Live Accessibility trust for the current process.
///
/// `AXIsProcessTrusted` caches its answer in-process on recent macOS
/// versions, so a grant made in System Settings while Stalky is running keeps
/// reading as denied until relaunch. An active `CGEventTap` can only be
/// created when tccd grants accessibility at call time; the tap is disabled
/// and released before it is ever attached to a run loop, so it never sits in
/// the event path. This enrolls the app in the Accessibility pane and can
/// surface the system prompt, so it must only be called while the user is
/// actively being asked for the permission, never from passive polling.
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
            // CGRequestScreenCaptureAccess is the explicit native request. A
            // subsequent comparison of the cached preflight against live TCC
            // remains the source of truth for the result.
            let _ = CGRequestScreenCaptureAccess();
            Ok(screen_recording_permission_state())
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
