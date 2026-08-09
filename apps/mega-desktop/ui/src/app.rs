use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, closure::Closure};

use crate::accessibility::Accessibility;
use crate::components::{EventRow, InspectorField, MetricCard, Tone};
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
    Audio,
    Diagnostics,
    Settings,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Capture => "Screen capture",
            Self::Accessibility => "Accessibility",
            Self::Audio => "Audio input",
            Self::Diagnostics => "Diagnostics",
            Self::Settings => "Settings",
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    let (section, set_section) = signal(Section::Overview);
    let (private, set_private) = signal(false);
    let (inspector_open, set_inspector_open) = signal(true);
    let (capture, set_capture) = signal(CaptureStatus::default());
    let (capture_busy, set_capture_busy) = signal(false);
    let (capture_message, set_capture_message) = signal(None::<String>);
    let (onboarding, set_onboarding) = signal(None::<OnboardingState>);
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
        spawn_local(async move {
            if let Ok(state) = onboarding_reset().await {
                set_onboarding.set(Some(state));
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

    let toggle_capture = move |_| {
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
    };

    view! {
        <div
            class="app-shell"
            class:inspector-closed=move || !inspector_open.get() || section.get() == Section::Accessibility
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
                    <span class="slash">"/"</span>
                    <span>{move || section.get().title()}</span>
                </div>
                <div class="titlebar-actions">
                    <span class="global-state" class:paused=move || !capture.get().is_running()>
                        <i></i>{move || match capture.get().state {
                            CaptureState::Running if private.get() => "Private",
                            CaptureState::Running => "Capture active",
                            CaptureState::Starting => "Starting capture",
                            CaptureState::Stopping => "Stopping capture",
                            CaptureState::Failed => "Capture needs attention",
                            CaptureState::Stopped => "Capture off",
                        }}
                    </span>
                    <button
                        class="icon-button"
                        class:active=move || inspector_open.get()
                        class:hidden=move || section.get() == Section::Accessibility
                        aria-label="Toggle inspector"
                        title="Toggle inspector"
                        on:click=move |_| set_inspector_open.update(|open| *open = !*open)
                    >"◫"</button>
                </div>
            </header>

            <aside class="sidebar">
                <div class="sidebar-heading">"Workspace"</div>
                <nav aria-label="Infrastructure sections">
                    <NavButton label="Overview" glyph="⌂" active=Signal::derive(move || section.get() == Section::Overview) on_click=move || set_section.set(Section::Overview) />
                    <NavButton label="Capture" glyph="▣" active=Signal::derive(move || section.get() == Section::Capture) on_click=move || set_section.set(Section::Capture) />
                    <NavButton label="Accessibility" glyph="◎" active=Signal::derive(move || section.get() == Section::Accessibility) on_click=move || set_section.set(Section::Accessibility) />
                    <NavButton label="Audio" glyph="≋" active=Signal::derive(move || section.get() == Section::Audio) on_click=move || set_section.set(Section::Audio) />
                </nav>

                <div class="sidebar-heading secondary">"System"</div>
                <nav aria-label="System sections">
                    <NavButton label="Diagnostics" glyph="⌁" active=Signal::derive(move || section.get() == Section::Diagnostics) on_click=move || set_section.set(Section::Diagnostics) />
                    <NavButton label="Settings" glyph="⚙" active=Signal::derive(move || section.get() == Section::Settings) on_click=move || set_section.set(Section::Settings) />
                </nav>

                <div class="sidebar-spacer"></div>
                <div class="sidebar-health">
                    <div class="health-ring" aria-hidden="true"><span></span></div>
                    <div><strong>"Local runtime ready"</strong><span>"Protected services are opt-in"</span></div>
                </div>
                <button class="profile-row" disabled title="Workspace profile is not available in this build">
                    <span class="avatar">"M"</span>
                    <span><strong>"Local workspace"</strong><small>"On this Mac"</small></span>
                    <span class="more">"···"</span>
                </button>
            </aside>

            <section class="workspace">
                {move || match section.get() {
                    Section::Overview => view! { <Overview capture=Signal::from(capture) permissions=Signal::from(permissions) /> }.into_any(),
                    Section::Capture => view! { <Capture capture=Signal::from(capture) message=Signal::from(capture_message) permissions=Signal::from(permissions) refresh=refresh_permissions /> }.into_any(),
                    Section::Accessibility => view! { <Accessibility /> }.into_any(),
                    Section::Audio => view! { <Audio permissions=Signal::from(permissions) refresh=refresh_permissions /> }.into_any(),
                    Section::Diagnostics => view! { <Diagnostics /> }.into_any(),
                    Section::Settings => view! { <Settings permissions=Signal::from(permissions) refresh=refresh_permissions on_reset=replay_onboarding /> }.into_any(),
                }}
            </section>

            <aside class="inspector" class:closed=move || !inspector_open.get() || section.get() == Section::Accessibility>
                <div class="inspector-header">
                    <div><span>"Inspector"</span><strong>"Current context"</strong></div>
                    <button class="icon-button small" aria-label="Close inspector" on:click=move |_| set_inspector_open.set(false)>"×"</button>
                </div>
                <div class="live-preview" class:paused=move || !capture.get().is_running()>
                    <div class="preview-grid" aria-hidden="true"><i></i><i></i><i></i><i></i></div>
                    <span class="live-badge">{move || if capture.get().is_running() { "LIVE" } else { "OFF" }}</span>
                    <div class="preview-window"><span></span><span></span><span></span></div>
                </div>
                <div class="inspector-section">
                    <h3>"Source"</h3>
                    <InspectorField label="Display" value="Primary display" />
                    <InspectorField label="Scope" value="Full display" />
                    <InspectorField label="Stalky process" value="Excluded" />
                </div>
                <div class="inspector-section">
                    <h3>"Latest frame"</h3>
                    <div class="inspector-field"><span>"Dimensions"</span><strong>{move || capture.get().metrics.last_frame.as_ref().map_or_else(|| "—".to_owned(), |frame| format!("{} × {}", frame.width, frame.height))}</strong></div>
                    <div class="inspector-field"><span>"Accepted"</span><strong>{move || capture.get().metrics.accepted_frames}</strong></div>
                    <div class="inspector-field"><span>"Dropped"</span><strong>{move || capture.get().metrics.dropped_frames}</strong></div>
                </div>
                <div class="privacy-note">
                    <span aria-hidden="true">"◇"</span>
                    <p><strong>"Ephemeral by default"</strong>"Frames stay in memory and are discarded after inspection."</p>
                </div>
            </aside>

            <footer class="control-dock">
                <div class="dock-context"><span class="pulse" class:paused=move || !capture.get().is_running()></span><span>{move || if section.get() == Section::Accessibility { "Accessibility controls are scoped to the selected live element".to_owned() } else { capture_message.get().unwrap_or_else(|| if capture.get().is_running() { "Watching the selected display".to_owned() } else { "Screen capture is off".to_owned() }) }}</span></div>
                <div class="dock-actions">
                    {move || if section.get() == Section::Accessibility {
                        view! { <span class="ax-dock-note"><i></i>"No autonomous controls"</span> }.into_any()
                    } else {
                        view! {
                            <button class="dock-button" class:active=move || private.get() on:click=move |_| set_private.update(|value| *value = !*value)><span>"◇"</span>{move || if private.get() { "Private on" } else { "Private" }}</button>
                            <button class="dock-button" disabled title="Snapshot is not available in this build"><span>"◉"</span>"Snapshot"</button>
                            <button class="dock-button" disabled title="Microphone testing is available from the Audio section"><span>"≋"</span>"Mic test"</button>
                            <button class="primary-dock-button" disabled=move || capture_busy.get() on:click=toggle_capture>
                                <span>{move || if capture.get().needs_stop() { "Ⅱ" } else { "▶" }}</span>{move || if capture_busy.get() { "Working…" } else { match capture.get().state { CaptureState::Running => "Pause capture", CaptureState::Failed => "Reset capture", _ => "Start capture" } }}
                            </button>
                        }.into_any()
                    }}
                </div>
            </footer>
        </div>
        {move || if onboarding.get().is_none_or(|state| !state.completed) {
            view! { <Onboarding on_complete=finish_onboarding /> }.into_any()
        } else {
            ().into_any()
        }}
    }
}

#[component]
fn NavButton<F>(
    label: &'static str,
    glyph: &'static str,
    active: Signal<bool>,
    on_click: F,
) -> impl IntoView
where
    F: Fn() + Send + Sync + 'static,
{
    view! {
        <button class="nav-item" class:active=move || active.get() on:click=move |_| on_click()>
            <span class="nav-glyph" aria-hidden="true">{glyph}</span>
            <span>{label}</span>
        </button>
    }
}

#[component]
fn Overview(
    capture: Signal<CaptureStatus>,
    permissions: Signal<PermissionStatuses>,
) -> impl IntoView {
    view! {
        <div class="page overview-page">
            <PageHeader eyebrow="Infrastructure" title="Everything, quietly in view." body="Stalky keeps screen, interface, and audio context ready on this Mac—without sending or saving raw content." />
            <div class="status-panel">
                <div class="status-row">
                    <span class="status-dot" class:good=move || capture.get().is_running() aria-hidden="true"></span>
                    <div class="status-copy"><strong>"Screen capture"</strong><span>"Primary display · 1 fps · memory only"</span></div>
                    <span class="status-value" class:good=move || capture.get().is_running()>{move || if capture.get().is_running() { "Live" } else { "Off" }}</span>
                </div>
                <LiveStatusRow label="Accessibility" detail="Observation and explicit controls" state=Signal::derive(move || permissions.get().accessibility) />
                <LiveStatusRow label="Microphone" detail="No input session active" state=Signal::derive(move || permissions.get().microphone) />
                <LiveStatusRow label="Background" detail="Launch at login" state=Signal::derive(move || permissions.get().launch_at_login) />
            </div>
            <div class="section-heading"><div><span>"Live performance"</span><h2>"A light footprint."</h2></div><button class="text-button" disabled title="Diagnostics navigation is not available in this build">"Open diagnostics" <span>"→"</span></button></div>
            <div class="metric-grid">
                <article class="metric-card"><div class="metric-topline"><span>"Accepted frames"</span><span class="metric-trend">"this run"</span></div><div class="metric-value">{move || capture.get().metrics.accepted_frames}<small>" frames"</small></div><div class="meter-track" aria-hidden="true"><span style="width: 28%"></span></div></article>
                <MetricCard eyebrow="CPU" value="3.2" suffix="%" trend="−0.4%" meter=20 />
                <MetricCard eyebrow="Memory" value="186" suffix=" MB" trend="steady" meter=42 />
            </div>
            <div class="section-heading compact"><div><span>"Activity"</span><h2>"Recent events"</h2></div><button class="filter-button" disabled title="Event filtering is not available in this build">"All systems" <span>"⌄"</span></button></div>
            <div class="event-list">
                <EventRow time="Now" source="Capture" title="Frame changed" detail="18% of the active display changed; preview refreshed." tone=Tone::Good />
                <EventRow time="−12s" source="Accessibility" title="Focus moved" detail="Focused window changed from Finder to Safari." tone=Tone::Quiet />
                <EventRow time="−48s" source="Privacy" title="Sensitive region hidden" detail="One secure field was removed from the context view." tone=Tone::Warning />
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
fn Audio(permissions: Signal<PermissionStatuses>, refresh: Callback<()>) -> impl IntoView {
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let microphone = Signal::derive(move || permissions.get().microphone);
    let request_microphone = move |_| {
        if busy.get_untracked() || microphone.get_untracked().is_granted() {
            return;
        }
        if microphone.get_untracked().needs_settings() {
            spawn_local(async move {
                if let Err(error) = permission_open_settings(PermissionCapability::Microphone).await
                {
                    set_message.set(Some(error));
                }
            });
            return;
        }
        if microphone.get_untracked() == PermissionState::Unsupported {
            return;
        }
        set_busy.set(true);
        spawn_local(async move {
            match permission_request(PermissionCapability::Microphone).await {
                Ok(_) => refresh.run(()),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };
    view! {
        <div class="page">
            <PageHeader eyebrow="Audio" title="Ready when you hold." body="Local microphone metering and voice activity detection. No transcription, upload, or automatic recording."/>
            <div class="audio-stage"><div class="audio-orb"><i></i><i></i><i></i></div><div><span class="micro-label">{move || if microphone.get().is_granted() { "INPUT READY" } else { "PERMISSION NEEDED" }}</span><h2>"System microphone"</h2><p aria-live="polite">{move || format!("{} · no session active", microphone.get().label())}</p></div><button class="hold-button" disabled=move || busy.get() || microphone.get().is_granted() || microphone.get() == PermissionState::Unsupported on:click=request_microphone>{move || if busy.get() { "Waiting…" } else if microphone.get().is_granted() { "Input ready" } else if microphone.get().needs_settings() { "Open Settings" } else if microphone.get() == PermissionState::Unsupported { "Unavailable" } else { "Request access" }}</button></div>
            <div class="waveform" aria-label="Audio level: inactive">{(0..42).map(|index| view! { <i style=format!("height:{}%", 14 + ((index * 17) % 62))></i> }).collect_view()}</div>
            {move || message.get().map(|copy| view! { <div class="settings-message" aria-live="polite">{copy}</div> })}
            <div class="two-column"><SettingsGroup title="Input"><SettingRow label="Device" value="System default"/><SettingRow label="Analysis format" value="16 kHz mono"/></SettingsGroup><SettingsGroup title="Privacy"><SettingRow label="Ring buffer" value="3 seconds"/><SettingRow label="Audio files" value="Never automatic"/></SettingsGroup></div>
        </div>
    }
}

#[component]
fn Diagnostics() -> impl IntoView {
    view! { <div class="page"><PageHeader eyebrow="Diagnostics" title="Make the invisible inspectable." body="Bounded, content-free operational data for understanding health and failures."/><div class="diagnostic-summary"><div><span class="health-ring large"><i></i></span><div><h2>"Healthy"</h2><p>"No subsystem requires attention."</p></div></div><button class="secondary-button" disabled title="Support bundle export is not available in this build">"Export support bundle"</button></div><div class="metric-grid"><MetricCard eyebrow="IPC p95" value="8" suffix=" ms" trend="healthy" meter=12/><MetricCard eyebrow="Dropped events" value="0" suffix="" trend="last hour" meter=0/><MetricCard eyebrow="Store p95" value="3" suffix=" ms" trend="healthy" meter=8/></div><div class="log-console"><div><span>"20:21:04"</span><strong>"capture"</strong><p>"adaptive rate settled at 0.8 fps"</p></div><div><span>"20:20:58"</span><strong>"privacy"</strong><p>"redaction rules applied (1 region)"</p></div><div><span>"20:20:55"</span><strong>"runtime"</strong><p>"all supervisors running"</p></div></div></div> }
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
            <button class="secondary-button compact-button" disabled=move || busy.get() || state.get() == PermissionState::Unsupported on:click=request>{move || if busy.get() { "Waiting…" } else if state.get().is_granted() { "Granted" } else if state.get().needs_settings() { "Open Settings" } else { "Request" }}</button>
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
