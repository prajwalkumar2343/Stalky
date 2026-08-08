use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::tauri::{
    PermissionCapability, PermissionSnapshot, PermissionState, PermissionStateLabel,
    PermissionStatus, permission_open_settings, permission_recheck, permission_request,
    subscribe_permissions,
};

const ONBOARDING_STORAGE_KEY: &str = "stalky.permissions.onboarding.v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PermissionViewMode {
    Onboarding,
    Settings,
}

fn onboarding_completed() -> bool {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(ONBOARDING_STORAGE_KEY).ok().flatten())
        .as_deref()
        == Some("complete")
}

fn complete_onboarding() {
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(ONBOARDING_STORAGE_KEY, "complete");
    }
}

fn restore_background_focus() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if let Some(app) = document.get_element_by_id("stalky-app-shell") {
        let _ = app.remove_attribute("aria-hidden");
        let _ = app.remove_attribute("inert");
        if let Some(app) = app.dyn_ref::<web_sys::HtmlElement>() {
            let _ = app.focus();
        }
    }
}

fn contain_modal_focus(event: &web_sys::KeyboardEvent) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(active) = document.active_element() else {
        return;
    };
    let Ok(tabbable) =
        document.query_selector_all("[data-permission-modal=\"true\"] button:not([disabled])")
    else {
        return;
    };
    let Some(first) = tabbable
        .item(0)
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let Some(last) = tabbable
        .item(tabbable.length().saturating_sub(1))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    if event.shift_key() && active.is_same_node(Some(&first)) {
        event.prevent_default();
        let _ = last.focus();
    } else if !event.shift_key() && active.is_same_node(Some(&last)) {
        event.prevent_default();
        let _ = first.focus();
    }
}

/// Applies only current-or-newer snapshots. Native requests and focus
/// rechecks are serialized, but this guard also protects the UI from a late
/// webview future or event delivery race.
fn apply_snapshot(current: &mut Option<PermissionSnapshot>, incoming: PermissionSnapshot) {
    if current
        .as_ref()
        .is_some_and(|current| incoming.sequence <= current.sequence)
    {
        return;
    }
    *current = Some(incoming);
}

fn next_step(current: usize, count: usize) -> usize {
    current.saturating_add(1).min(count.saturating_sub(1))
}

fn capability_label(capability: PermissionCapability) -> &'static str {
    match capability {
        PermissionCapability::ScreenRecording => "Screen Recording",
        PermissionCapability::Accessibility => "Accessibility",
        PermissionCapability::Microphone => "Microphone",
    }
}

fn capability_number(capability: PermissionCapability) -> &'static str {
    match capability {
        PermissionCapability::ScreenRecording => "01",
        PermissionCapability::Accessibility => "02",
        PermissionCapability::Microphone => "03",
    }
}

fn state_detail(state: PermissionState) -> &'static str {
    match state {
        PermissionState::Unknown => {
            "macOS has not exposed a trustworthy answer yet. Check again or choose an action below."
        }
        PermissionState::NotDetermined => {
            "macOS has not asked for this access yet. Stalky will explain why before requesting it."
        }
        PermissionState::Requesting => "Waiting for the macOS approval flow to finish.",
        PermissionState::Rechecking => {
            "Checking the current macOS authorization without prompting."
        }
        PermissionState::Granted => {
            "Ready. Stalky will only use this capability when you start it."
        }
        PermissionState::Denied => {
            "Access is not granted. You can retry once or manage it in System Settings."
        }
        PermissionState::Restricted => {
            "This access is restricted by macOS or device policy. A native request is unavailable."
        }
        PermissionState::Unsupported => "This capability is unavailable on this Mac or build.",
        PermissionState::Revoked => {
            "Access was revoked while Stalky was running. Review the setting and check again."
        }
        PermissionState::RestartRequired => {
            "macOS requires Stalky to restart before this access can be used."
        }
    }
}

fn status_for(
    snapshot: &Option<PermissionSnapshot>,
    capability: PermissionCapability,
) -> Option<PermissionStatus> {
    snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .statuses
            .iter()
            .find(|status| status.capability == capability)
            .cloned()
    })
}

#[component]
pub fn PermissionOnboarding() -> impl IntoView {
    if onboarding_completed() {
        return view! { <PermissionRuntimeSync /> }.into_any();
    }
    let (dismissed, set_dismissed) = signal(onboarding_completed());
    Effect::new(move |_| {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(app) = document.get_element_by_id("stalky-app-shell") else {
            return;
        };
        if dismissed.get() {
            let _ = app.remove_attribute("aria-hidden");
            let _ = app.remove_attribute("inert");
        } else {
            let _ = app.set_attribute("aria-hidden", "true");
            let _ = app.set_attribute("inert", "");
        }
    });
    let dismiss = Callback::new(move |()| {
        complete_onboarding();
        set_dismissed.set(true);
        restore_background_focus();
    });
    view! {
        <Show when=move || !dismissed.get() fallback=|| view! { <></> }>
            <PermissionCenter mode=PermissionViewMode::Onboarding on_dismiss=dismiss />
        </Show>
    }
    .into_any()
}

