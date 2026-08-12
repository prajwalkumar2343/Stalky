use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, closure::Closure};

use crate::accessibility::Accessibility;
use crate::components::{Glyph, HOME, MONITOR, PAUSE, PLAY, PULSE, SHIELD, SLIDERS, TARGET};
use crate::onboarding::Onboarding;
use crate::tauri::{
    CaptureState, CaptureStatus, GoogleAuthStatus, OnboardingState, PermissionCapability,
    PermissionState, PermissionStatuses, capture_start, capture_status as load_capture_status,
    capture_stop, google_auth_sign_out, google_auth_status, is_available as capture_is_available,
    onboarding_reset, permission_open_settings, permission_request, permission_statuses,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Overview,
    Capture,
    Accessibility,
    Diagnostics,
    Settings,
}

impl Section {
    fn storage_key(self) -> &'static str {
        match self {
            Self::Overview => "home",
            Self::Capture => "screen",
            Self::Accessibility => "interface",
            Self::Diagnostics => "activity",
            Self::Settings => "settings",
        }
    }

    fn from_storage(value: &str) -> Self {
        match value {
            "screen" => Self::Capture,
            "interface" => Self::Accessibility,
            "activity" => Self::Diagnostics,
            "settings" => Self::Settings,
            _ => Self::Overview,
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    let initial_section = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item("stalky.section").ok().flatten())
        .map_or(Section::Overview, |value| Section::from_storage(&value));
    let (section, set_section) = signal(initial_section);
    let (capture, set_capture) = signal(CaptureStatus::default());
    let (capture_busy, set_capture_busy) = signal(false);
    let (capture_message, set_capture_message) = signal(None::<String>);
    let (onboarding, set_onboarding) =
        signal((!capture_is_available()).then_some(OnboardingState {
            completed: true,
            account_mode: None,
        }));
    let (onboarding_message, set_onboarding_message) = signal(None::<String>);
    let (permissions, set_permissions) = signal(PermissionStatuses::default());

    let refresh_permissions = Callback::new(move |_: ()| {
        if !capture_is_available() {
            return;
        }
        spawn_local(async move {
            if let Ok(status) = permission_statuses().await {
                set_permissions.set(status);
            }
        });
    });

    Effect::new(move |_| {
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item("stalky.section", section.get().storage_key());
        }
    });

    Effect::new(move |_| {
        if !capture_is_available() {
            return;
        }
        spawn_local(async move {
            if let Ok(state) = crate::tauri::onboarding_state().await {
                set_onboarding.set(Some(state));
            }
            if let Ok(status) = permission_statuses().await {
                set_permissions.set(status);
            }
        });
    });

    let permission_poller = gloo_timers::callback::Interval::new(1_500, move || {
        refresh_permissions.run(());
    });
    permission_poller.forget();

    if let Some(window) = web_sys::window() {
        let refresh_on_focus = refresh_permissions;
        let callback =
            Closure::wrap(Box::new(move || refresh_on_focus.run(())) as Box<dyn FnMut()>);
        let _ = window.add_event_listener_with_callback("focus", callback.as_ref().unchecked_ref());
        callback.forget();
    }

    let finish_onboarding =
        Callback::new(move |state: OnboardingState| set_onboarding.set(Some(state)));
    let replay_onboarding = Callback::new(move |_: ()| {
        set_onboarding_message.set(None);
        spawn_local(async move {
            match onboarding_reset().await {
                Ok(state) => set_onboarding.set(Some(state)),
                Err(error) => set_onboarding_message
                    .set(Some(format!("Could not replay onboarding: {error}"))),
            }
        });
    });

    Effect::new(move |_| {
        if !capture_is_available() {
            return;
        }
        spawn_local(async move {
            match load_capture_status().await {
                Ok(status) => set_capture.set(status),
                Err(error) => set_capture_message.set(Some(error)),
            }
        });
    });

    let capture_poller = gloo_timers::callback::Interval::new(1_000, move || {
        if !capture_is_available() || capture_busy.get_untracked() {
            return;
        }
        spawn_local(async move {
            if let Ok(status) = load_capture_status().await {
                set_capture.set(status);
            }
        });
    });
    // App is mounted exactly once for the webview lifetime; the browser owns
    // and releases this interval with the page.
    capture_poller.forget();

    let toggle_capture = Callback::new(move |_: ()| {
        if capture_busy.get_untracked() {
            return;
        }
        let should_stop = capture.get_untracked().needs_stop();
        set_capture_busy.set(true);
        set_capture_message.set(None);
        spawn_local(async move {
            let result = if should_stop {
                capture_stop().await
            } else {
                capture_start().await
            };
            match result {
                Ok(status) => set_capture.set(status),
                Err(error) => set_capture_message.set(Some(error)),
            }
            set_capture_busy.set(false);
        });
    });

    if let Some(window) = web_sys::window() {
        let shortcut = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
            if !event.meta_key() {
                return;
            }
            match event.key().as_str() {
                "," => {
                    event.prevent_default();
                    set_section.set(Section::Settings);
                }
                "Enter" if capture_is_available() => {
                    event.prevent_default();
                    toggle_capture.run(());
                }
                _ => {}
            }
        }) as Box<dyn FnMut(_)>);
        let _ =
            window.add_event_listener_with_callback("keydown", shortcut.as_ref().unchecked_ref());
        shortcut.forget();
    }

    view! {
        <div
            class="app-shell"
            class:app-obscured=move || onboarding.get().is_none_or(|state| !state.completed)
            aria-hidden=move || {
                if onboarding.get().is_none_or(|state| !state.completed) {
                    "true"
                } else {
                    "false"
                }
            }
            inert=move || onboarding.get().is_none_or(|state| !state.completed)
        >
            <header class="titlebar" data-tauri-drag-region="true">
                <div class="traffic-space" aria-hidden="true"></div>
                <div class="workspace-identity">
                    <span class="stalky-mark" aria-hidden="true"><i></i><i></i></span>
                    <strong>"Stalky"</strong>
                </div>
                <div class="titlebar-actions">
                    <span class="global-state" class:paused=move || !capture.get().is_running()>
                        <i></i>{move || match capture.get().state {
                            CaptureState::Running => "Capture active",
                            CaptureState::Starting => "Starting capture",
                            CaptureState::Stopping => "Stopping capture",
                            CaptureState::Failed => "Capture needs attention",
                            CaptureState::Stopped => "Capture off",
                        }}
                    </span>
                    <button
                        class="titlebar-primary"
                        disabled=move || capture_busy.get() || !capture_is_available()
                        on:click=move |_| toggle_capture.run(())
                    >
                        {move || if capture.get().needs_stop() { view! { <Glyph paths=PAUSE /> }.into_any() } else { view! { <Glyph paths=PLAY /> }.into_any() }}
                        {move || if capture_busy.get() { "Working…" } else if capture.get().needs_stop() { "Pause capture" } else { "Start capture" }}
                    </button>
                </div>
            </header>

            <aside class="sidebar">
                <div class="sidebar-heading">"Workspace"</div>
                <nav aria-label="Stalky sections">
                    <NavButton label="Home" glyph=HOME active=Signal::derive(move || section.get() == Section::Overview) on_click=move || set_section.set(Section::Overview) />
                    <NavButton label="Screen" glyph=MONITOR active=Signal::derive(move || section.get() == Section::Capture) on_click=move || set_section.set(Section::Capture) />
                    <NavButton label="Interface" glyph=TARGET active=Signal::derive(move || section.get() == Section::Accessibility) on_click=move || set_section.set(Section::Accessibility) />
                    <NavButton label="Activity" glyph=PULSE active=Signal::derive(move || section.get() == Section::Diagnostics) on_click=move || set_section.set(Section::Diagnostics) />
                </nav>

                <div class="sidebar-spacer"></div>
                <nav class="sidebar-bottom" aria-label="Application settings">
                    <NavButton label="Settings" glyph=SLIDERS active=Signal::derive(move || section.get() == Section::Settings) on_click=move || set_section.set(Section::Settings) />
                </nav>
                <div class="sidebar-boundary"><Glyph paths=SHIELD /><span><strong>"Local by default"</strong><small>"Nothing uploads automatically"</small></span></div>
            </aside>

            <section class="workspace">
                {move || match section.get() {
                    Section::Overview => view! { <Overview capture=Signal::from(capture) permissions=Signal::from(permissions) /> }.into_any(),
                    Section::Capture => view! { <Capture capture=Signal::from(capture) message=Signal::from(capture_message) permissions=Signal::from(permissions) refresh=refresh_permissions /> }.into_any(),
                    Section::Accessibility => view! { <Accessibility /> }.into_any(),
                    Section::Diagnostics => view! { <Diagnostics capture=Signal::from(capture) /> }.into_any(),
                    Section::Settings => view! { <Settings permissions=Signal::from(permissions) refresh=refresh_permissions on_reset=replay_onboarding /> }.into_any(),
                }}
            </section>
        </div>
        {move || if onboarding.get().is_none_or(|state| !state.completed) {
            view! { <Onboarding on_complete=finish_onboarding /> }.into_any()
        } else {
            ().into_any()
        }}
        {move || onboarding_message.get().map(|message| view! {
            <div class="app-message" role="alert" aria-live="assertive">
                <strong>"Onboarding could not be reset"</strong>
                <span>{message}</span>
            </div>
        })}
    }
}

