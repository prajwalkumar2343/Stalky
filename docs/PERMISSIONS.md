# Stalky macOS permissions

Stalky treats Accessibility, Screen Recording, and Microphone as independent macOS privacy permissions. Launch at Login is intentionally separate and is not implemented by this flow.

## Runtime behavior

- Startup and focus/visibility checks perform read-only probes. They never call a native request API.
- The onboarding screen is a pre-prompt. Each native request starts only after the user presses `Request access`, one capability at a time.
- `Open System Settings` uses an allowlisted capability-specific URL and falls back to the general Privacy & Security pane and then System Settings itself.
- Returning focus schedules one debounced, read-only recheck. Settings also remains available permanently for recovery.
- The coordinator retains the last trustworthy authorization separately from transient `Requesting` and `Rechecking` operations. A missing authorization stops the protected capture or Accessibility subsystem; a previously granted authorization remains usable while a recheck is in flight.
- Screen Recording and Accessibility boolean probes return `Unknown` on an initial false result because macOS collapses not-determined and denied. The UI presents this as `Needs access`, never as a precise denial. Microphone uses the precise AVFAudio `NotDetermined`, `Denied`, or `Granted` result where available.

The native adapter is available only on macOS. The app bundle includes `NSMicrophoneUsageDescription`, `NSScreenCaptureUsageDescription`, the microphone input entitlement, and a macOS 15.0 minimum runtime. Distribution builds must still be signed with the final bundle identifier (`com.stalky.desktop`) so TCC records permissions against the installed app identity.

## Manual macOS TCC QA

These checks require a signed macOS app and cannot be made deterministic in workspace tests:

1. Start from a clean TCC state. Quit Stalky, then use `tccutil reset Accessibility com.stalky.desktop`, `tccutil reset ScreenCapture com.stalky.desktop`, and `tccutil reset Microphone com.stalky.desktop` as appropriate for the test machine.
2. Launch Stalky and verify no privacy prompt appears. The first-run pre-prompt is visible, keyboard reachable, and VoiceOver announces the heading, current step, status, and actions.
3. Request Screen Recording, Accessibility, and Microphone one at a time. Verify each OS prompt/settings transition is user-triggered, the card announces `Requesting`, and the settled state is reflected without a webview reload.
4. Deny each capability, close and reopen the app, and verify the card explains recovery. Restricted/managed access must show policy copy and must not offer a native request loop.
5. Use `Open System Settings`, change one switch, return focus to Stalky, and verify the debounced recheck settles the card. Repeat with the specific URL anchor unavailable or on a macOS version where it falls back to the general pane.
6. While capture or Accessibility observation is active, revoke its TCC switch in System Settings. Return to Stalky and verify the protected service stops or declines safely, the state becomes revoked/not granted, and no new native prompt appears.
7. Start capture or Accessibility directly before authorization and verify the command is rejected by the coordinator/runtime gate. Grant access, then verify the subsystem starts only after a fresh granted authorization.
8. Resize below the normal desktop width, test disabled controls and high-contrast/dark appearance, and verify the modal remains readable. Enable `prefers-reduced-motion` and verify no essential permission feedback depends on motion.
9. Press Escape to dismiss onboarding, reopen the app, and verify the versioned onboarding choice persists while Settings still exposes all three recovery cards.

For a final release build, repeat the same matrix on the signed `.app`/DMG rather than only a development binary; TCC identity and prompt behavior are bundle-signing dependent.
