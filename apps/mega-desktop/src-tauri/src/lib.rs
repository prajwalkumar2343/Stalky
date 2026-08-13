mod audio;
mod auth;
mod cloud;
mod history;
mod hud;
mod memory;
mod permissions;
mod preferences;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use mega_capture::{CaptureError, CaptureService, CaptureSource, CaptureStatus};
use mega_core::{PermissionCapability, PermissionState};
use mega_platform_macos::{MacOsPlatform, PlatformErrorKind};
use permissions::{PermissionCoordinator, PermissionRequestError};
use preferences::{AccountMode, OnboardingState, PreferenceStore};
use serde::{Deserialize, Serialize};
use stalky_accessibility::{
    AccessibilityActionRequest, AccessibilityError, AccessibilityService, AccessibilityStatus,
};
use tauri::{Manager, State, WindowEvent};

use hud::HudWindowState;

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
///
/// `live` switches the Accessibility probe to the active event-tap check,
/// which sees grants made in System Settings while the app is running
/// (`AXIsProcessTrusted` caches its answer in-process). The live probe can
/// surface the system prompt and enroll the app in the Accessibility pane, so
/// it must only be used while the user is actively granting that permission.
fn refresh_permission_snapshot(
    coordinator: &PermissionCoordinator,
    preferences: &PreferenceStore,
    capture: &CaptureService,
    accessibility: &AccessibilityService,
    live: bool,
) -> PermissionStatuses {
    let platform = MacOsPlatform::new();
    for capability in [
        PermissionCapability::Accessibility,
        PermissionCapability::ScreenRecording,
        PermissionCapability::Microphone,
    ] {
        let state = platform
            .permission_status(capability)
            .unwrap_or_else(|error| match error.kind {
                PlatformErrorKind::Unsupported => PermissionState::Unsupported,
                PlatformErrorKind::RequestTimeout => PermissionState::Unknown,
            });
        coordinator.observe(capability, state);
    }
    if live && let Ok(live_state) = platform.accessibility_permission_status_live() {
        coordinator.observe(PermissionCapability::Accessibility, live_state);
    }
    coordinator.observe(
        PermissionCapability::LaunchAtLogin,
        PermissionState::Unsupported,
    );
    let snapshot = coordinator.snapshot();
    stop_services_without_permission(&snapshot, capture, accessibility);
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

/// A protected service must not outlive the OS grant it depends on. Refreshes
/// run on focus and on the UI poller, including when the capability page is not
/// mounted, so revocation always tears down the corresponding session.
fn stop_services_without_permission(
    snapshot: &BTreeMap<PermissionCapability, PermissionState>,
    capture: &CaptureService,
    accessibility: &AccessibilityService,
) {
    if snapshot
        .get(&PermissionCapability::ScreenRecording)
        .is_some_and(|state| !state.is_granted())
    {
        let _ = capture.stop();
    }
    if snapshot
        .get(&PermissionCapability::Accessibility)
        .is_some_and(|state| !state.is_granted())
    {
        let _ = accessibility.stop();
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
    capture: State<'_, CaptureService>,
    accessibility: State<'_, AccessibilityService>,
) -> PermissionStatuses {
    // Once the user has explicitly requested Accessibility, the active probe
    // is allowed and lets a System Settings grant appear without relaunching.
    let live_accessibility = preferences.has_requested(PermissionCapability::Accessibility);
    refresh_permission_snapshot(
        coordinator.inner(),
        preferences.inner(),
        capture.inner(),
        accessibility.inner(),
        live_accessibility,
    )
}

/// Live variant used by onboarding after the user starts granting. The
/// Accessibility probe switches to the active event-tap check so a grant made
/// in System Settings is seen without relaunching Stalky.
#[tauri::command]
fn permission_statuses_live(
    coordinator: State<'_, PermissionCoordinator>,
    preferences: State<'_, PreferenceStore>,
    capture: State<'_, CaptureService>,
    accessibility: State<'_, AccessibilityService>,
) -> PermissionStatuses {
    refresh_permission_snapshot(
        coordinator.inner(),
        preferences.inner(),
        capture.inner(),
        accessibility.inner(),
        true,
    )
}

#[tauri::command]
fn permission_open_settings(capability: PermissionSettingsTarget) -> Result<(), String> {
    open_settings_pane(capability)
}

fn open_settings_pane(target: PermissionSettingsTarget) -> Result<(), String> {
    let target = match target {
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
            return Err("Launch at login is not supported in this build.".to_owned());
        }
    };
    let status = Command::new("open")
        .arg(target)
        .status()
        .map_err(|error| format!("could not open System Settings: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not open System Settings (exit status: {status})"
        ))
    }
}

