use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, closure::Closure};

use crate::components::{ARROW_UP_RIGHT, CHEVRON, CHIP, Glyph, SHIELD, SPARKLE};
use crate::tauri::{
    AccountMode, GoogleAuthStatus, OnboardingState, PermissionCapability, PermissionState,
    PermissionStatuses, google_auth_start, google_auth_status, onboarding_complete,
    onboarding_set_account_mode, permission_open_settings, permission_request, permission_statuses,
    permission_statuses_live, relaunch_app,
};

#[derive(Clone, Copy)]
struct PermissionStepCopy {
    capability: PermissionCapability,
    eyebrow: &'static str,
    title: &'static str,
    body: &'static str,
    privacy: &'static str,
}

// Screen Recording is deliberately requested last: granting it can require an
// app restart to take effect, so asking earlier would send the user back into
// System Settings mid-flow.
const PERMISSION_STEPS: [PermissionStepCopy; 3] = [
    PermissionStepCopy {
        capability: PermissionCapability::Accessibility,
        eyebrow: "01 · Accessibility",
        title: "Understand interface structure.",
        body: "Accessibility lets Stalky read a bounded view of the focused interface and expose controls only when you choose them.",
        privacy: "No autonomous clicks, typing, or focus changes are performed.",
    },
    PermissionStepCopy {
        capability: PermissionCapability::Microphone,
        eyebrow: "02 · Microphone",
        title: "Keep audio local and deliberate.",
        body: "Microphone access is used only for an explicit local input test or a feature you start.",
        privacy: "No automatic recording, transcription, or upload is enabled.",
    },
    PermissionStepCopy {
        capability: PermissionCapability::ScreenRecording,
        eyebrow: "03 · Screen Recording",
        title: "See only the display you choose.",
        body: "Screen Recording lets Stalky build an ephemeral, local context stream from the display or window you explicitly select.",
        privacy: "Frames stay in memory and are discarded after inspection.",
    },
];

fn step_capability(step: usize) -> PermissionCapability {
    PERMISSION_STEPS[step.saturating_sub(1).min(PERMISSION_STEPS.len() - 1)].capability
}

