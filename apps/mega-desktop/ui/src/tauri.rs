use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(
        catch,
        js_namespace = ["window", "__TAURI__", "core"],
        js_name = invoke
    )]
    async fn invoke_without_args(command: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(
        catch,
        js_namespace = ["window", "__TAURI__", "core"],
        js_name = invoke
    )]
    async fn invoke_with_args(command: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityState {
    #[default]
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    #[default]
    Unknown,
    NotRequested,
    Requesting,
    Granted,
    Denied,
    Restricted,
    RestartRequired,
    Revoked,
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

impl AccessibilityAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Press => "Press",
            Self::Increment => "Increment",
            Self::Decrement => "Decrement",
            Self::ShowMenu => "Show menu",
            Self::Raise => "Raise",
            Self::Focus => "Focus",
            Self::SetValue => "Set value",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityElementId {
    pub id: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityApplication {
    pub pid: i32,
    pub name: Option<String>,
    pub bundle_identifier: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
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

impl AccessibilityEventKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::FocusedApplication => "Focused application",
            Self::FocusedWindow => "Focused window",
            Self::FocusedElement => "Focused element",
            Self::WindowCreated => "Window created",
            Self::ValueChanged => "Value changed",
            Self::SelectionChanged => "Selection changed",
            Self::TitleChanged => "Title changed",
            Self::ElementDestroyed => "Element removed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityEvent {
    pub sequence: u64,
    pub kind: AccessibilityEventKind,
    pub element: Option<AccessibilityElementId>,
    pub observed_at_millis: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AccessibilityMetrics {
    pub observed_events: u64,
    pub dropped_events: u64,
    pub errors: u64,
    pub stale_events: u64,
    pub unsupported_notifications: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilitySnapshot {
    pub generation: u64,
    pub observed_at_millis: u64,
    pub application: Option<AccessibilityApplication>,
    pub focused_window: Option<AccessibilityNode>,
    pub focused_element: Option<AccessibilityNode>,
    pub tree: Option<AccessibilityNode>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AccessibilityStatus {
    pub state: AccessibilityState,
    pub permission: PermissionState,
    pub snapshot: Option<AccessibilitySnapshot>,
    pub recent_events: Vec<AccessibilityEvent>,
    pub metrics: AccessibilityMetrics,
    pub last_error: Option<String>,
}

impl AccessibilityStatus {
    pub fn is_running(&self) -> bool {
        self.state == AccessibilityState::Running
    }

    pub fn needs_stop(&self) -> bool {
        matches!(
            self.state,
            AccessibilityState::Running | AccessibilityState::Failed
        )
    }
}

impl AccessibilityState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Observing",
            Self::Failed => "Needs attention",
        }
    }
}

impl PermissionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::NotRequested => "Not requested",
            Self::Requesting => "Waiting for approval",
            Self::Granted => "Granted",
            Self::Denied => "Not granted",
            Self::Restricted => "Restricted",
            Self::RestartRequired => "Restart required",
            Self::Revoked => "Revoked",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityActionRequest {
    pub element: AccessibilityElementId,
    pub action: AccessibilityAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Serialize)]
struct AccessibilityActionArgs {
    request: AccessibilityActionRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityActionResult {
    pub executed: bool,
    pub element: AccessibilityElementId,
    pub action: AccessibilityAction,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct FrameMetadata {
    pub width: usize,
    pub height: usize,
    pub timestamp_millis: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct FrameMetrics {
    pub accepted_frames: u64,
    pub invalid_frames: u64,
    pub duplicate_frames: u64,
    pub dropped_frames: u64,
    pub replaced_frames: u64,
    pub stream_errors: u64,
    pub last_frame: Option<FrameMetadata>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub metrics: FrameMetrics,
    pub last_error: Option<String>,
}

impl CaptureStatus {
    pub fn is_running(&self) -> bool {
        self.state == CaptureState::Running
    }

    pub fn needs_stop(&self) -> bool {
        matches!(self.state, CaptureState::Running | CaptureState::Failed)
    }
}

impl CaptureState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Failed => "Needs attention",
        }
    }
}

pub fn is_available() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(tauri) = js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__")) else {
        return false;
    };
    if tauri.is_null() || tauri.is_undefined() {
        return false;
    }
    js_sys::Reflect::get(&tauri, &JsValue::from_str("core"))
        .is_ok_and(|core| !core.is_null() && !core.is_undefined())
}

