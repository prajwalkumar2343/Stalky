# Stalky macOS infrastructure plan

Status: active infrastructure implementation. AI orchestration, agent autonomy, model providers, prompts, and synthesized keyboard/mouse input remain deferred. Explicit user-initiated controls through supported macOS Accessibility actions are in scope.

## 1. Current milestone

Build a stable Rust-first macOS foundation for a future ambient assistant. The milestone delivers:

- A polished desktop shell with a visual structure inspired by Codex.
- ScreenCaptureKit screen acquisition with adaptive sampling and privacy controls.
- Live macOS Accessibility observation, normalized UI snapshots, and narrowly gated user-initiated AX controls.
- Microphone capture, metering, buffering, and local voice-activity detection.
- Permission onboarding and recovery for Screen Recording, Accessibility, Microphone, and Launch at Login.
- Durable settings, bounded diagnostics, lifecycle supervision, crash recovery, signing, notarization, and automated QA.

This milestone does **not** call an AI model, interpret prompts, autonomously operate applications, synthesize keyboard/mouse input, run tools, maintain conversational memory, or perform automations. Accessibility controls execute only from an explicit UI action and only when the target still advertises the requested AX capability.

## 2. Definition of done

The milestone is complete when a signed/notarized Stalky build can run for a four-hour soak test, visibly and reliably report what it is capturing, survive permission changes and sleep/wake transitions, expose normalized screen/Accessibility/audio events to a future consumer, and meet the visual and accessibility quality gates in this plan.

The infrastructure output is an internal event stream and diagnostics UI—not an assistant response.

## 3. Product principles

- **Quiet utility:** the UI stays out of the way and makes system state legible.
- **Visible capture:** no invisible screen or microphone activity.
- **Local by default:** raw frames and audio remain in process memory and are not retained.
- **Explicit control:** Accessibility observation is continuous only while enabled; controls are allowlisted, revalidated, visible, and user initiated.
- **Backpressure everywhere:** slow consumers cannot cause unbounded frame, AX, audio, log, or IPC queues.
- **Recoverable states:** permission denial, device changes, display changes, process crashes, and sleep/wake are normal state transitions.
- **One source of truth:** the Rust runtime owns state; UI surfaces render typed projections of it.
- **Measurable polish:** layout, keyboard access, VoiceOver, latency, memory, CPU, and failure recovery have acceptance gates.

## 4. Recommended stack

### Selected direction: Tauri 2 + Leptos + Rust workspace

- **Tauri 2:** application bundle, WebView host, windows, menu bar, global shortcuts, typed command/event bridge, updater integration, and distribution tooling.
- **Leptos/WASM:** Rust-authored UI state and components.
- **CSS:** design tokens, responsive layout, motion, macOS material treatment, and visual themes.
- **Tokio:** supervised asynchronous runtime for non-real-time work.
- **ScreenCaptureKit:** screen and window frame delivery.
- **ApplicationServices/AXUIElement:** read-only focused-app/window/UI-element observation.
- **CoreAudio or `cpal`:** microphone device enumeration and PCM capture; final choice comes from the phase-zero latency/device-change spike.
- **SQLite:** settings migrations, bounded operational history, and diagnostics metadata.
- **macOS Keychain:** local encryption key and future credentials; no secrets in preferences or logs.
- **ServiceManagement:** user-controlled Launch at Login support.

Tauri officially supports Rust application logic and Rust frontends such as Leptos. Its capability files also provide a useful first IPC boundary between the local UI and privileged Rust commands.

### Native bridge policy

Use maintained Rust bindings first. A small Swift/Objective-C bridge is permitted only when the phase-zero spike demonstrates a concrete binding, callback, permission, or panel integration gap.

Any bridge must:

- Be inside the signed app bundle.
- Contain platform adaptation only, with no product state or capture policy.
- Expose a versioned C ABI or Tauri plugin contract.
- Have deterministic fixtures and integration tests.
- Be documented through an architecture decision record.

## 5. Codex-inspired design direction

Stalky should feel related to Codex’s calm desktop-product grammar without copying OpenAI branding, icons, names, proprietary assets, or exact layouts.

### Layout grammar