/// Keeps runtime gating informed even after onboarding has been dismissed and
/// Settings is not mounted. The call is a read-only native probe.
#[component]
fn PermissionRuntimeSync() -> impl IntoView {
    spawn_local(async {
        let _ = permission_recheck().await;
    });
    view! { <></> }
}

#[component]
pub fn PermissionSettings() -> impl IntoView {
    view! { <PermissionCenter mode=PermissionViewMode::Settings on_dismiss=Callback::new(|_| {}) /> }
}

#[component]
fn PermissionCenter(mode: PermissionViewMode, on_dismiss: Callback<()>) -> impl IntoView {
    let (snapshot, set_snapshot) = signal(None::<PermissionSnapshot>);
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let (active_step, set_active_step) = signal(0usize);

    if let Some(subscription) = subscribe_permissions(move |incoming| {
        set_snapshot.update(|current| apply_snapshot(current, incoming));
    }) {
        let subscription = StoredValue::new_local(subscription);
        on_cleanup(move || {
            let _ = subscription.into_inner();
        });
    }

    let refresh = move || {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        spawn_local(async move {
            match permission_recheck().await {
                Ok(incoming) => set_snapshot.update(|current| apply_snapshot(current, incoming)),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };
    refresh();

    let request = move |capability: PermissionCapability| {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            match permission_request(capability).await {
                Ok(incoming) => set_snapshot.update(|current| apply_snapshot(current, incoming)),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let open_settings = move |capability: PermissionCapability| {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            match permission_open_settings(capability).await {
                Ok(incoming) => set_snapshot.update(|current| apply_snapshot(current, incoming)),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let mode_for_view = mode;
    view! {
        <section
            class="permission-center"
            class:permission-modal=move || mode_for_view == PermissionViewMode::Onboarding
            aria-busy=move || busy.get().to_string()
            data-permission-modal=move || (mode_for_view == PermissionViewMode::Onboarding).then_some("true")
            role=move || (mode_for_view == PermissionViewMode::Onboarding).then_some("dialog")
            aria-modal=move || (mode_for_view == PermissionViewMode::Onboarding).then_some("true")
            aria-labelledby=move || (mode_for_view == PermissionViewMode::Onboarding).then_some("permission-onboarding-title")
            on:keydown=move |event: web_sys::KeyboardEvent| {
                if mode_for_view == PermissionViewMode::Onboarding && event.key() == "Escape" {
                    event.prevent_default();
                    on_dismiss.run(());
                } else if mode_for_view == PermissionViewMode::Onboarding && event.key() == "Tab" {
                    contain_modal_focus(&event);
                }
            }
        >
            {move || if mode_for_view == PermissionViewMode::Onboarding {
                view! {
                    <div class="permission-backdrop" aria-hidden="true"></div>
                    <div class="permission-modal-card">
                        <div class="permission-eyebrow">"PRIVATE SETUP · ON THIS MAC"</div>
                        <h1 id="permission-onboarding-title">"A clear boundary before Stalky starts."</h1>
                        <p class="permission-intro">"Stalky can inspect your screen, interface, and microphone locally. Choose each permission yourself; nothing is requested silently at launch."</p>
                        <div class="permission-progress" aria-label="Permission setup progress">
                            {move || (0..3).map(|index| view! { <i class:active=move || active_step.get() == index></i> }).collect_view()}
                        </div>
                        {move || {
                            let capability = [
                                PermissionCapability::ScreenRecording,
                                PermissionCapability::Accessibility,
                                PermissionCapability::Microphone,
                            ][active_step.get().min(2)];
                            status_for(&snapshot.get(), capability).map(|status| view! {
                                <PermissionCardView status=status busy=Signal::from(busy) announce=true on_request=Callback::new(request) on_settings=Callback::new(open_settings) on_recheck=Callback::new(move |_| refresh()) />
                            }).map(|view| view.into_any()).unwrap_or_else(|| view! { <div class="permission-loading" aria-live="polite">"Checking the current authorization…"</div> }.into_any())
                        }}
                        {move || message.get().map(|message| view! { <p class="permission-error" role="alert">{message}</p> })}
                        <div class="permission-modal-actions">
                            <button id="permission-dismiss" class="text-button" autofocus=true on:click=move |_| on_dismiss.run(())>
                                "Continue without access"
                            </button>
                            <button id="permission-continue" class="primary-button" on:click=move |_| {
                                let step = active_step.get_untracked();
                                if step >= 2 { on_dismiss.run(()); } else { set_active_step.set(next_step(step, 3)); }
                            }>
                                {move || if active_step.get() >= 2 { "Finish setup" } else { "Continue" }}<span aria-hidden="true">"→"</span>
                            </button>
                        </div>
                        <p class="permission-footnote">"You can change any choice later in Settings. Escape also dismisses setup."</p>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="settings-permission-intro">
                        <div><span class="permission-eyebrow">"PRIVACY & SECURITY"</span><h2>"Permission center"</h2><p>"Review the three macOS privacy permissions Stalky can use. Rechecks never open prompts."</p></div>
                        <button class="secondary-button" on:click=move |_| refresh() disabled=move || busy.get()>"Check all again"</button>
                    </div>
                    {move || message.get().map(|message| view! { <p class="permission-error" role="alert">{message}</p> })}
                    <div class="permission-list" aria-label="Privacy permissions">
                        {move || snapshot.get().map(|snapshot| snapshot.statuses.into_iter().map(|status| view! {
                            <PermissionCardView status=status busy=Signal::from(busy) announce=false on_request=Callback::new(request) on_settings=Callback::new(open_settings) on_recheck=Callback::new(move |_| refresh()) />
                        }).collect_view())}
                    </div>
                    <div class="settings-group launch-login-note"><h3>"Launch at login"</h3><div class="setting-row"><span>"Optional startup preference"</span><strong>"Separate from privacy permissions"</strong></div><p>"This preference is managed independently and is not part of the macOS TCC permission flow."</p></div>
                }.into_any()
            }}
        </section>
    }
}

#[component]
fn PermissionCardView(
    status: PermissionStatus,
    busy: Signal<bool>,
    announce: bool,
    on_request: Callback<PermissionCapability>,
    on_settings: Callback<PermissionCapability>,
    on_recheck: Callback<PermissionCapability>,
) -> impl IntoView {
    let capability = status.capability;
    let state = status.state;
    view! {
        <article class="permission-card permission-card-rich" aria-live=announce.then_some("polite") aria-busy=move || busy.get().to_string()>
            <div class="permission-card-topline"><span class="permission-number">{capability_number(capability)}</span><span class="permission-state" class:granted=state == PermissionState::Granted>{state.label()}</span></div>
            <div class="permission-card-copy"><h3>{capability_label(capability)}</h3><p>{state_detail(state)}</p></div>
            <div class="permission-card-actions">
                {if status.can_request { view! { <button class="primary-button small" disabled=move || busy.get() on:click=move |_| on_request.run(capability)>{if state == PermissionState::Revoked { "Restore access" } else { "Request access" }}</button> }.into_any() } else { view! { <span class="permission-no-request">{if state == PermissionState::Restricted { "Managed by policy" } else if state == PermissionState::Unsupported { "Unavailable" } else if state == PermissionState::Granted { "Ready" } else { "No request available" }}</span> }.into_any() }}
                {if status.can_open_settings { view! { <button class="secondary-button small" disabled=move || busy.get() on:click=move |_| on_settings.run(capability)>"Open System Settings"</button> }.into_any() } else { view! { <span></span> }.into_any() }}
                <button class="text-button small" disabled=move || busy.get() on:click=move |_| on_recheck.run(capability)>"Check again"</button>
            </div>
            {status.last_error.map(|error| view! { <p class="permission-error">{error.to_string()}</p> })}
        </article>
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_snapshot, next_step};
    use crate::tauri::{PermissionSnapshot, PermissionStatus};

    fn snapshot(sequence: u64) -> PermissionSnapshot {
        PermissionSnapshot {
            schema_version: 1,
            sequence,
            statuses: Vec::<PermissionStatus>::new(),
        }
    }

    #[test]
    fn stale_snapshots_cannot_replace_newer_state() {
        let mut current = Some(snapshot(4));
        apply_snapshot(&mut current, snapshot(3));
        assert_eq!(current.as_ref().unwrap().sequence, 4);
        apply_snapshot(&mut current, snapshot(5));
        assert_eq!(current.as_ref().unwrap().sequence, 5);
        apply_snapshot(&mut current, snapshot(5));
        assert_eq!(current.as_ref().unwrap().sequence, 5);
    }

    #[test]
    fn onboarding_step_is_bounded() {
        assert_eq!(next_step(0, 3), 1);
        assert_eq!(next_step(2, 3), 2);
        assert_eq!(next_step(9, 3), 2);
    }
}
