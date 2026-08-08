use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Quiet,
    Warning,
}

impl Tone {
    fn class_name(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Quiet => "quiet",
            Self::Warning => "warning",
        }
    }
}

#[component]
pub fn StatusRow(
    label: &'static str,
    detail: &'static str,
    value: &'static str,
    tone: Tone,
) -> impl IntoView {
    view! {
        <div class="status-row">
            <span class=format!("status-dot {}", tone.class_name()) aria-hidden="true"></span>
            <div class="status-copy">
                <strong>{label}</strong>
                <span>{detail}</span>
            </div>
            <span class=format!("status-value {}", tone.class_name())>{value}</span>
        </div>
    }
}

#[component]
pub fn MetricCard(
    eyebrow: &'static str,
    value: &'static str,
    suffix: &'static str,
    trend: &'static str,
    meter: u8,
) -> impl IntoView {
    let meter = meter.min(100);
    view! {
        <article class="metric-card">
            <div class="metric-topline">
                <span>{eyebrow}</span>
                <span class="metric-trend">{trend}</span>
            </div>
            <div class="metric-value">{value}<small>{suffix}</small></div>
            <div class="meter-track" aria-hidden="true">
                <span style=format!("width: {meter}%")></span>
            </div>
        </article>
    }
}

#[component]
pub fn EventRow(
    time: &'static str,
    source: &'static str,
    title: &'static str,
    detail: &'static str,
    tone: Tone,
) -> impl IntoView {
    view! {
        <div class="event-row">
            <span class="event-time">{time}</span>
            <span class=format!("event-icon {}", tone.class_name()) aria-hidden="true"></span>
            <div class="event-copy">
                <div><strong>{title}</strong><span>{source}</span></div>
                <p>{detail}</p>
            </div>
            <button class="icon-button small" aria-label=format!("Inspect {title}") title="Inspect event">"↗"</button>
        </div>
    }
}

#[component]
pub fn PermissionCard(
    number: &'static str,
    title: &'static str,
    body: &'static str,
    granted: bool,
) -> impl IntoView {
    view! {
        <article class="permission-card">
            <span class="permission-number">{number}</span>
            <div>
                <h3>{title}</h3>
                <p>{body}</p>
            </div>
            <span class="permission-state" class:granted=granted>
                {if granted { "Granted" } else { "Not requested" }}
            </span>
        </article>
    }
}

#[component]
pub fn InspectorField(label: &'static str, value: &'static str) -> impl IntoView {
    view! {
        <div class="inspector-field">
            <span>{label}</span>
            <strong>{value}</strong>
        </div>
    }
}
