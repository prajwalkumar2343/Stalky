use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::tauri::{
    AccountMode, GoogleAuthStatus, OnboardingState, PermissionCapability, PermissionState,
    PermissionStatuses, google_auth_start, google_auth_status, onboarding_complete,
    onboarding_set_account_mode, permission_open_settings, permission_request, permission_statuses,
};

#[derive(Clone, Copy)]
struct PermissionStepCopy {
    capability: PermissionCapability,
    eyebrow: &'static str,
    title: &'static str,
    body: &'static str,
    privacy: &'static str,
}

const PERMISSION_STEPS: [PermissionStepCopy; 4] = [
    PermissionStepCopy {
        capability: PermissionCapability::ScreenRecording,
        eyebrow: "01 · Screen Recording",
        title: "See only the display you choose.",
        body: "Screen Recording lets Stalky build an ephemeral, local context stream from the display or window you explicitly select.",
        privacy: "Frames stay in memory and are discarded after inspection.",
    },
    PermissionStepCopy {
        capability: PermissionCapability::Accessibility,
        eyebrow: "02 · Accessibility",
        title: "Understand interface structure.",
        body: "Accessibility lets Stalky read a bounded view of the focused interface and expose controls only when you choose them.",
        privacy: "No autonomous clicks, typing, or focus changes are performed.",
    },
    PermissionStepCopy {
        capability: PermissionCapability::Microphone,
        eyebrow: "03 · Microphone",
        title: "Keep audio local and deliberate.",
        body: "Microphone access is used only for an explicit local input test or a feature you start.",
        privacy: "No automatic recording, transcription, or upload is enabled.",
    },
    PermissionStepCopy {
        capability: PermissionCapability::LaunchAtLogin,
        eyebrow: "04 · Launch at login",
        title: "Choose whether Stalky starts with your Mac.",
        body: "This optional convenience is separate from capture and permission access.",
        privacy: "The current build leaves launch at login disabled until a maintained native binding is available.",
    },
];

