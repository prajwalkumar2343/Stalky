mod permissions;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mega_capture::{CaptureError, CaptureService, CaptureSource, CaptureStatus};
use mega_ipc::{
    PERMISSIONS_CHANGED_EVENT, PermissionCapability, PermissionError, PermissionSnapshot,
    PermissionState,
};
use permissions::PermissionCoordinator;
use serde::Serialize;
use stalky_accessibility::{
    AccessibilityActionRequest, AccessibilityError, AccessibilityService, AccessibilityStatus,
};
use tauri::{Emitter, Manager};

#[derive(Clone, Debug, Default)]
struct PermissionFocusGate {
    scheduled: Arc<AtomicBool>,
}

impl PermissionFocusGate {
    fn try_schedule(&self) -> bool {
        self.scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    fn finish(&self) {
        self.scheduled.store(false, Ordering::Release);
    }
}

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

#[tauri::command]
fn permission_snapshot(
    coordinator: tauri::State<'_, PermissionCoordinator>,
) -> Result<PermissionSnapshot, PermissionError> {
    coordinator.inner().snapshot()
}

#[tauri::command]
async fn permission_recheck(
    app: tauri::AppHandle,
    coordinator: tauri::State<'_, PermissionCoordinator>,
    capture: tauri::State<'_, CaptureService>,
    accessibility: tauri::State<'_, AccessibilityService>,
) -> Result<PermissionSnapshot, PermissionError> {
    let coordinator = coordinator.inner().clone();
    let capture = capture.inner().clone();
    let accessibility = accessibility.inner().clone();
    let event_app = app.clone();
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        coordinator.recheck_with_notify(|snapshot| {
            publish_permission_snapshot(&event_app, &snapshot);
        })
    })
    .await
    .map_err(|error| PermissionError::ProbeFailed {
        capability: PermissionCapability::ScreenRecording,
        message: format!("permission recheck task failed: {error}"),
    })??;
    gate_runtime(&snapshot, &capture, &accessibility);
    publish_permission_snapshot(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
async fn permission_request(
    app: tauri::AppHandle,
    coordinator: tauri::State<'_, PermissionCoordinator>,
    capture: tauri::State<'_, CaptureService>,
    accessibility: tauri::State<'_, AccessibilityService>,
    capability: PermissionCapability,
) -> Result<PermissionSnapshot, PermissionError> {
    let coordinator = coordinator.inner().clone();
    let capture = capture.inner().clone();
    let accessibility = accessibility.inner().clone();
    let event_app = app.clone();
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        coordinator.request(capability, |snapshot| {
            publish_permission_snapshot(&event_app, &snapshot);
        })
    })
    .await
    .map_err(|error| PermissionError::RequestFailed {
        capability,
        message: format!("permission request task failed: {error}"),
    })??;
    gate_runtime(&snapshot, &capture, &accessibility);
    publish_permission_snapshot(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
async fn permission_open_settings(
    app: tauri::AppHandle,
    coordinator: tauri::State<'_, PermissionCoordinator>,
    capability: PermissionCapability,
) -> Result<PermissionSnapshot, PermissionError> {
    let coordinator = coordinator.inner().clone();
    let snapshot =
        tauri::async_runtime::spawn_blocking(move || coordinator.open_settings(capability))
            .await
            .map_err(|error| PermissionError::SettingsFailed {
                capability,
                message: format!("System Settings task failed: {error}"),
            })??;
    publish_permission_snapshot(&app, &snapshot);
    Ok(snapshot)
}

fn publish_permission_snapshot(app: &tauri::AppHandle, snapshot: &PermissionSnapshot) {
    let _ = app.emit(PERMISSIONS_CHANGED_EVENT, snapshot);
}

fn gate_runtime(
    snapshot: &PermissionSnapshot,
    capture: &CaptureService,
    accessibility: &AccessibilityService,
) {
    let is_lost = |capability| {
        snapshot
            .statuses
            .iter()
            .find(|status| status.capability == capability)
            .is_some_and(|status| status.authorization != PermissionState::Granted)
    };

    if is_lost(PermissionCapability::ScreenRecording) {
        let _ = capture.stop();
    }
    if is_lost(PermissionCapability::Accessibility) {
        let _ = accessibility.stop();
    }
}

#[tauri::command]
async fn capture_start(
    coordinator: tauri::State<'_, PermissionCoordinator>,
    service: tauri::State<'_, CaptureService>,
    source: Option<CaptureSource>,
) -> Result<CaptureStatus, CaptureError> {
    let source = source.unwrap_or(CaptureSource::PrimaryDisplay);
    let source_label = source.to_string();
    let observed = coordinator
        .inner()
        .snapshot()
        .ok()
        .and_then(|snapshot| {
            snapshot
                .statuses
                .into_iter()
                .find(|status| status.capability == PermissionCapability::ScreenRecording)
                .map(|status| status.authorization)
        })
        .unwrap_or(PermissionState::Unknown);
    if observed != PermissionState::Granted {
        return Err(CaptureError::PermissionNotGranted { observed });
    }
    let service = service.inner().clone();
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
    coordinator: tauri::State<'_, PermissionCoordinator>,
    service: tauri::State<'_, AccessibilityService>,
) -> Result<AccessibilityStatus, AccessibilityError> {
    let granted = coordinator.inner().snapshot().ok().is_some_and(|snapshot| {
        snapshot
            .statuses
            .into_iter()
            .find(|status| status.capability == PermissionCapability::Accessibility)
            .is_some_and(|status| status.authorization == PermissionState::Granted)
    });
    if !granted {
        return Err(AccessibilityError::NotTrusted);
    }
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
        .manage(PermissionCoordinator::new())
        .manage(PermissionFocusGate::default())
        .on_window_event(|window, event| {
            if !matches!(event, tauri::WindowEvent::Focused(true)) {
                return;
            }

            let app = window.app_handle().clone();
            let focus_gate = app.state::<PermissionFocusGate>().inner().clone();
            if !focus_gate.try_schedule() {
                return;
            }
            let coordinator = app.state::<PermissionCoordinator>().inner().clone();
            let capture = app.state::<CaptureService>().inner().clone();
            let accessibility = app.state::<AccessibilityService>().inner().clone();
            let event_app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = tauri::async_runtime::spawn_blocking(|| {
                    std::thread::sleep(Duration::from_millis(350));
                })
                .await;
                if let Ok(Ok(snapshot)) = tauri::async_runtime::spawn_blocking(move || {
                    coordinator.recheck_with_notify(|snapshot| {
                        publish_permission_snapshot(&event_app, &snapshot);
                    })
                })
                .await
                {
                    gate_runtime(&snapshot, &capture, &accessibility);
                    publish_permission_snapshot(&app, &snapshot);
                }
                focus_gate.finish();
            });
        })
        .invoke_handler(tauri::generate_handler![
            shell_identity,
            permission_snapshot,
            permission_recheck,
            permission_request,
            permission_open_settings,
            capture_start,
            capture_stop,
            capture_status,
            accessibility_start,
            accessibility_stop,
            accessibility_status,
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