```text
┌────────────────────────────────────────────────────────────────────┐
│ integrated title bar · workspace title · global status · controls │
├───────────────┬──────────────────────────────────┬─────────────────┤
│ left sidebar  │ central workspace                │ inspector       │
│ 232–264 px    │ flexible                         │ 288–336 px      │
│               │                                  │ optional        │
│ Overview      │ capture/audio/permission view    │ event details   │
│ Capture       │                                  │ metadata        │
│ Accessibility│                                  │ diagnostics     │
│ Audio         │                                  │                 │
│ Diagnostics   │                                  │                 │
│ Settings      │                                  │                 │
├───────────────┴──────────────────────────────────┴─────────────────┤
│ compact control dock: pause · snapshot · mic test · health         │
└────────────────────────────────────────────────────────────────────┘
```

### Window behavior

- Default size: 1,240 × 800 points.
- Minimum size: 960 × 640 points.
- Sidebar remains fixed within 232–264 points; inspector collapses below 1,080 points.
- Main content width is capped for forms and prose but fluid for timelines and previews.
- The title bar is integrated with content while preserving standard traffic-light placement and draggable regions.
- Restore last window size, position, sidebar state, inspector state, and selected section.
- Respect multiple displays, Spaces, fullscreen apps, and macOS safe areas.

### Visual tokens

The exact values are starting points and must be calibrated through screenshots on real displays.

| Token | Light | Dark | Purpose |
|---|---|---|---|
| `canvas` | `#F7F7F5` | `#171717` | App background |
| `surface` | `#FFFFFF` | `#202020` | Cards and raised regions |
| `surface-muted` | `#EFEFEC` | `#292929` | Sidebar selection and wells |
| `text-primary` | `#171717` | `#F2F2F0` | Primary content |
| `text-secondary` | `#676762` | `#A6A6A0` | Metadata |
| `border` | `#DEDED9` | `#353535` | Dividers and outlines |
| `focus` | `#5B7CFA` | `#8CA2FF` | Keyboard focus only |
| `success` | `#2F8A57` | `#58B77A` | Healthy state |
| `warning` | `#A86D16` | `#D79A42` | Degraded state |
| `danger` | `#B6403A` | `#E07168` | Error or stopped state |

- Use system font metrics for controls and body text.
- Default body: 13 px/18 px; metadata: 11 px/15 px; section title: 18 px/24 px.
- Spacing scale: 4, 8, 12, 16, 24, 32.
- Corner radii: 6 px controls, 10 px cards, 14 px temporary dock.
- Hairline borders and restrained elevation; avoid excessive glass, glow, gradients, and floating cards.
- Color communicates state only alongside text/icon changes.

### Motion

- Hover and selection: 100–140 ms.
- Panel enter/exit: 160–220 ms.
- Inspector resize/collapse: 180–240 ms.
- Capture or mic state changes use a single subtle pulse, not continuous animation.
- Reduced Motion replaces transforms with crossfades or immediate state changes.

### Interaction details

- Navigation rows are 32–36 points high with a 16-point icon and quiet selection fill.
- Toolbar controls use symbols plus accessible labels and tooltips.
- Tables/timelines use dense rows, aligned timestamps, monospace only for identifiers and technical values.
- Empty states explain the next concrete setup action.
- Errors appear next to the affected subsystem, with a global summary only when multiple systems fail.
- The bottom dock resembles a compact composer but contains infrastructure controls, not a chat input.

## 6. Primary screens

### 6.1 Overview

Purpose: give a truthful, glanceable view of system health.

Content:

- Four compact status rows: Screen, Accessibility, Microphone, Background.
- Current mode: Running, Private, Paused, Sleeping, or Degraded.
- Active display/window identity with sensitive titles redacted where configured.
- Live rates: captured frames, discarded duplicates, AX events, audio level.
- Recent infrastructure events limited to the last 20 records.
- One-click Pause All and Open Privacy Controls.

### 6.2 Screen Capture

Content:

- Live low-resolution preview with a prominent LIVE/PAUSED badge.
- Display/window source selector using system-provided content selection where possible.
- Current sampling rate, frame dimensions, queue depth, dropped-frame count, and average processing latency.
- Excluded applications and user-defined redaction regions.
- “Capture one diagnostic frame” action that requires an explicit save destination if persisted.
- Side-by-side raw preview versus redacted preview available only while this screen is open.