#[component]
pub fn Onboarding(on_complete: Callback<OnboardingState>) -> impl IntoView {
    let (step, set_step) = signal(0_usize);
    let (account_mode, set_account_mode) = signal(None::<AccountMode>);
    let (permissions, set_permissions) = signal(PermissionStatuses::default());
    let (auth, set_auth) = signal(GoogleAuthStatus::default());
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);

    let refresh = move || {
        spawn_local(async move {
            if let Ok(status) = permission_statuses().await {
                set_permissions.set(status);
            }
            if let Ok(status) = google_auth_status().await {
                set_auth.set(status);
            }
        });
    };
    Effect::new(move |_| refresh());

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
                    refresh();
                }
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let complete = move || {
        let Some(mode) = account_mode.get_untracked() else {
            return;
        };
        set_busy.set(true);
        spawn_local(async move {
            match onboarding_complete(mode).await {
                Ok(state) => on_complete.run(state),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let next_step = move |_| {
        if busy.get_untracked() {
            return;
        }
        if step.get_untracked() >= PERMISSION_STEPS.len() {
            complete();
        } else {
            set_step.update(|current| *current += 1);
        }
    };

    let request_permission = move |_| {
        let current = PERMISSION_STEPS[step.get_untracked().saturating_sub(1)];
        if busy.get_untracked() || current.capability == PermissionCapability::LaunchAtLogin {
            return;
        }
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            match permission_request(current.capability).await {
                Ok(status) => set_permissions.set(status),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let open_settings = move |_| {
        let current = PERMISSION_STEPS[step.get_untracked().saturating_sub(1)];
        spawn_local(async move {
            if let Err(error) = permission_open_settings(current.capability).await {
                set_message.set(Some(error));
            }
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
                <span class:active=Signal::derive(move || step.get() == 0)></span>
                <span class:active=Signal::derive(move || step.get() > 0)></span>
                <span class="progress-label">{move || if step.get() == 0 { "Choose how to continue".to_owned() } else { format!("Permission {}/{}", step.get(), PERMISSION_STEPS.len()) }}</span>
            </div>
            {move || if step.get() == 0 {
                view! {
                    <section class="onboarding-card account-step" aria-labelledby="onboarding-title">
                        <div class="onboarding-kicker"><span class="signal-glyph" aria-hidden="true">"✦"</span><span>"A quieter way to begin"</span></div>
                        <h1 id="onboarding-title">"Your Mac stays yours."</h1>
                        <p class="onboarding-lede">"Stalky is local-first infrastructure for the context you choose to make visible. Start without an account, or connect Google through your system browser when you want identity portability."</p>
                        <div class="account-options" role="group" aria-label="Account choices">
                            <button class="account-option" disabled=move || busy.get() on:click=move |_| choose_account(AccountMode::Google)>
                                <span class="option-icon google-icon" aria-hidden="true">"G"</span>
                                <span><strong>"Continue with Google"</strong><small>"Native browser sign-in · PKCE · minimal scopes"</small></span>
                                <span class="option-arrow" aria-hidden="true">"↗"</span>
                            </button>
                            <button class="account-option" disabled=move || busy.get() on:click=move |_| choose_account(AccountMode::Local)>
                                <span class="option-icon local-icon" aria-hidden="true">"⌂"</span>
                                <span><strong>"Continue locally"</strong><small>"Full experience · no account · this Mac only"</small></span>
                                <span class="option-arrow" aria-hidden="true">"→"</span>
                            </button>
                        </div>
                        <p class="onboarding-footnote"><span class="status-mark" aria-hidden="true">"•"</span>"No permission is requested until you choose a capability."</p>
                        {move || auth.get().configured.then_some(view! { <span class="config-note">"Google OAuth is configured for this build."</span> })}
                    </section>
                }.into_any()
            } else {
                let current = PERMISSION_STEPS[step.get().saturating_sub(1).min(PERMISSION_STEPS.len() - 1)];
                let state = Signal::derive(move || permissions.get().state(current.capability));
                view! {
                    <section class="onboarding-card permission-step" aria-labelledby="onboarding-title">
                        <div class="permission-step-copy">
                            <span class="onboarding-eyebrow">{current.eyebrow}</span>
                            <h1 id="onboarding-title">{current.title}</h1>
                            <p class="onboarding-lede">{current.body}</p>
                            <div class="privacy-callout"><span aria-hidden="true">"◇"</span><p><strong>"Privacy boundary"</strong>{current.privacy}</p></div>
                        </div>
                        <div class="permission-step-action">
                            <div class="permission-status-large" class:granted=move || state.get().is_granted()>
                                <span class="status-symbol" aria-hidden="true">{move || if state.get().is_granted() { "✓" } else if state.get() == PermissionState::Unsupported { "—" } else { "○" }}</span>
                                <span><strong>{move || state.get().label()}</strong><small>{move || status_detail(state.get())}</small></span>
                            </div>
                            {move || if state.get().is_granted() {
                                view! { <div class="permission-action-row"><span class="inline-success">"Ready on this Mac"</span><button class="primary-button" disabled=move || busy.get() on:click=next_step>{if step.get() == PERMISSION_STEPS.len() { "Finish setup" } else { "Continue" }}</button></div> }.into_any()
                            } else if state.get() == PermissionState::Unsupported {
                                view! { <div class="permission-action-row"><span class="inline-note">"Optional in this build"</span><button class="primary-button" on:click=next_step>{if step.get() == PERMISSION_STEPS.len() { "Finish setup" } else { "Continue" }}</button></div> }.into_any()
                            } else if state.get().needs_settings() {
                                view! { <div class="permission-action-row"><button class="secondary-button" on:click=open_settings>"Open System Settings"</button><button class="primary-button" disabled=move || busy.get() on:click=next_step>"Skip for now"</button></div> }.into_any()
                            } else {
                                view! { <div class="permission-action-row"><button class="primary-button" disabled=move || busy.get() on:click=request_permission>{move || if busy.get() { "Waiting…" } else { "Continue to System Permission" }}</button><button class="text-button" disabled=move || busy.get() on:click=next_step>"Skip for now"</button></div> }.into_any()
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

fn status_detail(state: PermissionState) -> &'static str {
    match state {
        PermissionState::Requesting => "Waiting for the macOS permission sheet",
        PermissionState::Denied | PermissionState::Restricted | PermissionState::Revoked => {
            "System Settings is the recovery path"
        }
        PermissionState::Unsupported => "The optional native binding is not enabled",
        PermissionState::Unknown => "Status will be checked before any request",
        _ => "Nothing is active until you explicitly start it",
    }
}