#[component]
fn NavButton<F>(
    label: &'static str,
    glyph: &'static [&'static str],
    active: Signal<bool>,
    on_click: F,
) -> impl IntoView
where
    F: Fn() + Send + Sync + 'static,
{
    view! {
        <button class="nav-item" class:active=move || active.get() on:click=move |_| on_click()>
            <span class="nav-glyph" aria-hidden="true"><Glyph paths=glyph /></span>
            <span>{label}</span>
        </button>
    }
}

#[component]
fn Overview(
    capture: Signal<CaptureStatus>,
    permissions: Signal<PermissionStatuses>,
) -> impl IntoView {
    let ready_count = Signal::derive(move || {
        [
            permissions.get().screen_recording,
            permissions.get().accessibility,
            permissions.get().microphone,
        ]
        .into_iter()
        .filter(|state| state.is_granted())
        .count()
    });
    view! {
        <div class="page overview-page">
            <PageHeader eyebrow="Home" title="Local context, at a glance." body="See what Stalky can access, what is active, and what stays on this Mac." />
            <section class="home-hero" class:live=move || capture.get().is_running()>
                <div class="home-hero-status">
                    <span class="hero-beacon" aria-hidden="true"><i></i></span>
                    <div>
                        <span>{move || if capture.get().is_running() { "CAPTURE ACTIVE" } else { "READY WHEN YOU ARE" }}</span>
                        <h2>{move || if capture.get().is_running() { "Screen context is live." } else { "Nothing is being captured." }}</h2>
                        <p>{move || if capture.get().is_running() { "The latest bounded frame is held in memory and never returned to the interface." } else { "Start capture when you want Stalky to observe the primary display locally." }}</p>
                    </div>
                </div>
                <div class="home-hero-stats">
                    <div><strong>{move || capture.get().metrics.accepted_frames}</strong><span>"Frames this run"</span></div>
                    <div><strong>{move || ready_count.get()}"/3"</strong><span>"Permissions ready"</span></div>
                    <div><strong>{move || capture.get().metrics.last_frame.as_ref().map_or_else(|| "—".to_owned(), |frame| format!("{} × {}", frame.width, frame.height))}</strong><span>"Latest frame"</span></div>
                </div>
            </section>
            <div class="content-section-heading"><div><span>"Readiness"</span><h2>"Capabilities"</h2></div><p>"Permissions stay independent and can be changed at any time."</p></div>
            <div class="status-panel clean-status-panel">
                <div class="status-row"><span class="status-dot" class:good=move || permissions.get().screen_recording.is_granted() aria-hidden="true"></span><div class="status-copy"><strong>"Screen"</strong><span>"Primary display · bounded memory"</span></div><span class="status-value" class:good=move || permissions.get().screen_recording.is_granted()>{move || permissions.get().screen_recording.label()}</span></div>
                <LiveStatusRow label="Interface" detail="Focused hierarchy · explicit controls only" state=Signal::derive(move || permissions.get().accessibility) />
                <LiveStatusRow label="Microphone" detail="Local input access · no session active" state=Signal::derive(move || permissions.get().microphone) />
            </div>
        </div>
    }
}

