use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::accessibility::Accessibility;
use crate::components::{EventRow, InspectorField, MetricCard, StatusRow, Tone};
use crate::permissions::{PermissionOnboarding, PermissionSettings};
use crate::tauri::{
    CaptureState, CaptureStatus, capture_start, capture_status as load_capture_status,
    capture_stop, is_available as capture_is_available,
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
        <PermissionOnboarding />
        <main id="stalky-app-shell" tabindex="-1" class="app-shell" class:inspector-closed=move || !inspector_open.get() || section.get() == Section::Accessibility>
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
                <button class="profile-row">
                    <span class="avatar">"M"</span>
                    <span><strong>"Local workspace"</strong><small>"On this Mac"</small></span>
                    <span class="more">"···"</span>
                </button>
            </aside>

            <section class="workspace">
                {move || match section.get() {
                    Section::Overview => view! { <Overview capture=Signal::from(capture) /> }.into_any(),
                    Section::Capture => view! { <Capture capture=Signal::from(capture) message=Signal::from(capture_message) /> }.into_any(),
                    Section::Accessibility => view! { <Accessibility /> }.into_any(),
                    Section::Audio => view! { <Audio /> }.into_any(),
                    Section::Diagnostics => view! { <Diagnostics /> }.into_any(),
                    Section::Settings => view! { <Settings /> }.into_any(),
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
                            <button class="dock-button"><span>"◉"</span>"Snapshot"</button>
                            <button class="dock-button"><span>"≋"</span>"Mic test"</button>
                            <button class="primary-dock-button" disabled=move || capture_busy.get() on:click=toggle_capture>
                                <span>{move || if capture.get().needs_stop() { "Ⅱ" } else { "▶" }}</span>{move || if capture_busy.get() { "Working…" } else { match capture.get().state { CaptureState::Running => "Pause capture", CaptureState::Failed => "Reset capture", _ => "Start capture" } }}
                            </button>
                        }.into_any()
                    }}
                </div>
            </footer>
        </main>
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
fn Overview(capture: Signal<CaptureStatus>) -> impl IntoView {
    view! {
        <div class="page overview-page">
            <PageHeader eyebrow="Infrastructure" title="Everything, quietly in view." body="Stalky keeps screen, interface, and audio context ready on this Mac—without sending or saving raw content." />
            <div class="status-panel">
                <div class="status-row">
                    <span class="status-dot" class:good=move || capture.get().is_running() aria-hidden="true"></span>
                    <div class="status-copy"><strong>"Screen capture"</strong><span>"Primary display · 1 fps · memory only"</span></div>
                    <span class="status-value" class:good=move || capture.get().is_running()>{move || if capture.get().is_running() { "Live" } else { "Off" }}</span>
                </div>
                <StatusRow label="Accessibility" detail="Observation and explicit controls" value="Opt-in" tone=Tone::Warning />
                <StatusRow label="Microphone" detail="No input session active" value="Off" tone=Tone::Quiet />
                <StatusRow label="Background" detail="Launch at login disabled" value="Optional" tone=Tone::Warning />
            </div>
            <div class="section-heading"><div><span>"Live performance"</span><h2>"A light footprint."</h2></div><button class="text-button">"Open diagnostics" <span>"→"</span></button></div>
            <div class="metric-grid">
                <article class="metric-card"><div class="metric-topline"><span>"Accepted frames"</span><span class="metric-trend">"this run"</span></div><div class="metric-value">{move || capture.get().metrics.accepted_frames}<small>" frames"</small></div><div class="meter-track" aria-hidden="true"><span style="width: 28%"></span></div></article>
                <MetricCard eyebrow="CPU" value="3.2" suffix="%" trend="−0.4%" meter=20 />
                <MetricCard eyebrow="Memory" value="186" suffix=" MB" trend="steady" meter=42 />
            </div>
            <div class="section-heading compact"><div><span>"Activity"</span><h2>"Recent events"</h2></div><button class="filter-button">"All systems" <span>"⌄"</span></button></div>
            <div class="event-list">
                <EventRow time="Now" source="Capture" title="Frame changed" detail="18% of the active display changed; preview refreshed." tone=Tone::Good />
                <EventRow time="−12s" source="Accessibility" title="Focus moved" detail="Focused window changed from Finder to Safari." tone=Tone::Quiet />
                <EventRow time="−48s" source="Privacy" title="Sensitive region hidden" detail="One secure field was removed from the context view." tone=Tone::Warning />
            </div>
        </div>
    }
}

#[component]
fn Capture(capture: Signal<CaptureStatus>, message: Signal<Option<String>>) -> impl IntoView {
    view! {
        <div class="page">
            <PageHeader eyebrow="Capture" title="See only what matters." body="A bounded, privacy-filtered ScreenCaptureKit stream with explicit start and stop controls."/>
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
            {move || message.get().map(|message| view! { <div class="boundary-callout capture-error"><span>"Capture unavailable"</span><p>{message}</p><strong>"Review permissions"</strong></div> })}
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
fn Audio() -> impl IntoView {
    view! { <div class="page"><PageHeader eyebrow="Audio" title="Ready when you hold." body="Local microphone metering and voice activity detection. No transcription, upload, or automatic recording."/><div class="audio-stage"><div class="audio-orb"><i></i><i></i><i></i></div><div><span class="micro-label">"INPUT READY"</span><h2>"MacBook Pro Microphone"</h2><p>"48 kHz · 1 channel · 12 ms input latency"</p></div><button class="hold-button">"Hold to test"</button></div><div class="waveform" aria-label="Audio level: quiet">{(0..42).map(|index| view! { <i style=format!("height:{}%", 14 + ((index * 17) % 62))></i> }).collect_view()}</div><div class="two-column"><SettingsGroup title="Input"><SettingRow label="Device" value="System default"/><SettingRow label="Analysis format" value="16 kHz mono"/></SettingsGroup><SettingsGroup title="Privacy"><SettingRow label="Ring buffer" value="3 seconds"/><SettingRow label="Audio files" value="Never automatic"/></SettingsGroup></div></div> }
}

#[component]
fn Diagnostics() -> impl IntoView {
    view! { <div class="page"><PageHeader eyebrow="Diagnostics" title="Make the invisible inspectable." body="Bounded, content-free operational data for understanding health and failures."/><div class="diagnostic-summary"><div><span class="health-ring large"><i></i></span><div><h2>"Healthy"</h2><p>"No subsystem requires attention."</p></div></div><button class="secondary-button">"Export support bundle"</button></div><div class="metric-grid"><MetricCard eyebrow="IPC p95" value="8" suffix=" ms" trend="healthy" meter=12/><MetricCard eyebrow="Dropped events" value="0" suffix="" trend="last hour" meter=0/><MetricCard eyebrow="Store p95" value="3" suffix=" ms" trend="healthy" meter=8/></div><div class="log-console"><div><span>"20:21:04"</span><strong>"capture"</strong><p>"adaptive rate settled at 0.8 fps"</p></div><div><span>"20:20:58"</span><strong>"privacy"</strong><p>"redaction rules applied (1 region)"</p></div><div><span>"20:20:55"</span><strong>"runtime"</strong><p>"all supervisors running"</p></div></div></div> }
}

#[component]
fn Settings() -> impl IntoView {
    view! { <div class="page"><PageHeader eyebrow="Settings" title="Your Mac, your boundaries." body="Every ambient capability remains visible, reversible, and independently configurable."/><PermissionSettings /></div> }
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