#[tauri::command]
async fn permission_request(
    app: tauri::AppHandle,
    coordinator: State<'_, PermissionCoordinator>,
    preferences: State<'_, PreferenceStore>,
    capture: State<'_, CaptureService>,
    accessibility: State<'_, AccessibilityService>,
    capability: PermissionSettingsTarget,
) -> Result<PermissionStatuses, String> {
    let capability = capability.into();
    coordinator
        .begin_request(capability)
        .map_err(permission_request_message)?;
    if let Err(error) = preferences.record_permission_request(capability) {
        coordinator.mark_request_failed(capability);
        return Err(error);
    }

    let coordinator = coordinator.inner().clone();
    let accessibility = accessibility.inner().clone();
    let accessibility_for_request = accessibility.clone();
    let result = if capability == PermissionCapability::ScreenRecording {
        // Open System Settings first so the pane sits behind the native
        // request: on macOS 15+ the consent sheet layers over Settings, and
        // if the user dismisses it, Settings is already open and they are
        // not stuck. The request itself is what enrolls Stalky in the
        // Screen Recording list.
        let _ = open_settings_pane(PermissionSettingsTarget::ScreenRecording);
        enroll_screen_recording_on_main(app).await
    } else {
        match tauri::async_runtime::spawn_blocking(move || match capability {
            PermissionCapability::Accessibility => {
                // Same pattern as Screen Recording: the native prompt only
                // appears once per app identity, so the Accessibility pane
                // is opened first. If the prompt is dismissed (or never
                // appears again), the user is already in System Settings
                // and can toggle Stalky's row directly.
                let _ = open_settings_pane(PermissionSettingsTarget::Accessibility);
                accessibility_for_request
                    .request_permission()
                    .map_err(|error| error.to_string())
            }
            PermissionCapability::Microphone => {
                request_microphone_permission().map_err(|error| error.to_string())
            }
            PermissionCapability::ScreenRecording => unreachable!(),
            PermissionCapability::LaunchAtLogin => {
                Err("Launch at login is optional and not available in this build.".to_owned())
            }
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                coordinator.mark_request_failed(capability);
                return Err(format!("permission request task failed: {error}"));
            }
        }
    };

    match result {
        Ok(state) => coordinator.finish_request(capability, state),
        Err(_) => coordinator.mark_request_failed(capability),
    }
    result.map_err(|error| format_permission_error(capability, error))?;

    Ok(refresh_permission_snapshot(
        &coordinator,
        preferences.inner(),
        capture.inner(),
        &accessibility,
        false,
    ))
}

/// Runs Stalky's native Screen Recording enrollment from the application main
/// thread. `CGRequestScreenCaptureAccess` creates Stalky's row in System
/// Settings → Privacy & Security → Screen Recording and shows the system
/// prompt; calling it off the main thread can leave the row unregistered.
async fn enroll_screen_recording_on_main(app: tauri::AppHandle) -> Result<PermissionState, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let result = MacOsPlatform::new()
            .request_permission(PermissionCapability::ScreenRecording)
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    })
    .map_err(|error| format!("could not schedule the Screen Recording request: {error}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver
            .recv_timeout(std::time::Duration::from_secs(120))
            .map_err(|_| "macOS did not finish the Screen Recording request.".to_owned())?
    })
    .await
    .map_err(|error| format!("Screen Recording request task failed: {error}"))?
}