pub async fn capture_start() -> Result<CaptureStatus, String> {
    invoke("capture_start").await
}

pub async fn capture_stop() -> Result<CaptureStatus, String> {
    invoke("capture_stop").await
}

pub async fn capture_status() -> Result<CaptureStatus, String> {
    invoke("capture_status").await
}

pub async fn accessibility_start() -> Result<AccessibilityStatus, String> {
    invoke("accessibility_start").await
}

pub async fn accessibility_stop() -> Result<AccessibilityStatus, String> {
    invoke("accessibility_stop").await
}

pub async fn accessibility_status() -> Result<AccessibilityStatus, String> {
    invoke("accessibility_status").await
}

pub async fn accessibility_request() -> Result<PermissionState, String> {
    invoke("accessibility_request").await
}

pub async fn accessibility_action(
    request: AccessibilityActionRequest,
) -> Result<AccessibilityActionResult, String> {
    invoke_with("accessibility_action", &AccessibilityActionArgs { request }).await
}

pub async fn invoke<T>(command: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    if !is_available() {
        return Err("This capability is available in the Stalky desktop app".to_owned());
    }
    let value = invoke_without_args(command)
        .await
        .map_err(render_js_error)?;
    serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())
}

pub async fn invoke_with<T, A>(command: &str, args: &A) -> Result<T, String>
where
    T: DeserializeOwned,
    A: Serialize,
{
    if !is_available() {
        return Err("This capability is available in the Stalky desktop app".to_owned());
    }
    let args = serde_wasm_bindgen::to_value(args).map_err(|error| error.to_string())?;
    let value = invoke_with_args(command, args)
        .await
        .map_err(render_js_error)?;
    serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())
}

fn render_js_error(error: JsValue) -> String {
    if let Some(message) = error.as_string() {
        return message;
    }

    let code = js_sys::Reflect::get(&error, &JsValue::from_str("code"))
        .ok()
        .and_then(|value| value.as_string());
    match code.as_deref() {
        Some("permission_not_granted") => {
            "Screen Recording permission is required. Enable Stalky in System Settings → Privacy & Security → Screen & System Audio Recording, then try again."
                .to_owned()
        }
        Some("permission_preflight") => {
            "Stalky could not read the current Screen Recording permission state.".to_owned()
        }
        Some("no_displays") | Some("display_not_found") => {
            "The selected display is no longer available.".to_owned()
        }
        Some("already_active") => "Screen capture is already active.".to_owned(),
        Some("stream_start") | Some("output_handler_registration") => {
            "ScreenCaptureKit could not start the display stream.".to_owned()
        }
        Some("stream_stop") | Some("stream_stopped") => {
            "The display stream stopped unexpectedly.".to_owned()
        }
        Some("not_trusted") => {
            "Accessibility access is required. Grant Stalky access in System Settings → Privacy & Security → Accessibility, then retest."
                .to_owned()
        }
        Some("already_running") => "Accessibility observation is already active.".to_owned(),
        Some("not_running") => "Start Accessibility observation before using controls.".to_owned(),
        Some("action_rejected") => {
            "That control is no longer available for the selected interface element."
                .to_owned()
        }
        Some("timeout") => "The selected application did not respond in time.".to_owned(),
        Some("worker_stopped") | Some("worker_start") => {
            "The Accessibility observer stopped unexpectedly.".to_owned()
        }
        _ => "The native Stalky capture service returned an unknown error.".to_owned(),
    }
}