### 6.3 Accessibility

This screen combines observation with narrowly gated, explicit control.

Content:

- Trust state and System Settings deep link.
- Focused application/window summary.
- Read-only tree inspector: role, title, value summary, bounds, enabled, focused, children count.
- Notification stream with filters for focus, value, selection, window, and application changes.
- Snapshot validation warnings for unsupported, stale, cyclic, or oversized trees.
- Supported AX actions are shown per selected element: press, increment, decrement, show menu, raise, and focus.
- Value editing appears only when AX reports the value attribute as settable and remains length-bounded.
- No synthesized key events, pointer injection, arbitrary action names, autonomous execution, or hidden background control.

### 6.4 Audio

Content:

- Input-device selector, channel count, sample rate, format, and latency.
- Live meter, waveform, Voice Activity Detection state, overrun counter, and buffer health.
- Hold-to-test control; audio is discarded after the test.
- Device-change and permission-recovery guidance.
- Optional local WAV diagnostic export behind explicit confirmation and destination selection.
- No transcription or remote streaming in this milestone.

### 6.5 Diagnostics

Content:

- Subsystem health list and supervisor restart counts.
- CPU, memory, frame throughput, queue occupancy, dropped events, audio overruns, database latency, and UI IPC latency.
- Structured event table with severity, subsystem, timestamp, correlation ID, and bounded metadata.
- Copy redacted diagnostics and export support bundle.
- Support bundle preview lists every included file and field before export.

### 6.6 Settings

Sections:

- General: launch at login, reopen behavior, menu-bar visibility, global shortcut.
- Privacy: capture modes, exclusions, redaction areas, history retention, clear data.
- Screen: preferred source, adaptive sampling bounds, preview quality.
- Audio: input device, VAD sensitivity, buffer length, test controls.
- Appearance: system/light/dark, density, sidebar/inspector defaults.
- Accessibility: text size, increased contrast, reduced motion/transparency.
- Advanced: diagnostics level, database location, reset infrastructure.

## 7. Permission onboarding

Permissions are requested progressively, never as a four-dialog wall.

### Step 1 — welcome

- Explain that Stalky’s first build is local infrastructure.
- Show the four subsystems and their data behavior.
- Allow entry into the app without granting anything.

### Step 2 — Screen Recording

- Demonstrate the feature with a static mock preview before prompting.
- Use a plain-language purpose string.
- After the OS prompt, poll state at bounded intervals and offer a retest button.
- Explain clearly if an app restart is required by the current macOS behavior.

### Step 3 — Accessibility

- State that Stalky reads interface structure and can perform only the supported control the user explicitly selects.
- Prompt through `AXIsProcessTrustedWithOptions` only after user intent.
- Provide a System Settings deep link and an exact verification status.

### Step 4 — Microphone

- Request only when the user opens Audio or presses Hold to Test.
- Show a visible waveform and discard guarantee.
- Handle restricted, denied, and no-device states separately.

### Step 5 — Launch at Login

- Optional and disabled by default.
- Register through `SMAppService`; display OS approval state and deep link.
- The application must remain fully usable when background permission is disabled.

## 8. Rust workspace structure

```text
Stalky/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── deny.toml
├── apps/
│   └── mega-desktop/
│       ├── src-tauri/              # Tauri entry, menus, windows, capabilities
│       └── ui/                     # Leptos app and CSS
├── crates/
│   ├── mega-core/                  # domain state, commands, coordinator
│   ├── mega-runtime/               # supervisors, cancellation, bounded buses
│   ├── mega-platform-macos/        # system events, sleep/lock/display changes
│   ├── mega-capture/               # ScreenCaptureKit adapter and frame pipeline
│   ├── stalky-accessibility/       # live AX observer, bounded snapshots, explicit controls
│   ├── mega-audio/                 # devices, PCM stream, ring buffer, VAD
│   ├── mega-permissions/           # permission state machines and deep links
│   ├── mega-privacy/               # exclusions, redaction, retention decisions
│   ├── mega-store/                 # SQLite, migrations, settings, event metadata
│   ├── mega-observability/         # metrics, spans, bounded structured logging
│   ├── mega-ipc/                   # versioned UI command/event contracts
│   └── mega-test-support/          # fakes, fixtures, deterministic clocks
├── fixtures/
│   ├── ax/
│   ├── frames/
│   ├── audio/
│   └── lifecycle/
├── docs/
│   ├── architecture/
│   ├── privacy/
│   ├── runbooks/
│   └── adr/
└── scripts/                        # build/sign/notarize helpers only
```

