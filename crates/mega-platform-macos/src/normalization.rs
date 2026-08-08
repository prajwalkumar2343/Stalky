use mega_core::{DisplayEvent, LifecycleEvent, PermissionState, PlatformEvent};

/// `kCGDisplayAddFlag` from `CGDisplayChangeSummaryFlags`.
pub const CG_DISPLAY_ADDED_FLAG: u32 = 1 << 4;
/// `kCGDisplayRemoveFlag` from `CGDisplayChangeSummaryFlags`.
pub const CG_DISPLAY_REMOVED_FLAG: u32 = 1 << 5;
/// Configuration flags that become one normalized `DisplayEvent::Changed`.
pub const CG_DISPLAY_CHANGED_FLAGS: u32 =
    (1 << 1) | (1 << 2) | (1 << 3) | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOsMicrophonePermission {
    Undetermined,
    Denied,
    Granted,
    Unknown,
}

/// Lifecycle notifications that a macOS notification source can normalize.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOsLifecycleEvent {
    WillSleep,
    DidWake,
    ScreenLocked,
    ScreenUnlocked,
    WillLogout,
    DidLogout,
}

/// Raw display reconfiguration data before conversion to a core event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacOsDisplayChange {
    pub display_id: u32,
    pub summary_flags: u32,
}

impl MacOsDisplayChange {
    pub const fn new(display_id: u32, summary_flags: u32) -> Self {
        Self {
            display_id,
            summary_flags,
        }
    }
}

/// Platform-specific event input accepted by the pure normalizer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOsPlatformEvent {
    Lifecycle(MacOsLifecycleEvent),
    Display(MacOsDisplayChange),
}

pub const fn normalize_boolean_permission(granted: bool) -> PermissionState {
    if granted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

pub const fn normalize_microphone_permission(
    permission: MacOsMicrophonePermission,
) -> PermissionState {
    match permission {
        MacOsMicrophonePermission::Undetermined => PermissionState::NotRequested,
        MacOsMicrophonePermission::Denied => PermissionState::Denied,
        MacOsMicrophonePermission::Granted => PermissionState::Granted,
        MacOsMicrophonePermission::Unknown => PermissionState::Unknown,
    }
}

pub const fn normalize_lifecycle_event(event: MacOsLifecycleEvent) -> LifecycleEvent {
    match event {
        MacOsLifecycleEvent::WillSleep => LifecycleEvent::WillSleep,
        MacOsLifecycleEvent::DidWake => LifecycleEvent::DidWake,
        MacOsLifecycleEvent::ScreenLocked => LifecycleEvent::ScreenLocked,
        MacOsLifecycleEvent::ScreenUnlocked => LifecycleEvent::ScreenUnlocked,
        MacOsLifecycleEvent::WillLogout => LifecycleEvent::WillLogout,
        MacOsLifecycleEvent::DidLogout => LifecycleEvent::DidLogout,
    }
}

pub fn normalize_display_change(change: MacOsDisplayChange) -> Option<DisplayEvent> {
    let flags = change.summary_flags;
    let display_id = u64::from(change.display_id);

    if flags & CG_DISPLAY_ADDED_FLAG != 0 && flags & CG_DISPLAY_REMOVED_FLAG == 0 {
        Some(DisplayEvent::Added { display_id })
    } else if flags & CG_DISPLAY_REMOVED_FLAG != 0 && flags & CG_DISPLAY_ADDED_FLAG == 0 {
        Some(DisplayEvent::Removed { display_id })
    } else if flags & (CG_DISPLAY_CHANGED_FLAGS | CG_DISPLAY_ADDED_FLAG | CG_DISPLAY_REMOVED_FLAG)
        != 0
    {
        Some(DisplayEvent::Changed { display_id })
    } else {
        None
    }
}

pub fn normalize_platform_event(event: MacOsPlatformEvent) -> Option<PlatformEvent> {
    match event {
        MacOsPlatformEvent::Lifecycle(event) => {
            Some(PlatformEvent::Lifecycle(normalize_lifecycle_event(event)))
        }
        MacOsPlatformEvent::Display(change) => {
            normalize_display_change(change).map(PlatformEvent::Display)
        }
    }
}