#[component]
pub fn Onboarding(on_complete: Callback<OnboardingState>) -> impl IntoView {
    let (step, set_step) = signal(0_usize);
    let (account_mode, set_account_mode) = signal(None::<AccountMode>);
    let (permissions, set_permissions) = signal(PermissionStatuses::default());
    let (auth, set_auth) = signal(GoogleAuthStatus::default());
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);

    // Bookkeeping for auto-advance. `last_step` and `granted_before` stay
    // untracked; `busy` and `advance_scheduled` are read tracked below so the
    // Effect re-evaluates when an in-flight operation ends or a pending timer
    // releases its scheduling slot.
    let (last_step, set_last_step) = signal(usize::MAX);
    let (granted_before, set_granted_before) = signal(false);
    let (advance_scheduled, set_advance_scheduled) = signal(false);
    // `AXIsProcessTrusted` caches its answer in-process, so once the user has
    // asked for Accessibility the status poll switches to the live tccd probe
    // that sees a grant made in System Settings without a relaunch.
    let (accessibility_requested, set_accessibility_requested) = signal(false);
    // Guards against overlapping poll ticks writing out-of-order results.
    let poll_in_flight = Arc::new(AtomicBool::new(false));
    let poll_again = Arc::new(AtomicBool::new(false));

    let refresh = Callback::new(move |_: ()| {
        let live = accessibility_requested.get_untracked();
        spawn_local(async move {
            let statuses = if live {
                permission_statuses_live().await
            } else {
                permission_statuses().await
            };
            if let Ok(status) = statuses {
                set_permissions.set(status);
            }
            if let Ok(status) = google_auth_status().await {
                set_auth.set(status);
            }
        });
    });

    // Initial state plus a live poller so granting a permission in System
    // Settings (or dismissing a sheet) is reflected here within a second.
    Effect::new(move |_| refresh.run(()));
    let poller_handle = AtomicI32::new(-1);
    if let Some(window) = web_sys::window() {
        let callback = Closure::wrap(Box::new(move || {
            if poll_in_flight.swap(true, Ordering::Relaxed) {
                poll_again.store(true, Ordering::Relaxed);
                return;
            }
            let in_flight = Arc::clone(&poll_in_flight);
            let again = Arc::clone(&poll_again);
            spawn_local(async move {
                loop {
                    again.store(false, Ordering::Relaxed);
                    let live = accessibility_requested.get_untracked();
                    let statuses = if live {
                        permission_statuses_live().await
                    } else {
                        permission_statuses().await
                    };
                    if let Ok(status) = statuses {
                        set_permissions.set(status);
                    }
                    if let Ok(status) = google_auth_status().await {
                        set_auth.set(status);
                    }
                    if !again.swap(false, Ordering::Relaxed) {
                        break;
                    }
                }
                in_flight.store(false, Ordering::Relaxed);
            });
        }) as Box<dyn FnMut()>)
        .into_js_value();
        if let Ok(id) = window
            .set_interval_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 1_000)
        {
            poller_handle.store(id, Ordering::Relaxed);
        }
        on_cleanup(move || {
            let id = poller_handle.swap(-1, Ordering::Relaxed);
            if id >= 0 {
                window.clear_interval_with_handle(id);
            }
            drop(callback);
        });
    }

    // Refreshing the moment the window regains focus covers the return trip
    // from System Settings, where the 1 s poller might be mid-cycle.
    if let Some(window) = web_sys::window() {
        let refresh_on_focus = refresh;
        let callback =
            Closure::wrap(Box::new(move || refresh_on_focus.run(())) as Box<dyn FnMut()>)
                .into_js_value();
        let _ = window.add_event_listener_with_callback("focus", callback.unchecked_ref());
        on_cleanup(move || {
            let _ = window.remove_event_listener_with_callback("focus", callback.unchecked_ref());
        });
    }

    // When the current step's permission flips to granted, move on shortly
    // afterwards. Manual Continue is still available on every step.
    Effect::new(move |_| {
        let step_value = step.get();
        if step_value != 1 {
            set_accessibility_requested.set(false);
        }
        if step_value != last_step.get_untracked() {
            set_last_step.update_untracked(|value| *value = step_value);
            set_granted_before.update_untracked(|value| *value = false);
        }
        let granted = step_value > 0
            && permissions
                .get()
                .state(step_capability(step_value))
                .is_granted();
        let before = granted_before.get_untracked();
        let scheduled = advance_scheduled.get();
        let busy_now = busy.get();
        match observe_advance(step_value, granted, before, scheduled, busy_now) {
            AdvanceObservation::Schedule => {
                set_granted_before.update_untracked(|value| *value = true);
                set_advance_scheduled.set(true);
                let timeout = Timeout::new(950, move || {
                    let should_advance =
                        step.get_untracked() == step_value && !busy.get_untracked();
                    if !should_advance {
                        // Clear the latch before releasing the tracked scheduling
                        // slot so the retry is correct even if effects flush
                        // synchronously.
                        set_granted_before.update_untracked(|value| *value = false);
                    }
                    // Tracked clear so the Effect re-evaluates even when the
                    // timer does not advance (stale or busy).
                    set_advance_scheduled.set(false);
                    if should_advance {
                        set_step.update(|current| *current += 1);
                        set_message.set(None);
                        refresh.run(());
                    }
                });
                timeout.forget();
            }
            AdvanceObservation::BlockedByBusy | AdvanceObservation::TimerPending => {
                // Not final: keep the transition unlatched so a later run
                // (busy clearing, or the pending timer firing) can still
                // schedule the advance.
                set_granted_before.update_untracked(|value| *value = false);
            }
            AdvanceObservation::Settle => {
                set_granted_before.update_untracked(|value| *value = granted);
            }
        }
    });

    let complete = move || {
        let Some(mode) = account_mode.get_untracked() else {
            return;
        };
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            match onboarding_complete(mode).await {
                Ok(state) => on_complete.run(state),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let advance = move |_| {
        if busy.get_untracked() {
            return;
        }
        if step.get_untracked() >= PERMISSION_STEPS.len() {
            complete();
        } else {
            set_step.update(|current| *current += 1);
            set_message.set(None);
            refresh.run(());
        }
    };

    let go_back = move |_| {
        if busy.get_untracked() {
            return;
        }
        set_step.update(|current| *current = current.saturating_sub(1));
        set_message.set(None);
        refresh.run(());
    };

    let choose_account = move |mode: AccountMode| {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            let result = match mode {
                AccountMode::Local => onboarding_set_account_mode(mode).await.map(|_| ()),
                AccountMode::Google => google_auth_start().await.map(|status| {
                    set_auth.set(status);
                }),
            };
            match result {
                Ok(()) => {
                    set_account_mode.set(Some(mode));
                    set_step.set(1);
                    refresh.run(());
                }
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let open_settings = move |_| {
        let capability = step_capability(step.get_untracked());
        spawn_local(async move {
            if let Err(error) = permission_open_settings(capability).await {
                set_message.set(Some(error));
            }
        });
    };

    let request_permission = move |_| {
        if busy.get_untracked() {
            return;
        }
        let step_value = step.get_untracked();
        let current =
            PERMISSION_STEPS[step_value.saturating_sub(1).min(PERMISSION_STEPS.len() - 1)];
        if current.capability == PermissionCapability::LaunchAtLogin {
            advance(());
            return;
        }
        let state = permissions.get_untracked().state(current.capability);
        if state.is_granted() || state == PermissionState::Unsupported {
            advance(());
            return;
        }
        if state.needs_settings() {
            open_settings(());
            return;
        }
        if current.capability == PermissionCapability::Accessibility {
            set_accessibility_requested.set(true);
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            match permission_request(current.capability).await {
                Ok(status) => set_permissions.set(status),
                Err(error) => {
                    set_message.set(Some(error.clone()));
                    // A previous denial cannot be re-prompted; send the user
                    // to System Settings instead of leaving them stuck.
                    if error.contains("System Settings") {
                        let _ = permission_open_settings(current.capability).await;
                    }
                }
            }
            set_busy.set(false);
            refresh.run(());
        });
    };

    let restart = move |_| {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            if let Err(error) = relaunch_app().await {
                set_message.set(Some(error));
            }
            set_busy.set(false);
        });
    };

    view! {
        <main class="onboarding-shell" role="dialog" aria-modal="true" aria-labelledby="onboarding-title" tabindex="-1">
            <div class="onboarding-glow" aria-hidden="true"></div>
            <header class="onboarding-brand">
                <span class="stalky-mark" aria-hidden="true"><i></i><i></i></span>
                <strong>"Stalky"</strong>
                <span>"Private context infrastructure"</span>
            </header>
            <div class="onboarding-progress" aria-label="Onboarding progress">
                {move || (0..=PERMISSION_STEPS.len()).map(|index| {
                    let active = Signal::derive(move || step.get() >= index);
                    view! { <span class:active=active></span> }
                }).collect_view()}
                <span class="progress-label">{move || progress_label(step.get())}</span>
            </div>
            {move || if step.get() == 0 {
                view! {
                    <section class="onboarding-card account-step" aria-labelledby="onboarding-title">
                        <div class="onboarding-kicker"><span class="signal-glyph" aria-hidden="true"><Glyph paths=SPARKLE /></span><span>"A quieter way to begin"</span></div>
                        <h1 id="onboarding-title">"Your Mac stays yours."</h1>
                        <p class="onboarding-lede">"Stalky is local-first infrastructure for the context you choose to make visible. Start without an account, or connect Google through your system browser when you want identity portability."</p>
                        <div class="account-options" role="group" aria-label="Account choices">
                            <button class="account-option" disabled=move || busy.get() on:click=move |_| choose_account(AccountMode::Google)>
                                <span class="option-icon google-icon" aria-hidden="true">"G"</span>
                                <span><strong>"Continue with Google"</strong><small>"Native browser sign-in · PKCE · minimal scopes"</small></span>
                                <span class="option-arrow" aria-hidden="true"><Glyph paths=ARROW_UP_RIGHT /></span>
                            </button>
                            <button class="account-option" disabled=move || busy.get() on:click=move |_| choose_account(AccountMode::Local)>
                                <span class="option-icon local-icon" aria-hidden="true"><Glyph paths=CHIP /></span>
                                <span><strong>"Continue locally"</strong><small>"Full experience · no account · this Mac only"</small></span>
                                <span class="option-arrow" aria-hidden="true"><Glyph paths=CHEVRON /></span>
                            </button>
                        </div>
                        <p class="onboarding-footnote"><span class="status-mark" aria-hidden="true">"•"</span>"No permission is requested until you choose a capability."</p>
                        {move || auth.get().configured.then_some(view! { <span class="config-note">"Google OAuth is configured for this build."</span> })}
                    </section>
                }.into_any()
            } else {
                let current = PERMISSION_STEPS[step.get().saturating_sub(1).min(PERMISSION_STEPS.len() - 1)];
                let state = Signal::derive(move || permissions.get().state(current.capability));
                let is_last = step.get() == PERMISSION_STEPS.len();
                view! {
                    <section class="onboarding-card permission-step" aria-labelledby="onboarding-title">
                        <div class="permission-step-copy">
                            <span class="onboarding-eyebrow">{current.eyebrow}</span>
                            <h1 id="onboarding-title">{current.title}</h1>
                            <p class="onboarding-lede">{current.body}</p>
                            <div class="privacy-callout"><Glyph paths=SHIELD /><p><strong>"Privacy boundary"</strong>{current.privacy}</p></div>
                        </div>
                        <div class="permission-step-action">
                            <div class="permission-step-nav">
                                <button class="text-button" disabled=move || busy.get() on:click=go_back>"← Back"</button>
                                <span class="live-check" aria-hidden="true"><i></i>"Checking automatically"</span>
                            </div>
                            <div class="permission-status-large" class:granted=move || state.get().is_granted() class:waiting=move || busy.get()>
                                <span class="status-symbol" aria-hidden="true">{move || status_symbol(state.get(), busy.get())}</span>
                                <span><strong>{move || state.get().label()}</strong><small>{move || status_detail(state.get(), busy.get())}</small></span>
                            </div>
                            {move || if state.get().is_granted() {
                                view! { <div class="permission-action-row"><span class="inline-success">"Ready on this Mac"</span><button class="primary-button" disabled=move || busy.get() on:click=move |_| advance(())>{if is_last { "Finish setup" } else { "Continue" }}</button></div> }.into_any()
                            } else if state.get() == PermissionState::RestartRequired {
                                view! { <div class="permission-action-row"><span class="inline-note restart-note">"Restart to apply the grant"</span><button class="primary-button" disabled=move || busy.get() on:click=restart>{move || if busy.get() { "Restarting…" } else { "Restart Stalky" }}</button><button class="text-button" disabled=move || busy.get() on:click=move |_| advance(())>"Later"</button></div> }.into_any()
                            } else if state.get() == PermissionState::Unsupported {
                                view! { <div class="permission-action-row"><span class="inline-note">"Optional in this build"</span><button class="primary-button" on:click=move |_| advance(())>{if is_last { "Finish setup" } else { "Continue" }}</button></div> }.into_any()
                            } else if state.get().needs_settings() {
                                view! { <div class="permission-action-row"><button class="primary-button" on:click=move |_| open_settings(())>"Open System Settings"</button><button class="text-button" disabled=move || busy.get() on:click=move |_| advance(())>"Skip for now"</button></div> }.into_any()
                            } else {
                                view! { <div class="permission-action-row"><button class="primary-button" disabled=move || busy.get() on:click=request_permission>{move || if busy.get() { "Waiting…" } else { "Request access" }}</button><button class="text-button" disabled=move || busy.get() on:click=move |_| advance(())>"Skip for now"</button></div> }.into_any()
                            }}
                            <p class="onboarding-footnote step-footnote"><span class="status-mark" aria-hidden="true">"•"</span>"You can change this any time in Settings."</p>
                        </div>
                    </section>
                }.into_any()
            }}
            <div class="onboarding-message" aria-live="polite">{move || message.get()}</div>
            <footer class="onboarding-footer"><span>"Local by default · optional by design"</span><span>{move || if auth.get().signed_in { "Google connected" } else { "No account required" }}</span></footer>
        </main>
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvanceObservation {
    /// Create the 950 ms advance timer and latch the transition.
    Schedule,
    /// Granted transition detected, but an in-flight operation blocks
    /// scheduling: keep the transition unlatched so it re-fires on retry.
    BlockedByBusy,
    /// A timer from an earlier observation is still pending: keep the
    /// transition unlatched so the timer's tracked clear re-triggers
    /// scheduling for the current step.
    TimerPending,
    /// Nothing to schedule now; latch the transition to the granted state.
    Settle,
}

/// Pure decision for the auto-advance timer. `granted_before` tracks the
/// not-granted → granted transition across Effect runs and must only be
/// latched once the outcome is final; otherwise a busy poll write or a stale
/// timer would swallow the transition and the step would never auto-advance.
fn observe_advance(
    step: usize,
    granted: bool,
    granted_before: bool,
    advance_scheduled: bool,
    busy: bool,
) -> AdvanceObservation {
    if granted && !granted_before && !advance_scheduled && step > 0 && step < PERMISSION_STEPS.len()
    {
        if busy {
            AdvanceObservation::BlockedByBusy
        } else {
            AdvanceObservation::Schedule
        }
    } else if granted && !granted_before && advance_scheduled {
        AdvanceObservation::TimerPending
    } else {
        AdvanceObservation::Settle
    }
}

fn progress_label(step: usize) -> String {
    if step == 0 {
        "Choose how to continue".to_owned()
    } else {
        PERMISSION_STEPS[step.saturating_sub(1).min(PERMISSION_STEPS.len() - 1)]
            .eyebrow
            .to_owned()
    }
}

fn status_symbol(state: PermissionState, busy: bool) -> &'static str {
    if busy || state == PermissionState::Requesting {
        "…"
    } else if state.is_granted() {
        "✓"
    } else if state == PermissionState::Unsupported {
        "—"
    } else if state == PermissionState::RestartRequired {
        "↻"
    } else if state.needs_settings() {
        "✕"
    } else {
        "○"
    }
}

fn status_detail(state: PermissionState, busy: bool) -> &'static str {
    if busy || state == PermissionState::Requesting {
        "Waiting for the macOS permission sheet"
    } else {
        match state {
            PermissionState::Granted => "Nothing is active until you explicitly start it",
            PermissionState::RestartRequired => {
                "The grant is recorded — restart Stalky to apply it"
            }
            PermissionState::Denied | PermissionState::Restricted | PermissionState::Revoked => {
                "Grant it in System Settings to continue"
            }
            PermissionState::Unsupported => "Optional — no permission needed in this build",
            PermissionState::NotRequested => "Stalky will ask macOS when you continue",
            PermissionState::Unknown => "Checking the current OS status…",
            PermissionState::Requesting => "Waiting for the macOS permission sheet",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_on_grant_transition() {
        assert_eq!(
            observe_advance(1, true, false, false, false),
            AdvanceObservation::Schedule
        );
    }

    #[test]
    fn keeps_transition_open_while_busy() {
        assert_eq!(
            observe_advance(1, true, false, false, true),
            AdvanceObservation::BlockedByBusy
        );
    }

    #[test]
    fn keeps_transition_open_while_old_timer_pending() {
        assert_eq!(
            observe_advance(2, true, false, true, false),
            AdvanceObservation::TimerPending
        );
        assert_eq!(
            observe_advance(2, true, false, true, true),
            AdvanceObservation::TimerPending
        );
    }

    #[test]
    fn does_not_reschedule_after_scheduling() {
        assert_eq!(
            observe_advance(1, true, true, true, false),
            AdvanceObservation::Settle
        );
        assert_eq!(
            observe_advance(1, true, true, false, false),
            AdvanceObservation::Settle
        );
    }

    #[test]
    fn nothing_to_do_when_not_granted() {
        assert_eq!(
            observe_advance(1, false, false, false, false),
            AdvanceObservation::Settle
        );
        assert_eq!(
            observe_advance(1, false, true, false, false),
            AdvanceObservation::Settle
        );
    }

    #[test]
    fn never_schedules_past_the_last_step() {
        assert_eq!(
            observe_advance(PERMISSION_STEPS.len(), true, false, false, false),
            AdvanceObservation::Settle
        );
        assert_eq!(
            observe_advance(0, true, false, false, false),
            AdvanceObservation::Settle
        );
    }
}