Rules:

- The platform crate is the only location allowed to contain macOS FFI and `unsafe` blocks.
- UI code cannot call platform APIs directly.
- IPC types live in one crate and are serialized with explicit version fields.
- Cross-crate domain errors are typed; user-facing messages are mapped at the UI boundary.
- Every queue declares capacity, overflow policy, and metrics.
- `cargo fmt`, Clippy with warnings denied in CI, `cargo deny`, tests, and license checks are required.

## 9. Runtime topology

Use one application process initially. This avoids separate TCC permission identities and reduces signing complexity.

### Execution domains

- **Main thread:** AppKit/Tauri windowing, menu-bar integration, and callbacks that require the macOS main run loop.
- **Tokio runtime:** coordination, persistence, settings, diagnostics, privacy rules, and non-real-time transforms.
- **Screen capture queue:** receives `CMSampleBuffer` frames and immediately converts them to lightweight references/metadata.
- **AX run-loop source:** receives accessibility notifications and schedules bounded normalization work.
- **Audio callback:** real-time-safe copy into a preallocated lock-free ring buffer; no allocation, logging, database access, or async blocking.
- **Persistence worker:** serializes database writes and batches low-priority metrics.

### Supervisors

Each subsystem implements:

```text
Stopped → Starting → Running → Degraded → Stopping → Stopped
                 ↘ Failed(retryable | terminal)
```

Supervisor policy:

- Exponential retry for idempotent startup failures: 250 ms, 1 s, 4 s, then user-visible degraded state.
- No retry loop for denied permissions or missing user action.
- Maximum restart rate prevents crash loops.
- Child cancellation tokens derive from the application token.
- Shutdown order: stop producers → drain bounded metadata → checkpoint settings → close store → exit.

## 10. Typed internal event bus

Use bounded Tokio broadcast/watch/mpsc channels according to semantics:

- `watch`: latest permission, lifecycle, device, and subsystem health state.
- `broadcast`: low-volume UI notifications where lagging consumers may resubscribe.
- `mpsc`: ownership-transfer work queues such as persistence records.
- Lock-free ring buffer: real-time PCM only.

Representative events:

```rust
enum InfraEvent {
    Lifecycle(LifecycleEvent),
    Permission(PermissionEvent),
    Display(DisplayEvent),
    Frame(FrameMetadata),
    Accessibility(AxEvent),
    Audio(AudioEvent),
    Privacy(PrivacyEvent),
    Health(HealthEvent),
}
```

Event envelopes include schema version, monotonic timestamp, wall-clock timestamp, subsystem, correlation ID, sequence number, and redaction classification.

Large frames and PCM buffers never cross the Tauri IPC bridge. The UI receives downscaled preview handles or rate-limited aggregates.

## 11. Screen capture pipeline

```text
SCStream callback
  → validate sample/frame status
  → attach source + timing metadata
  → privacy source filter
  → downscaled change-detection surface
  → perceptual hash / changed-region estimate
  → preview renderer (rate limited)
  → ephemeral latest-frame slot
  → metrics
```

### Capture policies

- Prefer the system content-sharing picker for source selection.
- Exclude Stalky’s own windows from captured content.
- Default to the active display or explicitly selected window; never silently widen scope.
- Typical adaptive rate: 0.2–1 FPS; burst to 2 FPS for up to five seconds after meaningful changes.
- Queue depth target: 3; hard maximum 5 unless profiling justifies otherwise.
- Keep only the newest frame when a consumer falls behind.
- Validate complete frame status before processing.
- Stop capture on screen lock, logout, sleep, permission loss, manual pause, and fatal stream error.
- Re-enumerate sources on display attach/detach and Space/fullscreen transitions.

### Frame ownership

- Avoid CPU copies until a processing consumer actually requests pixels.
- Track IOSurface/CVPixelBuffer lifetime explicitly.
- Never hold framework buffers across unbounded async work.
- Preview conversion happens on a dedicated bounded worker.
- Raw frames are never written automatically.

