mod auth;
mod permissions;
mod preferences;

use std::path::PathBuf;
use std::process::Command;

use mega_capture::{CaptureError, CaptureService, CaptureSource, CaptureStatus};
use mega_core::{PermissionCapability, PermissionState};
use mega_platform_macos::MacOsPlatform;
use permissions::{PermissionCoordinator, PermissionRequestError};
use preferences::{AccountMode, OnboardingState, PreferenceStore};
use serde::{Deserialize, Serialize};
use stalky_accessibility::{
    AccessibilityActionRequest, AccessibilityError, AccessibilityService, AccessibilityStatus,
};
use tauri::{Manager, State, WindowEvent};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellIdentity {
    name: &'static str,
    version: &'static str,
    milestone: &'static str,
}

#[tauri::command]
fn shell_identity() -> ShellIdentity {
    ShellIdentity {
        name: "Stalky",
        version: env!("CARGO_PKG_VERSION"),
        milestone: "infrastructure",
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct PermissionStatuses {
    accessibility: PermissionState,
    screen_recording: PermissionState,
    microphone: PermissionState,
    launch_at_login: PermissionState,
    launch_at_login_supported: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PermissionSettingsTarget {
    Accessibility,
    ScreenRecording,
    Microphone,
    LaunchAtLogin,
}

impl From<PermissionSettingsTarget> for PermissionCapability {
    fn from(value: PermissionSettingsTarget) -> Self {
        match value {
            PermissionSettingsTarget::Accessibility => Self::Accessibility,
            PermissionSettingsTarget::ScreenRecording => Self::ScreenRecording,
            PermissionSettingsTarget::Microphone => Self::Microphone,
            PermissionSettingsTarget::LaunchAtLogin => Self::LaunchAtLogin,
        }
    }
}

/// Reads fresh OS truth and updates the in-memory coordinator. No native
/// request method is called here, including when the window regains focus.
fn refresh_permission_snapshot(
    coordinator: &PermissionCoordinator,
    preferences: &PreferenceStore,
) -> PermissionStatuses {
    let platform = MacOsPlatform::new();
    for capability in [
        PermissionCapability::Accessibility,
        PermissionCapability::ScreenRecording,
        PermissionCapability::Microphone,
    ] {
        let state = platform
            .permission_status(capability)
            .unwrap_or(PermissionState::Unsupported);
        coordinator.observe(capability, state);
    }
    coordinator.observe(
        PermissionCapability::LaunchAtLogin,
        PermissionState::Unsupported,
    );
    let snapshot = coordinator.snapshot();
    PermissionStatuses {
        accessibility: display_state(
            snapshot.get(&PermissionCapability::Accessibility).copied(),
            preferences.has_requested(PermissionCapability::Accessibility),
        ),
        screen_recording: display_state(
            snapshot
                .get(&PermissionCapability::ScreenRecording)
                .copied(),
            preferences.has_requested(PermissionCapability::ScreenRecording),
        ),
        microphone: display_state(
            snapshot.get(&PermissionCapability::Microphone).copied(),
            preferences.has_requested(PermissionCapability::Microphone),
        ),
        launch_at_login: PermissionState::Unsupported,
        launch_at_login_supported: false,
    }
}

fn display_state(state: Option<PermissionState>, has_requested: bool) -> PermissionState {
    match state.unwrap_or(PermissionState::Unknown) {
        PermissionState::Denied if !has_requested => PermissionState::NotRequested,
        state => state,
    }
}

#[tauri::command]
fn permission_statuses(
    coordinator: State<'_, PermissionCoordinator>,
    preferences: State<'_, PreferenceStore>,
) -> PermissionStatuses {
    refresh_permission_snapshot(coordinator.inner(), preferences.inner())
}

#[tauri::command]
fn permission_open_settings(capability: PermissionSettingsTarget) -> Result<(), String> {
    let target = match capability {
        PermissionSettingsTarget::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionSettingsTarget::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PermissionSettingsTarget::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PermissionSettingsTarget::LaunchAtLogin => {
            "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
        }
    };
    Command::new("open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open System Settings: {error}"))
}

#[tauri::command]
async fn permission_request(
    coordinator: State<'_, PermissionCoordinator>,
    preferences: State<'_, PreferenceStore>,
    accessibility: State<'_, AccessibilityService>,
    capability: PermissionSettingsTarget,
) -> Result<PermissionStatuses, String> {
    let capability = capability.into();
    let has_requested = preferences.has_requested(capability);
    coordinator
        .begin_request(capability, has_requested)
        .map_err(permission_request_message)?;
    if let Err(error) = preferences.record_permission_request(capability) {
        coordinator.mark_request_failed(capability);
        return Err(error);
    }

    let coordinator = coordinator.inner().clone();
    let accessibility = accessibility.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || match capability {
        PermissionCapability::Accessibility => accessibility
            .request_permission()
            .map_err(|error| error.to_string()),
        PermissionCapability::ScreenRecording | PermissionCapability::Microphone => {
            MacOsPlatform::new()
                .request_permission(capability)
                .map_err(|error| error.to_string())
        }
        PermissionCapability::LaunchAtLogin => {
            Err("Launch at login is optional and not available in this build.".to_owned())
        }
    })
    .await
    .map_err(|error| format!("permission request task failed: {error}"))?;

    match result {
        Ok(state) => coordinator.finish_request(capability, state),
        Err(_) => coordinator.mark_request_failed(capability),
    }
    result.map_err(|error| format_permission_error(capability, error))?;

    Ok(refresh_permission_snapshot(
        &coordinator,
        preferences.inner(),
    ))
}

fn permission_request_message(error: PermissionRequestError) -> String {
    match error {
        PermissionRequestError::AlreadyGranted => "This permission is already granted.".to_owned(),
        PermissionRequestError::AlreadyRequesting => {
            "Stalky is waiting for the current permission request to finish.".to_owned()
        }
        PermissionRequestError::OpenSettings => {
            "This permission was denied earlier. Open System Settings to change it.".to_owned()
        }
        PermissionRequestError::Unsupported => {
            "This capability is unavailable in this build.".to_owned()
        }
        PermissionRequestError::Transition(error) => error,
    }
}

fn format_permission_error(capability: PermissionCapability, error: String) -> String {
    match capability {
        PermissionCapability::Accessibility => {
            format!("Accessibility request failed: {error}")
        }
        PermissionCapability::ScreenRecording => {
            format!("Screen Recording request failed: {error}")
        }
        PermissionCapability::Microphone => format!("Microphone request failed: {error}"),
        PermissionCapability::LaunchAtLogin => error,
    }
}

#[tauri::command]
async fn accessibility_status(
    service: State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.refresh_permission())
        .await
        .map_err(|_| AccessibilityError::WorkerStopped)?
}

