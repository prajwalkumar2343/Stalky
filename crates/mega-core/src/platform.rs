use serde::{Deserialize, Serialize};

/// Normalized lifecycle notifications emitted by a platform adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleEvent {
    WillSleep,
    DidWake,
    ScreenLocked,
    ScreenUnlocked,
    WillLogout,
    DidLogout,
}

/// Normalized display topology notifications emitted by a platform adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayEvent {
    Added { display_id: u64 },
    Removed { display_id: u64 },
    Changed { display_id: u64 },
}

/// Platform lifecycle and display events after adapter-specific normalization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformEvent {
    Lifecycle(LifecycleEvent),
    Display(DisplayEvent),
}