## 12. Accessibility observation pipeline

```text
workspace/focus notification
  → identify trusted state and target PID
  → attach AXObserver
  → receive bounded notification
  → read allowlisted attributes
  → normalize values and bounds
  → enforce depth/node/text limits
  → redact secure/sensitive values
  → publish snapshot metadata
```

### Read-only allowlist

- Role, subrole, title, description, help, enabled, focused, selected, position, size, window, selected text summary, child count, and supported attribute names.
- Value extraction is bounded by character and collection limits.
- Secure fields never expose values.
- Unknown CF types become typed `Unsupported` values rather than debug dumps.

### Snapshot bounds

- Maximum depth: 12.
- Maximum nodes: 2,500 per explicit diagnostic snapshot; 500 for background focus snapshots.
- Maximum text: 256 characters per node and 32 KiB per snapshot.
- Cycle detection uses stable AX element identity during traversal.
- Stale/invalid elements produce structured errors and trigger one root refresh, not a retry storm.

The crate does not link or expose APIs for setting AX values, performing AX actions, posting keyboard/mouse events, or controlling application focus in this milestone.

## 13. Audio pipeline

```text
device callback
  → preallocated PCM ring buffer
  → format normalization worker
  → 16 kHz mono analysis frames
  → level meter + local VAD
  → ephemeral invocation buffer
  → diagnostics aggregates
```

### Real-time constraints

- No heap allocation or locks in the device callback.
- No UI, network, file, SQLite, tracing formatter, or blocking channel work in the callback.
- Track overruns atomically.
- Device sample rate/channel changes rebuild the graph outside the callback.
- Normalize timestamps to the application monotonic clock.

### Retention

- Default ring buffer: 3 seconds.
- Hold-to-test session maximum: 30 seconds.
- Buffers are zeroed/released on cancel, permission loss, input change, pause, sleep, and completion.
- No network transcription and no automatic audio files.

## 14. Permission state machines

Use a distinct state machine for each protected capability:

```text
Unknown → NotRequested → Requesting → Granted
                              ├──────→ Denied
                              ├──────→ Restricted
                              └──────→ RestartRequired
Granted → Revoked
```

Requirements:

- UI displays OS truth, not optimistic local settings.
- Permission checks are debounced and rate limited.
- Opening System Settings is user initiated.
- Permission loss stops only the dependent producer.
- Recovery does not require deleting preferences or reinstalling.
- Purpose strings are specific, short, and tested on every supported locale.

## 15. Privacy and local data

### Default data policy

- Raw frames: memory only.
- PCM audio: memory only.
- AX text: memory only unless the user exports an explicit diagnostic snapshot.
- Operational events: local SQLite, metadata only, seven-day rolling retention.
- Performance metrics: local aggregates, 30-day rolling retention.
- Content telemetry: disabled.

### Redaction order

1. Exclude disallowed source applications/windows.
2. Remove secure AX values.
3. Apply user-defined display regions.
4. Redact title patterns and sensitive metadata.
5. Generate preview and event projection.
6. Apply export-specific redaction again.

### Local security

- Database lives in the app-support directory with user-only permissions.
- Encryption key is generated locally and stored in Keychain.
- Diagnostic metadata fields containing variable user content are encrypted individually.
- Logs use structured fields and a deny-by-default content policy.
- “Clear local data” removes database content and cached previews while preserving only the minimum preference needed to avoid immediately restarting capture.

## 16. Persistence schema

Tables:

- `schema_migrations(version, applied_at)`
- `settings(key, version, encrypted_value, updated_at)`
- `permission_history(id, capability, old_state, new_state, reason_code, created_at)`
- `lifecycle_events(id, subsystem, event_code, severity, metadata, created_at)`
- `performance_samples(bucket_at, subsystem, metric, min, max, avg, p95, count)`
- `diagnostic_exports(id, manifest_hash, created_at)`

Do not create conversation, prompt, model-call, tool-call, memory, or action tables in this milestone.

SQLite configuration:

