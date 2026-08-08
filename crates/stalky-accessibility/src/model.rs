use mega_permissions::PermissionState;
use serde::{Deserialize, Serialize};

pub const MAX_RECENT_EVENTS: usize = 100;
pub const MAX_DIAGNOSTIC_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityState {
    #[default]
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityAction {
    Press,
    Increment,
    Decrement,
    ShowMenu,
    Raise,
    Focus,
    SetValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityElementId {
    pub id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityActionRequest {
    pub element: AccessibilityElementId,
    pub action: AccessibilityAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityActionResult {
    pub executed: bool,
    pub element: AccessibilityElementId,
    pub action: AccessibilityAction,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityNode {
    pub element: Option<AccessibilityElementId>,
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub title: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<AccessibilityRect>,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub children_count: usize,
    pub children: Vec<Self>,
    pub truncated: bool,
    pub supported_actions: Vec<AccessibilityAction>,
    pub value_settable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityApplication {
    pub pid: i32,
    pub name: Option<String>,
    pub bundle_identifier: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityEventKind {
    FocusedApplication,
    FocusedWindow,
    FocusedElement,
    WindowCreated,
    ValueChanged,
    SelectionChanged,
    TitleChanged,
    ElementDestroyed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityEvent {
    pub sequence: u64,
    pub kind: AccessibilityEventKind,
    pub element: Option<AccessibilityElementId>,
    pub observed_at_millis: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityMetrics {
    pub observed_events: u64,
    pub dropped_events: u64,
    pub errors: u64,
    pub stale_events: u64,
    pub unsupported_notifications: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilitySnapshot {
    pub generation: u64,
    pub observed_at_millis: u64,
    pub application: Option<AccessibilityApplication>,
    pub focused_window: Option<AccessibilityNode>,
    pub focused_element: Option<AccessibilityNode>,
    pub tree: Option<AccessibilityNode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityStatus {
    pub state: AccessibilityState,
    pub permission: PermissionState,
    pub snapshot: Option<AccessibilitySnapshot>,
    pub recent_events: Vec<AccessibilityEvent>,
    pub metrics: AccessibilityMetrics,
    pub last_error: Option<String>,
}

impl Default for AccessibilityStatus {
    fn default() -> Self {
        Self {
            state: AccessibilityState::Stopped,
            permission: PermissionState::Unknown,
            snapshot: None,
            recent_events: Vec::new(),
            metrics: AccessibilityMetrics::default(),
            last_error: None,
        }
    }
}
