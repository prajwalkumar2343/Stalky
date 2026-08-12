use std::sync::Mutex;

use serde::Deserialize;
use tauri::{
    AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, State, WebviewWindow, Window,
    window::{Effect, EffectState, EffectsBuilder},
};

use crate::preferences::{HudAnchor, PreferenceStore};

const HUD_WINDOW_LABEL: &str = "hud";
const HUD_COMPACT_WIDTH: f64 = 320.0;
const HUD_COMPACT_HEIGHT: f64 = 68.0;
const HUD_EXPANDED_WIDTH: f64 = 320.0;
const HUD_EXPANDED_HEIGHT: f64 = 68.0;
const HUD_SCREEN_MARGIN: f64 = 18.0;
const HUD_DEFAULT_TOP_OFFSET: f64 = 58.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HudPresentation {
    Compact,
    Expanded,
}

impl HudPresentation {
    const fn logical_size(self) -> (f64, f64) {
        match self {
            Self::Compact => (HUD_COMPACT_WIDTH, HUD_COMPACT_HEIGHT),
            Self::Expanded => (HUD_EXPANDED_WIDTH, HUD_EXPANDED_HEIGHT),
        }
    }

    const fn corner_radius(self) -> f64 {
        match self {
            Self::Compact | Self::Expanded => 18.0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct HudWindowState {
    anchor: Mutex<Option<HudAnchor>>,
}

impl HudWindowState {
    pub(crate) fn new(anchor: Option<HudAnchor>) -> Self {
        Self {
            anchor: Mutex::new(anchor),
        }
    }

    fn anchor(&self) -> Option<HudAnchor> {
        *self
            .anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn remember(&self, anchor: HudAnchor) {
        *self
            .anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(anchor);
    }
}

pub(crate) fn prepare_window(app: &AppHandle, state: &HudWindowState) -> Result<(), String> {
    let window = app
        .get_webview_window(HUD_WINDOW_LABEL)
        .ok_or_else(|| "Stalky glance window was not created.".to_owned())?;
    apply_material(&window, HudPresentation::Compact)?;

    let monitor = monitor_for_anchor(&window, state.anchor())?;
    let scale = monitor.scale_factor();
    let target_size = physical_size(HudPresentation::Compact, scale);
    let anchor = state
        .anchor()
        .unwrap_or_else(|| default_anchor(&monitor, scale));
    let position = position_for_anchor(anchor, target_size, &monitor);

    window
        .set_position(position)
        .map_err(|error| format!("could not position Stalky glance: {error}"))?;
    state.remember(anchor_from_position(position, target_size));
    Ok(())
}

pub(crate) fn observe_window_position(window: &Window, state: &HudWindowState) {
    if window.label() != HUD_WINDOW_LABEL {
        return;
    }
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        state.remember(anchor_from_position(position, size));
    }
}

pub(crate) fn persist_window_position(preferences: &PreferenceStore, state: &HudWindowState) {
    if let Some(anchor) = state.anchor() {
        let _ = preferences.set_hud_anchor(anchor);
    }
}

#[tauri::command]
pub(crate) fn hud_set_presentation(
    window: WebviewWindow,
    state: State<'_, HudWindowState>,
    preferences: State<'_, PreferenceStore>,
    presentation: HudPresentation,
) -> Result<(), String> {
    require_hud_window(&window)?;
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        state.remember(anchor_from_position(position, size));
    }
    let anchor = state
        .anchor()
        .ok_or_else(|| "Stalky glance does not have a valid screen position yet.".to_owned())?;
    let monitor = monitor_for_anchor(&window, Some(anchor))?;
    let target_size = physical_size(presentation, monitor.scale_factor());
    let position = position_for_anchor(anchor, target_size, &monitor);

    apply_material(&window, presentation)?;
    window
        .set_position(position)
        .map_err(|error| format!("could not move Stalky glance: {error}"))?;
    window
        .set_size(tauri::LogicalSize::new(
            presentation.logical_size().0,
            presentation.logical_size().1,
        ))
        .map_err(|error| format!("could not resize Stalky glance: {error}"))?;

    state.remember(anchor_from_position(position, target_size));
    persist_window_position(preferences.inner(), state.inner());
    Ok(())
}

#[tauri::command]
pub(crate) fn hud_open_main(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_hud_window(&window)?;
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "The Stalky workspace is not available.".to_owned())?;
    main.show()
        .map_err(|error| format!("could not show Stalky: {error}"))?;
    let _ = main.unminimize();
    main.set_focus()
        .map_err(|error| format!("could not focus Stalky: {error}"))
}

fn require_hud_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == HUD_WINDOW_LABEL {
        Ok(())
    } else {
        Err("This window cannot control the Stalky glance surface.".to_owned())
    }
}