- WAL mode and foreign keys enabled.
- Busy timeout set and measured.
- All migrations forward-only during development; release migrations require upgrade fixtures.
- Integrity check on unclean shutdown, with quarantine/recreate flow for unrecoverable diagnostics data.
- Bounded maintenance job for retention and WAL checkpointing.

## 17. UI IPC contract

The UI may call only narrow infrastructure commands:

- `get_app_snapshot`
- `set_capture_mode`
- `select_capture_source`
- `pause_all`
- `resume_allowed_subsystems`
- `request_permission`
- `open_permission_settings`
- `set_audio_device`
- `start_audio_test`
- `stop_audio_test`
- `update_privacy_rules`
- `export_diagnostics`
- `clear_local_data`

All commands:

- Use versioned request/response structures.
- Validate enum values, lengths, identifiers, and paths in Rust.
- Return typed error codes plus safe user-facing summaries.
- Have cancellation and timeout semantics.
- Emit a correlation ID for diagnostics.
- Never accept arbitrary shell commands, scripts, URLs, or filesystem paths outside an OS picker result.

Tauri capabilities are split by window. The main window can access normal controls; the optional preview window gets read-only preview events; permission onboarding gets only permission commands.

## 18. Observability

### Metrics

- Process CPU and resident memory.
- Per-subsystem task count and supervisor state.
- Screen frame input/output/drop rates and processing latency.
- AX notification rate, snapshot size, traversal latency, truncation count, and invalid-element count.
- Audio callback interval, ring occupancy, overrun count, device rebuild count, and VAD duty cycle.
- IPC request latency and rejected-command count.
- SQLite write latency, busy count, WAL size, and maintenance duration.

### Logging

- Levels: error, warn, info, debug, trace.
- Production default: info with content fields removed.
- Every event has subsystem, code, correlation ID, and monotonic timestamp.
- Repeated high-rate errors are sampled and counted rather than emitted endlessly.
- Rotating local files have strict size and age bounds.

### Support bundle

Contains app/build metadata, OS/hardware summary, permission states, redacted configuration, aggregated metrics, recent bounded infrastructure events, crash reports owned by Stalky, and a manifest. It excludes frames, audio, AX text, usernames, window titles, paths, secrets, and Keychain content by default.

## 19. Accessibility of Stalky itself

- Every navigation item, status badge, chart, meter, preview, toolbar control, and dock action has a useful accessibility name and value.
- Status changes such as Paused, Permission Revoked, and Microphone Active are announced without overwhelming live regions.
- Full keyboard order matches visual order.
- Sidebar navigation supports arrow keys; Command-1…6 opens major sections.
- Command-Shift-P toggles Pause All; Escape cancels temporary panels/tests.
- Focus indicators meet contrast requirements and remain visible in increased contrast.
- Charts and waveforms provide textual summaries.
- UI works with VoiceOver, Full Keyboard Access, reduced motion, reduced transparency, larger text, and system accent changes.
- WebView focus entry/exit and title-bar controls receive dedicated regression tests.

## 20. Performance budgets

Validate on the oldest supported Intel Mac and a baseline Apple Silicon Mac.

- Idle CPU: median below 1.5% with all producers paused.
- Context mode CPU: median below 7% during ordinary desktop use.
- Audio-test CPU: median below 5% excluding optional diagnostic visualization.
- Idle RSS: below 220 MB.
- Active screen + AX + audio RSS: below 450 MB.
- Main window cold show: below 400 ms; warm show: below 120 ms.
- Pause All acknowledgement: below 100 ms in UI and producer stop initiated below 150 ms.
- AX focus event to UI summary: p95 below 250 ms.
- Capture frame to preview: p95 below 350 ms at diagnostic preview rate.
- Audio meter latency: p95 below 100 ms.
- UI IPC p95: below 50 ms for local state commands.
- No queue grows beyond declared capacity during a 4-hour soak test.

## 21. Test plan

### Unit tests

- Lifecycle and permission reducers.
- Queue overflow policies and cancellation.
- Privacy rules and redaction order.
- AX normalization, cycle detection, bounds, unsupported types, and secure fields.
- Frame metadata, duplicate detection, timestamps, and retention.
- Audio format conversion, ring wraparound, VAD fixtures, and overrun accounting.
- Store migrations, retention, corrupted rows, and encryption round trips.
- IPC validation and capability routing.

