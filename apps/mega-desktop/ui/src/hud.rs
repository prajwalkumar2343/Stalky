use gloo_timers::callback::Interval;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::glass::GlassSurface;
use crate::tauri::{
    CaptureState, CaptureStatus, OnboardingState, capture_start, capture_status, capture_stop,
    hud_open_main, is_available, onboarding_state,
};

#[component]
pub fn Hud() -> impl IntoView {
    let desktop_available = is_available();
    let (action_busy, set_action_busy) = signal(false);
    let (capture, set_capture) = signal(CaptureStatus::default());
    let (onboarding, set_onboarding) = signal(OnboardingState {
        completed: !desktop_available,
        account_mode: None,
    });
    let (message, set_message) = signal(None::<String>);

    let refresh = Callback::new(move |_: ()| {
        if !desktop_available {
            return;
        }
        spawn_local(async move {
            if let Ok(status) = capture_status().await {
                set_capture.set(status);
            }
            if let Ok(state) = onboarding_state().await {
                set_onboarding.set(state);
            }
        });
    });

    Effect::new(move |_| refresh.run(()));
    let poller = Interval::new(1_200, move || refresh.run(()));
    poller.forget();

    let open_main = Callback::new(move |_: ()| {
        if !desktop_available {
            return;
        }
        spawn_local(async move {
            if let Err(error) = hud_open_main().await {
                set_message.set(Some(error));
            }
        });
    });

    let primary_action = move |_| {
        if action_busy.get_untracked() || !desktop_available {
            return;
        }
        if !onboarding.get_untracked().completed {
            open_main.run(());
            return;
        }
        let state = capture.get_untracked().state;
        if matches!(state, CaptureState::Starting | CaptureState::Stopping) {
            return;
        }
        set_action_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            let result = match state {
                CaptureState::Running | CaptureState::Failed => capture_stop().await,
                CaptureState::Stopped => capture_start().await,
                CaptureState::Starting | CaptureState::Stopping => return,
            };
            match result {
                Ok(status) => set_capture.set(status),
                Err(error) => set_message.set(Some(error)),
            }
            set_action_busy.set(false);
        });
    };

    view! {
        <main
            class="glance-root"
            class:running=move || capture.get().is_running()
            class:attention=move || capture.get().state == CaptureState::Failed || message.get().is_some()
            role="status"
            aria-live="polite"
        >
            <div class="glance-pill" data-tauri-drag-region="true">
                <GlassSurface />
                <button class="glance-open" aria-label="Open Stalky" on:click=move |_| open_main.run(())>
                    <span class="glance-mark" aria-hidden="true"><StalkyGlyph /></span>
                    <span class="glance-copy">
                        <strong>{move || state_title(capture.get(), onboarding.get())}</strong>
                        <small>{move || message.get().unwrap_or_else(|| state_detail(capture.get(), onboarding.get()))}</small>
                    </span>
                </button>
                <button
                    class="glance-action"
                    class:stop=move || capture.get().is_running()
                    disabled=move || action_busy.get() || !desktop_available || matches!(capture.get().state, CaptureState::Starting | CaptureState::Stopping)
                    on:click=primary_action
                >
                    {move || action_label(capture.get(), onboarding.get(), action_busy.get())}
                </button>
            </div>
        </main>
    }
}

#[component]
fn StalkyGlyph() -> impl IntoView {
    view! {
        <svg class="stalky-glyph" viewBox="0 0 32 32" aria-hidden="true">
            <circle cx="13.5" cy="13" r="7" fill="none" stroke="currentColor" stroke-width="2.3" />
            <circle cx="19" cy="18.5" r="6.5" fill="none" stroke="currentColor" stroke-width="2" opacity=".5" />
            <path d="M8.5 19c2.2 4 6.2 6.2 10.6 6.2 1.5 0 2.9-.2 4.2-.7" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" />
        </svg>
    }
}

fn state_title(capture: CaptureStatus, onboarding: OnboardingState) -> &'static str {
    if !onboarding.completed {
        return "Finish setup";
    }
    match capture.state {
        CaptureState::Running => "Capture is on",
        CaptureState::Starting => "Starting capture…",
        CaptureState::Stopping => "Stopping capture…",
        CaptureState::Failed => "Capture stopped",
        CaptureState::Stopped => "Capture is off",
    }
}

fn state_detail(capture: CaptureStatus, onboarding: OnboardingState) -> String {
    if !onboarding.completed {
        return "Open Stalky to choose permissions.".to_owned();
    }
    if let Some(error) = capture.last_error {
        return error;
    }
    match capture.state {
        CaptureState::Running => "Local · memory only".to_owned(),
        CaptureState::Starting => "Preparing the primary display".to_owned(),
        CaptureState::Stopping => "Releasing the capture session".to_owned(),
        CaptureState::Failed => "Open Stalky for details".to_owned(),
        CaptureState::Stopped => "No screen content is captured".to_owned(),
    }
}

fn action_label(capture: CaptureStatus, onboarding: OnboardingState, busy: bool) -> &'static str {
    if busy {
        return "Working…";
    }
    if !onboarding.completed {
        return "Continue";
    }
    match capture.state {
        CaptureState::Running => "Stop",
        CaptureState::Starting => "Starting…",
        CaptureState::Stopping => "Stopping…",
        CaptureState::Failed => "Reset",
        CaptureState::Stopped => "Start",
    }
}
