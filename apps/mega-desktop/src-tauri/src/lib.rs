use mega_capture::{CaptureError, CaptureService, CaptureSource, CaptureStatus};
use mega_core::PermissionState;
use mega_platform_macos::MacOsPlatform;
use serde::Serialize;
use stalky_accessibility::{
    AccessibilityActionRequest, AccessibilityError, AccessibilityService, AccessibilityStatus,
};

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionStatuses {
    accessibility: PermissionState,
    screen_recording: PermissionState,
    microphone: PermissionState,
}

/// Reads current OS permission state. The adapter never opens a prompt.
#[tauri::command]
fn permission_statuses() -> Result<PermissionStatuses, String> {
    let statuses = MacOsPlatform::new()
        .permission_statuses()
        .map_err(|error| error.to_string())?;

    Ok(PermissionStatuses {
        accessibility: statuses.accessibility,
        screen_recording: statuses.screen_recording,
        microphone: statuses.microphone,
    })
}

#[tauri::command]
async fn capture_start(
    service: tauri::State<'_, CaptureService>,
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
async fn capture_stop(
    service: tauri::State<'_, CaptureService>,
) -> Result<CaptureStatus, CaptureError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.stop())
        .await
        .map_err(|error| CaptureError::StreamStop {
            capture_source: "active capture".to_owned(),
            message: format!("capture stop task failed: {error}"),
        })?
}

#[tauri::command]
fn capture_status(
    service: tauri::State<'_, CaptureService>,
) -> Result<CaptureStatus, CaptureError> {
    service.status()
}

#[tauri::command]
async fn accessibility_start(
    service: tauri::State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.start())
        .await
        .map_err(|_| AccessibilityError::WorkerStart)?
}

#[tauri::command]
async fn accessibility_stop(
    service: tauri::State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.stop())
        .await
        .map_err(|_| AccessibilityError::WorkerStopped)?
}

#[tauri::command]
fn accessibility_status(
    service: tauri::State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    service.status()
}

/// Explicit user-click permission request. No other command calls this API.
#[tauri::command]
async fn accessibility_request(
    service: tauri::State<'_, AccessibilityService>,
) -> Result<mega_core::PermissionState, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.request_permission())
        .await
        .map_err(|_| AccessibilityError::WorkerStart)?
}

#[tauri::command]
async fn accessibility_action(
    service: tauri::State<'_, AccessibilityService>,
    request: AccessibilityActionRequest,
) -> Result<stalky_accessibility::AccessibilityActionResult, AccessibilityError> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.execute(request))
        .await
        .map_err(|_| AccessibilityError::WorkerStopped)?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(CaptureService::new())
        .manage(AccessibilityService::new())
        .invoke_handler(tauri::generate_handler![
            shell_identity,
            permission_statuses,
            capture_start,
            capture_stop,
            capture_status,
            accessibility_start,
            accessibility_stop,
            accessibility_status,
            accessibility_request,
            accessibility_action
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