### Integration tests

- Fake capture source produces normal, incomplete, late, duplicate, resized, and missing frames.
- Recorded AX fixtures cover normal apps, large trees, stale elements, missing attributes, custom controls, and process exit.
- Virtual/fake audio sources cover silence, speech, noise, clipping, sample-rate switches, device removal, and permission loss.
- Permission tests cover Not Requested, Denied, Granted, Revoked, Restricted, and Restart Required.
- Forced termination during startup, capture, database write, permission polling, preview, and export.
- Lock/unlock, sleep/wake, logout cancellation, display attach/detach, resolution/scaling changes, Spaces, Stage Manager, and fullscreen.

### UI tests

- Screenshot baselines for every screen in light/dark and normal/increased contrast.
- Widths: 960, 1,080, 1,240, 1,440, and 1,720 points.
- Text scales: 100%, 125%, 150%, and 200%.
- Sidebar collapsed/expanded and inspector absent/present.
- Keyboard-only onboarding, navigation, capture selection, audio test, diagnostics export, and data deletion.
- VoiceOver reading order and live-state announcements.
- Reduced Motion and Reduced Transparency snapshots.

### Manual hardware matrix

- Apple Silicon laptop with built-in Retina display and microphone.
- Apple Silicon desktop with external display and USB microphone.
- Supported Intel Mac with non-Retina external display.
- One-, two-, and three-display arrangements.
- AirPods/Bluetooth input, USB input, aggregate device, and device unplug during capture.

### Release blockers

- Any hidden capture or mismatched indicator.
- Pause fails to stop a producer.
- Raw content appears in logs, database, or support bundles unexpectedly.
- Permission denial creates a dead end or crash.
- Unbounded queue/memory growth.
- Audio callback allocation or blocking.
- Keyboard trap, missing focus indicator, or inaccessible critical control.
- Unsigned nested binary, notarization failure, or updater signature failure.

## 22. Build, signing, and distribution

- Minimum deployment target: macOS 15 for the first beta.
- Produce `aarch64-apple-darwin` and `x86_64-apple-darwin`; combine and test a universal application.
- Enable hardened runtime and only required entitlements.
- Include specific `NSScreenCaptureUsageDescription` and `NSMicrophoneUsageDescription` strings.
- Sign the deepest nested code first, then the enclosing bundle.
- Submit with Developer ID through `notarytool` and staple the ticket.
- Verify with `codesign`, `spctl`, clean-machine install, first launch, upgrade, and removal tests.
- Use an authenticated, signature-verified update feed with staged release channels: internal, alpha, beta, stable.
- The updater cannot run while a capture export is being written and must preserve/validate schema compatibility.

## 23. Delivery phases

### Phase 0 — platform feasibility (4–5 engineering days)

- Scaffold Tauri + Leptos and render the three-pane shell.
- Prove menu-bar state, global shortcut, integrated title bar, optional inspector, and VoiceOver basics.
- Receive one ScreenCaptureKit stream, one AX notification, and one microphone stream from Rust.
- Validate permission identity in dev and signed builds.
- Produce a notarized spike and baseline CPU/RSS measurements.

Exit gate: retain Option A unless native integration or accessibility quality has a demonstrated blocker. If blocked, move only the shell/platform adapter to SwiftUI and keep Rust domain/runtime crates.

### Phase 1 — workspace and lifecycle (4–6 days)

- Workspace crates, dependency policy, CI, typed errors, cancellation tree, supervisors, bounded event bus, deterministic clock, and test-support crate.
- App lifecycle handling for launch, close, reopen, terminate, lock, sleep, wake, and display changes.

Exit gate: deterministic lifecycle tests and clean startup/shutdown under forced failure.

### Phase 2 — Codex-inspired shell and design system (5–7 days)

- Tokens, typography, sidebar, toolbar, workspace, inspector, bottom dock, tables, status rows, permission cards, empty/error states, and responsive breakpoints.
- Overview, Settings, and placeholder subsystem screens.
- Light/dark, increased contrast, reduced motion/transparency, keyboard map, and first screenshot suite.

Exit gate: all primary layout states pass visual and accessibility review before deeper features fill them.

### Phase 3 — permissions and privacy core (4–6 days)

