mod accessibility;
mod app;
mod components;
mod permissions;
mod tauri;

fn main() {
    leptos::mount::mount_to_body(app::App);
}