fn apply_material(window: &WebviewWindow, presentation: HudPresentation) -> Result<(), String> {
    window
        .set_effects(
            EffectsBuilder::new()
                .effect(Effect::HudWindow)
                .state(EffectState::Active)
                .radius(presentation.corner_radius())
                .build(),
        )
        .map_err(|error| format!("could not apply the Stalky glass material: {error}"))
}

fn monitor_for_anchor(
    window: &WebviewWindow,
    anchor: Option<HudAnchor>,
) -> Result<Monitor, String> {
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("could not read connected displays: {error}"))?;
    if let Some(anchor) = anchor
        && let Some(monitor) = monitors
            .iter()
            .find(|monitor| work_area_contains(monitor, anchor))
    {
        return Ok(monitor.clone());
    }
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| monitors.into_iter().next())
        .ok_or_else(|| "No display is available for Stalky glance.".to_owned())
}

fn work_area_contains(monitor: &Monitor, anchor: HudAnchor) -> bool {
    let area = monitor.work_area();
    let right = area.position.x.saturating_add_unsigned(area.size.width);
    let bottom = area.position.y.saturating_add_unsigned(area.size.height);
    anchor.right_edge > area.position.x
        && anchor.right_edge <= right
        && anchor.top >= area.position.y
        && anchor.top < bottom
}

fn physical_size(presentation: HudPresentation, scale: f64) -> PhysicalSize<u32> {
    let (width, height) = presentation.logical_size();
    PhysicalSize::new(
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    )
}

fn default_anchor(monitor: &Monitor, scale: f64) -> HudAnchor {
    let area = monitor.work_area();
    HudAnchor {
        right_edge: area
            .position
            .x
            .saturating_add_unsigned(area.size.width)
            .saturating_sub((HUD_SCREEN_MARGIN * scale).round() as i32),
        top: area
            .position
            .y
            .saturating_add((HUD_DEFAULT_TOP_OFFSET * scale).round() as i32),
    }
}

fn position_for_anchor(
    anchor: HudAnchor,
    target_size: PhysicalSize<u32>,
    monitor: &Monitor,
) -> PhysicalPosition<i32> {
    let area = monitor.work_area();
    let max_x = area
        .position
        .x
        .saturating_add_unsigned(area.size.width.saturating_sub(target_size.width));
    let max_y = area
        .position
        .y
        .saturating_add_unsigned(area.size.height.saturating_sub(target_size.height));
    PhysicalPosition::new(
        anchor
            .right_edge
            .saturating_sub_unsigned(target_size.width)
            .clamp(area.position.x, max_x),
        anchor.top.clamp(area.position.y, max_y),
    )
}

fn anchor_from_position(position: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> HudAnchor {
    HudAnchor {
        right_edge: position.x.saturating_add_unsigned(size.width),
        top: position.y,
    }
}

#[cfg(test)]
mod tests {
    use tauri::{PhysicalPosition, PhysicalSize};

    use super::{HudAnchor, anchor_from_position};

    #[test]
    fn anchor_tracks_the_stable_top_right_corner() {
        assert_eq!(
            anchor_from_position(PhysicalPosition::new(668, 64), PhysicalSize::new(320, 68)),
            HudAnchor {
                right_edge: 988,
                top: 64,
            }
        );
        assert_eq!(
            anchor_from_position(PhysicalPosition::new(668, 64), PhysicalSize::new(320, 68)),
            HudAnchor {
                right_edge: 988,
                top: 64,
            }
        );
    }
}
