use leptos::prelude::*;

/// Renders a 24×24 stroke icon from one or more `<path>` definitions.
/// Paths inherit `currentColor`, so the surrounding CSS controls the tone.
#[component]
pub fn Glyph(paths: &'static [&'static str]) -> impl IntoView {
    view! {
        <svg
            class="glyph"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            {paths.iter().map(|&path| view! { <path d=path></path> }).collect_view()}
        </svg>
    }
}

pub const HOME: &[&str] = &["M3.8 11.3 12 4.5l8.2 6.8", "M6.2 9.9V19.5h11.6V9.9"];
pub const MONITOR: &[&str] = &["M4 5.5h16v10.5H4z", "M9 19.5h6", "M12 16v3.5"];
pub const TARGET: &[&str] = &[
    "M12 12m-8 0a8 8 0 1 0 16 0a8 8 0 1 0-16 0",
    "M12 12m-3 0a3 3 0 1 0 6 0a3 3 0 1 0-6 0",
];
pub const PULSE: &[&str] = &["M2.5 12h4.5l2.5-6.5L13 18.5l2.5-6.5H21.5"];
pub const SLIDERS: &[&str] = &[
    "M4 6.5h16",
    "M4 12h16",
    "M4 17.5h16",
    "M9.5 4.5v4",
    "M14.5 10v4",
    "M9.5 15.5v4",
];
pub const PLAY: &[&str] = &["M8.2 5.4v13.2L18.2 12 8.2 5.4z"];
pub const PAUSE: &[&str] = &["M7.8 5.6h3v12.8h-3z", "M13.2 5.6h3v12.8h-3z"];
pub const CHEVRON: &[&str] = &["M9.3 5.8l6.4 6.2-6.4 6.2"];
pub const ARROW_UP_RIGHT: &[&str] = &["M7.5 16.5L16.5 7.5", "M9 7.5h7.5V15"];
pub const SPARKLE: &[&str] =
    &["M12 3.6l1.8 6.3 6.3 1.8-6.3 1.8L12 19.8l-1.8-6.3L3.9 11.7l6.3-1.8L12 3.6z"];
pub const SHIELD: &[&str] = &["M12 3.6l7 2.7v5.4c0 4.7-3 7.7-7 9.3-4-1.6-7-4.6-7-9.3V6.3l7-2.7z"];
pub const SEARCH: &[&str] = &["M10.5 4.5a6 6 0 1 1 0 12 6 6 0 0 1 0-12z", "M15 15l4.5 4.5"];
pub const CHIP: &[&str] = &[
    "M5 5h14v14H5z",
    "M9 5v14",
    "M15 5v14",
    "M5 9h14",
    "M5 15h14",
];
pub const DOT: &[&str] = &["M12 12m-1.4 0a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0-2.8 0"];
