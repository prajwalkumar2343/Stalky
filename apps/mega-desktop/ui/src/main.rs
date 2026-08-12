mod accessibility;
mod app;
mod components;
mod hud;
mod onboarding;
mod tauri;

fn main() {
    let is_hud = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| {
            query
                .split('&')
                .any(|part| part == "?view=hud" || part == "view=hud")
        });
    if is_hud {
        leptos::mount::mount_to_body(hud::Hud);
    } else {
        leptos::mount::mount_to_body(app::App);
    }
}