#[tauri::command]
async fn accessibility_start(
    service: State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.start())
        .await
        .map_err(|_| AccessibilityError::WorkerStart)?
}

#[tauri::command]
async fn accessibility_stop(
    service: State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.stop())
        .await
        .map_err(|_| AccessibilityError::WorkerStopped)?
}

#[tauri::command]
async fn accessibility_action(
    service: State<'_, AccessibilityService>,
    request: AccessibilityActionRequest,
) -> Result<stalky_accessibility::AccessibilityActionResult, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.execute(request))
        .await
        .map_err(|_| AccessibilityError::WorkerStopped)?
}

#[tauri::command]
async fn capture_start(
    service: State<'_, CaptureService>,
    source: Option<CaptureSource>,
) -> Result<CaptureStatus, CaptureError> {
    let service = service.inner().clone();
    let source = source.unwrap_or(CaptureSource::PrimaryDisplay);
    let source_label = source.to_string();
    tauri::async_runtime::spawn_blocking(move || service.start(source))
        .await
        .map_err(|error| CaptureError::StreamStart {
            capture_source: source_label,
            message: format!("capture start task failed: {error}"),
        })?
}

#[tauri::command]
async fn capture_stop(service: State<'_, CaptureService>) -> Result<CaptureStatus, CaptureError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.stop())
        .await
        .map_err(|error| CaptureError::StreamStop {
            capture_source: "active capture".to_owned(),
            message: format!("capture stop task failed: {error}"),
        })?
}