- Permission state machines, onboarding, System Settings links, purpose strings, revocation polling, capture modes, exclusion models, encrypted settings, and Clear Local Data.

Exit gate: every permission path works from a clean user account and after revocation.

### Phase 4 — screen infrastructure (7–10 days)

- ScreenCaptureKit adapter, source picker, source exclusion, frame lifetimes, adaptive sampler, duplicate detection, downscaled preview, privacy regions, metrics, and capture diagnostics UI.

Exit gate: multi-display and sleep/wake soak tests meet CPU/memory and no-retention requirements.

### Phase 5 — read-only Accessibility infrastructure (5–7 days)

- Trust flow, observer lifecycle, focus/application switching, allowlisted attributes, snapshot normalization/bounds/redaction, fixture recorder, and read-only inspector.

Exit gate: no write/action symbols exposed; large and malformed trees remain bounded and responsive.

### Phase 6 — audio infrastructure (5–7 days)

- Device discovery, PCM callback, ring buffer, normalization, meters, VAD, Hold to Test, device rebuilds, lifecycle integration, and audio diagnostics UI.

Exit gate: zero callback allocations/blocks, no unexpected persistence, and device matrix passes.

### Phase 7 — storage and diagnostics (4–6 days)

- SQLite migrations, retention, encrypted fields, metric aggregation, rotating logs, diagnostics tables, support bundle preview/export, and corruption recovery.

Exit gate: content leak audit passes and interrupted writes recover.

### Phase 8 — hardening and beta packaging (7–10 days)

- Performance optimization, long soak tests, complete UI matrix, VoiceOver review, permissions support runbook, universal signing/notarization, update channel, clean-machine install/upgrade/uninstall, and release checklist.

Exit gate: every release blocker in section 21 is cleared.

Estimated infrastructure milestone: 7–10 calendar weeks for two experienced engineers or 12–18 weeks for one engineer, including beta hardening.

## 24. Deliverables

- Signed and notarized Stalky `.app` and installer artifact.
- Rust workspace with reproducible builds and CI.
- Codex-inspired, original desktop design system and primary screens.
- Screen capture, read-only AX, and microphone infrastructure.
- Permission onboarding and privacy controls.
- Local store, metrics, diagnostics UI, and redacted support bundles.
- Automated unit/integration/UI suites and hardware QA report.
- Architecture decisions, privacy data map, operational runbooks, signing guide, and release checklist.
- Stable, versioned infrastructure interfaces for a later AI layer.

## 25. Explicitly deferred work

- Model API integration or local model packaging.
- Prompt construction, context selection for a model, token management, or compaction.
- Chat sessions, conversational memory, embeddings, or retrieval.
- Agent loops, planning, tools, approvals, or automation schedules.
- Accessibility writes, clicks, typing, keyboard/mouse injection, clipboard access, or AppleScript.
- Remote audio transcription.
- External connectors, plugins, third-party app integrations, or cloud sync.

These items require a separate design and approval after the infrastructure milestone meets its release gates.

## 26. Research basis

- Apple’s ScreenCaptureKit provides filtered, high-performance screen streams and runtime stream reconfiguration: [ScreenCaptureKit overview](https://developer.apple.com/documentation/ScreenCaptureKit?language=objc) and [macOS capture sample](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos?changes=_9).
- Accessibility trust is controlled by the user, while AXUIElement exposes the structure of accessible applications: [AXIsProcessTrustedWithOptions](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions) and [AXUIElement](https://developer.apple.com/documentation/applicationservices/axuielement_h?changes=latest_ma_2).
- Tauri supports Rust application logic, Rust frontends, and constrained per-window IPC capabilities: [Tauri overview](https://v2.tauri.app/start/), [frontend configuration](https://v2.tauri.app/start/frontend/), and [capabilities](https://v2.tauri.app/security/capabilities/).
- Apple recommends Service Management for user-visible login/background items: [SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice?language=objc).
- Developer ID distribution requires hardened runtime and notarization: [Apple notarization guidance](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution?changes=_9).
- Official OpenAI material describes a Mac app shell pattern with a sidebar, detail pane, and inspector; Stalky borrows that general information architecture while remaining visually original: [OpenAI macOS use case](https://learn.chatgpt.com/use-cases).