fn permission_request_message(error: PermissionRequestError) -> String {
    match error {
        PermissionRequestError::AlreadyGranted => "This permission is already granted.".to_owned(),
        PermissionRequestError::AlreadyRequesting => {
            "Stalky is waiting for the current permission request to finish.".to_owned()
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

/// Requests Microphone access only when macOS still allows a prompt.
///
/// A first-time request shows the native consent sheet. After a denial macOS
/// never re-prompts, so the request routes the user to the Microphone pane in
/// System Settings instead of invoking a no-op prompt.
fn request_microphone_permission() -> Result<PermissionState, mega_platform_macos::PlatformError> {
    let platform = MacOsPlatform::new();
    let current = platform.permission_status(PermissionCapability::Microphone)?;
    if current.is_granted() {
        return Ok(current);
    }
    if matches!(
        current,
        PermissionState::Denied | PermissionState::Restricted | PermissionState::Revoked
    ) {
        let _ = open_settings_pane(PermissionSettingsTarget::Microphone);
        return Ok(current);
    }
    platform.request_permission(PermissionCapability::Microphone)
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

/// Relaunches Stalky. Screen Recording grants made in the TCC sheet can take
/// effect only after the process restarts; this is the explicit, user-confirmed
/// action that performs it.
#[tauri::command]
fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}

/// Reveals the workspace only after the bundled WASM has mounted. This keeps
/// WKWebView's unpainted startup surface from appearing as a blank app window.
#[tauri::command]
fn main_window_ready(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "The Stalky workspace is unavailable.".to_owned())?;
    main.show()
        .map_err(|error| format!("could not show Stalky: {error}"))?;
    let _ = main.unminimize();
    main.set_focus()
        .map_err(|error| format!("could not focus Stalky: {error}"))
}

#[tauri::command]
fn onboarding_complete(
    preferences: State<'_, PreferenceStore>,
    account_mode: AccountMode,
) -> Result<OnboardingState, String> {
    validate_account_mode(account_mode)?;
    preferences.complete_onboarding(account_mode)?;
    Ok(preferences.onboarding_state())
}

#[tauri::command]
fn onboarding_set_account_mode(
    preferences: State<'_, PreferenceStore>,
    account_mode: AccountMode,
) -> Result<OnboardingState, String> {
    validate_account_mode(account_mode)?;
    preferences.set_account_mode(account_mode)?;
    Ok(preferences.onboarding_state())
}

fn validate_account_mode(account_mode: AccountMode) -> Result<(), String> {
    if account_mode == AccountMode::Google && !auth::signed_in() {
        return Err("Complete Google sign-in before selecting the Google account mode.".to_owned());
    }
    Ok(())
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
        return Err("Google sign-in is not configured for this build. Set STALKY_SUPABASE_URL and STALKY_SUPABASE_PUBLISHABLE_KEY, then restart Stalky.".to_owned());
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

#[tauri::command]
async fn cloud_profile() -> Result<cloud::CloudProfile, String> {
    tauri::async_runtime::spawn_blocking(cloud::profile)
        .await
        .map_err(|error| format!("Stalky Cloud task failed: {error}"))?
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
            let preferences = PreferenceStore::new(path);
            let hud_state = HudWindowState::new(preferences.hud_anchor());
            if let Err(error) = hud::prepare_window(app.handle(), &hud_state) {
                eprintln!("Stalky glance is unavailable: {error}");
            }
            app.manage(hud_state);
            app.manage(preferences);
            app.manage(PermissionCoordinator::new());
            let memory = memory::MemoryService::initialize(app.handle());
            let audio = audio::AudioVaultService::initialize(app.handle());
            let audio_history =
                audio::AudioHistoryService::initialize(audio.clone(), memory.clone())
                    .map_err(std::io::Error::other)?;
            let history = history::HistoryService::start(
                memory.clone(),
                audio.clone(),
                app.state::<CaptureService>().inner().clone(),
                app.state::<AccessibilityService>().inner().clone(),
            )
            .map_err(std::io::Error::other)?;
            app.manage(memory);
            app.manage(audio);
            app.manage(audio_history);
            app.manage(history);
            Ok(())
        })
        .manage(CaptureService::new())
        .manage(AccessibilityService::new())
        .on_window_event(|window, event| {
            if window.label() == "hud" {
                let state = window.state::<HudWindowState>();
                if matches!(event, WindowEvent::Moved(_)) {
                    hud::observe_window_position(window, state.inner());
                }
                if matches!(event, WindowEvent::Focused(false) | WindowEvent::Destroyed) {
                    let preferences = window.state::<PreferenceStore>();
                    hud::persist_window_position(preferences.inner(), state.inner());
                }
            }
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
                && window.get_webview_window("hud").is_some()
            {
                api.prevent_close();
                let _ = window.hide();
                return;
            }
            if matches!(event, WindowEvent::Focused(true)) {
                let coordinator = window.state::<PermissionCoordinator>();
                let preferences = window.state::<PreferenceStore>();
                let capture = window.state::<CaptureService>();
                let accessibility = window.state::<AccessibilityService>();
                let _ = refresh_permission_snapshot(
                    coordinator.inner(),
                    preferences.inner(),
                    capture.inner(),
                    accessibility.inner(),
                    preferences.has_requested(PermissionCapability::Accessibility),
                );
            }
        })
        .invoke_handler(tauri::generate_handler![
            shell_identity,
            permission_statuses,
            permission_statuses_live,
            permission_request,
            permission_open_settings,
            relaunch_app,
            main_window_ready,
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
            google_auth_sign_out,
            cloud_profile,
            memory::memory_create_manual,
            memory::memory_search,
            memory::memory_edit,
            memory::memory_confirm,
            memory::memory_reject,
            memory::memory_context,
            memory::memory_delete,
            history::history_status,
            history::history_search,
            history::history_delete,
            audio::audio_vault_status,
            audio::audio_history_start,
            audio::audio_history_stop,
            audio::audio_history_status,
            hud::hud_set_presentation,
            hud::hud_open_main
        ])
        .run(tauri::generate_context!())
        .expect("Stalky desktop runtime failed");
}

#[cfg(test)]
mod tests {
    use mega_core::PermissionState;

    use super::{AccountMode, display_state, shell_identity, validate_account_mode};

    #[test]
    fn shell_identity_exposes_infrastructure_milestone() {
        let identity = shell_identity();
        assert_eq!(identity.name, "Stalky");
        assert_eq!(identity.milestone, "infrastructure");
    }

    #[test]
    fn first_observation_of_denied_permission_is_displayed_as_not_requested() {
        assert_eq!(
            display_state(Some(PermissionState::Denied), false),
            PermissionState::NotRequested
        );
        assert_eq!(
            display_state(Some(PermissionState::Denied), true),
            PermissionState::Denied
        );
    }

    #[test]
    fn google_account_mode_cannot_be_persisted_without_a_session() {
        assert!(validate_account_mode(AccountMode::Local).is_ok());
        assert!(validate_account_mode(AccountMode::Google).is_err());
    }
}