#[tauri::command]
fn capture_status(service: State<'_, CaptureService>) -> Result<CaptureStatus, CaptureError> {
    service.status()
}

#[tauri::command]
fn onboarding_state(preferences: State<'_, PreferenceStore>) -> OnboardingState {
    preferences.onboarding_state()
}

#[tauri::command]
fn onboarding_complete(
    preferences: State<'_, PreferenceStore>,
    account_mode: AccountMode,
) -> Result<OnboardingState, String> {
    preferences.complete_onboarding(account_mode)?;
    Ok(preferences.onboarding_state())
}

#[tauri::command]
fn onboarding_set_account_mode(
    preferences: State<'_, PreferenceStore>,
    account_mode: AccountMode,
) -> Result<OnboardingState, String> {
    preferences.set_account_mode(account_mode)?;
    Ok(preferences.onboarding_state())
}

#[tauri::command]
fn onboarding_reset(preferences: State<'_, PreferenceStore>) -> Result<OnboardingState, String> {
    auth::sign_out()?;
    preferences.reset_onboarding()?;
    Ok(preferences.onboarding_state())
}

#[tauri::command]
fn google_auth_status() -> auth::GoogleAuthStatus {
    auth::status()
}

#[tauri::command]
async fn google_auth_start(
    preferences: State<'_, PreferenceStore>,
) -> Result<auth::GoogleAuthStatus, String> {
    if !auth::status().configured {
        return Err("Google sign-in is not configured for this build. Set STALKY_GOOGLE_CLIENT_ID and restart Stalky.".to_owned());
    }
    tauri::async_runtime::spawn_blocking(auth::run)
        .await
        .map_err(|error| format!("Google sign-in task failed: {error}"))??;
    preferences.set_account_mode(AccountMode::Google)?;
    Ok(auth::status())
}

#[tauri::command]
fn google_auth_sign_out(
    preferences: State<'_, PreferenceStore>,
) -> Result<auth::GoogleAuthStatus, String> {
    auth::sign_out()?;
    preferences.set_account_mode(AccountMode::Local)?;
    Ok(auth::status())
}

fn preferences_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("preferences.json"))
        .map_err(|error| format!("could not resolve Stalky local data directory: {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let path = preferences_path(app.handle()).map_err(std::io::Error::other)?;
            app.manage(PreferenceStore::new(path));
            app.manage(PermissionCoordinator::new());
            Ok(())
        })
        .manage(CaptureService::new())
        .manage(AccessibilityService::new())
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Focused(true)) {
                let coordinator = window.state::<PermissionCoordinator>();
                let preferences = window.state::<PreferenceStore>();
                let _ = refresh_permission_snapshot(coordinator.inner(), preferences.inner());
            }
        })
        .invoke_handler(tauri::generate_handler![
            shell_identity,
            permission_statuses,
            permission_request,
            permission_open_settings,
            capture_start,
            capture_stop,
            capture_status,
            accessibility_start,
            accessibility_stop,
            accessibility_status,
            accessibility_action,
            onboarding_state,
            onboarding_complete,
            onboarding_set_account_mode,
            onboarding_reset,
            google_auth_status,
            google_auth_start,
            google_auth_sign_out
        ])
        .run(tauri::generate_context!())
        .expect("Stalky desktop runtime failed");
}

#[cfg(test)]
mod tests {
    use super::shell_identity;

    #[test]
    fn shell_identity_exposes_infrastructure_milestone() {
        let identity = shell_identity();
        assert_eq!(identity.name, "Stalky");
        assert_eq!(identity.milestone, "infrastructure");
    }
}