#[component]
fn Capture(
    capture: Signal<CaptureStatus>,
    message: Signal<Option<String>>,
    permissions: Signal<PermissionStatuses>,
    refresh: Callback<()>,
) -> impl IntoView {
    let screen_permission = Signal::derive(move || permissions.get().screen_recording);
    let (permission_busy, set_permission_busy) = signal(false);
    let (permission_message, set_permission_message) = signal(None::<String>);
    let request_screen_permission = move |_| {
        if permission_busy.get_untracked() {
            return;
        }
        if screen_permission.get_untracked().needs_settings() {
            spawn_local(async move {
                let _ = permission_open_settings(PermissionCapability::ScreenRecording).await;
            });
            return;
        }
        set_permission_busy.set(true);
        spawn_local(async move {
            if let Err(error) = permission_request(PermissionCapability::ScreenRecording).await {
                set_permission_message.set(Some(error));
            }
            refresh.run(());
            set_permission_busy.set(false);
        });
    };
    view! {
        <div class="page">
            <PageHeader eyebrow="Capture" title="See only what matters." body="A bounded, privacy-filtered ScreenCaptureKit stream with explicit start and stop controls."/>
            <div class="feature-permission-strip"><span class="status-dot" class:good=move || screen_permission.get().is_granted() aria-hidden="true"></span><span>"Screen Recording"</span><strong aria-live="polite">{move || screen_permission.get().label()}</strong><button class="text-button" disabled=move || permission_busy.get() || screen_permission.get().is_granted() on:click=request_screen_permission>{move || if screen_permission.get().needs_settings() { "Open Settings" } else if permission_busy.get() { "Waiting…" } else { "Request access" }}</button></div>
            <div class="feature-stage">
                <div class="stage-toolbar">
                    <span class="live-badge">{move || if capture.get().is_running() { "LIVE" } else { "OFF" }}</span>
                    <span>"Primary display"</span>
                    <span>{move || format!("{} accepted", capture.get().metrics.accepted_frames)}</span>
                </div>
                <div class="capture-canvas" class:paused=move || !capture.get().is_running()>
                    <div class="capture-sidebar"></div>
                    <div class="capture-lines"><i></i><i></i><i></i><i></i><i></i></div>
                    <span class="redaction-block">{move || if capture.get().is_running() { "Ephemeral frame" } else { "Capture off" }}</span>
                </div>
            </div>
            {move || message.get().map(|message| view! { <div class="boundary-callout capture-error" role="status" aria-live="polite"><span>"Capture unavailable"</span><p>{message}</p><strong>"Review permissions"</strong></div> })}
            {move || permission_message.get().map(|message| view! { <div class="settings-message" role="status" aria-live="polite">{message}</div> })}
            <div class="two-column">
                <SettingsGroup title="Sampling">
                    <div class="setting-row"><span>"State"</span><strong>{move || capture.get().state.label()}</strong></div>
                    <SettingRow label="Maximum rate" value="1 fps"/>
                    <SettingRow label="Queue depth" value="3 frames"/>
                </SettingsGroup>
                <SettingsGroup title="Privacy">
                    <SettingRow label="Stalky process" value="Excluded"/>
                    <SettingRow label="Raw retention" value="Memory only"/>
                    <div class="setting-row"><span>"Dropped"</span><strong>{move || capture.get().metrics.dropped_frames}</strong></div>
                </SettingsGroup>
            </div>
        </div>
    }
}

