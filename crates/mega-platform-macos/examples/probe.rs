use mega_platform_macos::diagnostics::ProbeDiagnostics;
use std::io::Write;

fn main() {
    let d = ProbeDiagnostics::capture();
    let report = format!(
        "=== Stalky identity probes ({}) ===\n\
         ax_is_process_trusted:       {}\n\
         event_tap_probe:             {}\n\
         cg_preflight_screen:         {}\n\
         sck enumeration probe:       {}\n\
         screen state (derived):      {:?}\n\
         accessibility live (derived): {:?}\n\
         microphone state:            {:?}\n",
        std::env::args().next().unwrap_or_default(),
        d.ax_is_process_trusted,
        d.event_tap_probe,
        d.cg_preflight_screen,
        d.sck_probe,
        d.screen_state,
        d.accessibility_live_state,
        d.microphone_state,
    );
    let path = std::path::Path::new("/tmp/stalky_probe.txt");
    if let Ok(mut file) = std::fs::File::create(path) {
        let _ = file.write_all(report.as_bytes());
    }
    println!("{report}");
}
