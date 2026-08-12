# Stalky

Stalky is a Rust-first macOS infrastructure prototype for private ambient screen, interface, and microphone context.

The current milestone deliberately stops before AI behavior. It contains a Codex-inspired Leptos desktop interface, a Tauri shell, typed infrastructure contracts, continuous display capture, and live Accessibility observation with explicit user-initiated AX controls. It does not include model providers, prompts, agent loops, autonomous UI operation, synthesized keyboard/mouse input, transcription, or remote content upload.

Continuous display capture is opt-in: it starts only when the user presses **Start capture**. The macOS adapter uses ScreenCaptureKit at approximately one frame per second, excludes Stalky from the selected display when application metadata is available, and keeps only the newest bounded BGRA frame in memory. Raw frame bytes are never written to disk or returned through Tauri IPC; the interface receives capture state and content-free counters only.

Screen capture requires Screen Recording access in **System Settings → Privacy & Security → Screen & System Audio Recording**. Stalky checks the current authorization state but does not open a permission prompt during startup or tests.

Accessibility observation is also opt-in. Stalky keeps a bounded, redacted snapshot and recent event list in memory. Controls are restricted to actions the selected element currently advertises; targets are revalidated immediately before execution, stale element IDs fail closed, and permission prompting occurs only from the dedicated user action.

## Stalky glance

The macOS build includes a small always-on-top companion called **Stalky glance**. It stays as a draggable 74-point glass orb and expands into a compact status panel showing capture state, accepted frames, permission readiness, and latest frame dimensions. The panel can start or pause capture and reopen the full workspace. Its top-right screen anchor is stored locally and restored across launches.

The companion window uses Tauri's native macOS HUD material with a transparent, undecorated window. This requires Tauri's `macos-private-api` feature: signed and notarized direct distribution is supported, but the resulting build is not eligible for the Mac App Store.

## Development

Prerequisites:

- Rust stable with the `wasm32-unknown-unknown` target
- Trunk
- Xcode Command Line Tools

Build the Rust/WASM interface:

```sh
cd apps/mega-desktop/ui
trunk build
```

Run it in a browser during UI development:

```sh
cd apps/mega-desktop/ui
trunk serve
```

Run workspace checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The complete product and infrastructure specification is in [STALKY_APP_PLAN.md](STALKY_APP_PLAN.md).