#[component]
fn Diagnostics(capture: Signal<CaptureStatus>) -> impl IntoView {
    view! {
        <div class="page activity-page">
            <PageHeader eyebrow="Activity" title="A truthful view of this run." body="Only live, content-free capture counters are shown here. Stalky does not fabricate history or retain raw frames."/>
            <section class="activity-summary">
                <div class="activity-state"><span class="status-dot" class:good=move || capture.get().is_running()></span><div><span>"Current state"</span><h2>{move || capture.get().state.label()}</h2></div></div>
                <div class="activity-stat"><span>"Accepted frames"</span><strong>{move || capture.get().metrics.accepted_frames}</strong></div>
                <div class="activity-stat"><span>"Dropped frames"</span><strong>{move || capture.get().metrics.dropped_frames}</strong></div>
            </section>
            <section class="activity-detail">
                <div><span>"Source"</span><strong>"Primary display"</strong><small>"Stalky is excluded when application metadata is available."</small></div>
                <div><span>"Latest frame"</span><strong>{move || capture.get().metrics.last_frame.as_ref().map_or_else(|| "No frame yet".to_owned(), |frame| format!("{} × {}", frame.width, frame.height))}</strong><small>"Dimensions only; raw bytes never cross IPC."</small></div>
                <div><span>"Retention"</span><strong>"Memory only"</strong><small>"The newest bounded frame replaces the previous one."</small></div>
            </section>
        </div>
    }
}

#[component]
fn Settings(
    permissions: Signal<PermissionStatuses>,
    refresh: Callback<()>,
    on_reset: Callback<()>,
) -> impl IntoView {
    let (auth, set_auth) = signal(GoogleAuthStatus::default());
    let (message, set_message) = signal(None::<String>);
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(status) = google_auth_status().await {
                set_auth.set(status);
            }
        });
    });
    let sign_out = move |_| {
        spawn_local(async move {
            match google_auth_sign_out().await {
                Ok(status) => set_auth.set(status),
                Err(error) => set_message.set(Some(error)),
            }
        });
    };
    view! {
        <div class="page settings-page">
            <PageHeader eyebrow="Settings" title="Your Mac, your boundaries." body="Every ambient capability remains visible, reversible, and independently configurable."/>
            <div class="settings-account settings-group">
                <div><span class="settings-label">"Account"</span><h2>{move || if auth.get().signed_in { "Google connected" } else { "Local workspace" }}</h2><p>{move || if auth.get().signed_in { "Your browser sign-in is stored in macOS Keychain. Stalky keeps capture permissions independent." } else { "No account is required. This workspace stays on this Mac." }}</p></div>
                {move || auth.get().signed_in.then_some(view! { <button class="secondary-button" on:click=sign_out>"Sign out"</button> })}
            </div>
            <section class="settings-section"><div class="section-heading compact"><div><span>"Permissions"</span><h2>"Live OS status"</h2></div><button class="text-button" on:click=move |_| refresh.run(())>"Refresh status"</button></div>
                <div class="permission-list">
                    <LivePermissionCard number="01" capability=PermissionCapability::ScreenRecording title="Screen Recording" body="Capture the display or window you explicitly select." permissions=permissions refresh=refresh />
                    <LivePermissionCard number="02" capability=PermissionCapability::Accessibility title="Accessibility" body="Observe interface structure and run controls you explicitly choose." permissions=permissions refresh=refresh />
                    <LivePermissionCard number="03" capability=PermissionCapability::Microphone title="Microphone" body="Enable local input testing and voice activity detection." permissions=permissions refresh=refresh />
                    <LivePermissionCard number="04" capability=PermissionCapability::LaunchAtLogin title="Launch at login" body="Optional convenience; unavailable until the native binding is maintained." permissions=permissions refresh=refresh />
                </div>
            </section>
            <section class="settings-group reset-panel"><div><span class="settings-label">"First-run"</span><h2>"Replay onboarding"</h2><p>"Review account choice and each optional permission step. OS permissions remain the source of truth."</p></div><button class="secondary-button" on:click=move |_| on_reset.run(())>"Replay onboarding"</button></section>
            {move || message.get().map(|copy| view! { <div class="settings-message" aria-live="polite">{copy}</div> })}
        </div>
    }
}

#[component]
fn LiveStatusRow(
    label: &'static str,
    detail: &'static str,
    state: Signal<PermissionState>,
) -> impl IntoView {
    view! {
        <div class="status-row">
            <span class="status-dot" class:good=move || state.get().is_granted() aria-hidden="true"></span>
            <div class="status-copy"><strong>{label}</strong><span>{detail}</span></div>
            <span class="status-value" class:good=move || state.get().is_granted()>{move || state.get().label()}</span>
        </div>
    }
}

#[component]
fn LivePermissionCard(
    number: &'static str,
    capability: PermissionCapability,
    title: &'static str,
    body: &'static str,
    permissions: Signal<PermissionStatuses>,
    refresh: Callback<()>,
) -> impl IntoView {
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let state = Signal::derive(move || permissions.get().state(capability));
    let request = move |_| {
        if busy.get_untracked() || state.get_untracked() == PermissionState::Unsupported {
            return;
        }
        if state.get_untracked().needs_settings() {
            spawn_local(async move {
                if let Err(error) = permission_open_settings(capability).await {
                    set_message.set(Some(error));
                }
            });
            return;
        }
        set_busy.set(true);
        spawn_local(async move {
            match permission_request(capability).await {
                Ok(_) => refresh.run(()),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };
    view! {
        <article class="permission-card live-permission-card">
            <span class="permission-number">{number}</span>
            <div class="permission-card-copy"><h3>{title}</h3><p>{body}</p><span class="permission-state-label">{move || state.get().label()}</span></div>
            {move || if state.get().is_granted() || state.get() == PermissionState::Unsupported {
                view! { <span class="permission-final-state" class:good=move || state.get().is_granted()>{move || state.get().label()}</span> }.into_any()
            } else {
                view! { <button class="secondary-button compact-button" disabled=move || busy.get() on:click=request>{move || if busy.get() { "Waiting…" } else if state.get().needs_settings() { "Open Settings" } else { "Request access" }}</button> }.into_any()
            }}
            {move || message.get().map(|copy| view! { <span class="permission-inline-message" aria-live="polite">{copy}</span> })}
        </article>
    }
}

#[component]
fn PageHeader(eyebrow: &'static str, title: &'static str, body: &'static str) -> impl IntoView {
    view! { <header class="page-header"><span>{eyebrow}</span><h1>{title}</h1><p>{body}</p></header> }
}

#[component]
fn SettingsGroup(title: &'static str, children: Children) -> impl IntoView {
    view! { <section class="settings-group"><h3>{title}</h3>{children()}</section> }
}

#[component]
fn SettingRow(label: &'static str, value: &'static str) -> impl IntoView {
    view! { <div class="setting-row"><span>{label}</span><strong>{value}</strong></div> }
}
