# AuraBot - Swift Version

A native Swift/macOS rewrite of AuraBot with smart context routing, optional visual capture, a local memory backend, and an embedded computer-use engine.

## Features

- ✅ **Smart Context Capture** - Routes browser, terminal, coding, and app context before falling back to screenshots
- ✅ **Screen Capture Fallback** - Uses ScreenCaptureKit when visual context is needed
- ✅ **Memory Storage** - Memory API integration with vector embeddings
- ✅ **LLM Integration** - OpenAI-compatible API support
- ✅ **Quick Enhance** - Global hotkey (⌘⌥E) to enhance any text
- ✅ **Floating Overlay** - System-wide floating button
- ✅ **SwiftUI Interface** - Native macOS app
- ✅ **Browser Context API** - Local HTTP API for browser extension support
- ✅ **Computer Use** - Embedded AuraBot computer-use engine for app/window automation

## Requirements

- macOS 13.0+
- Xcode 15.0+
- Swift 5.9+

## Build Instructions

### 1. Clone and Setup

```bash
cd apps/macos
```

### 2. Build with Swift Package Manager

```bash
swift build
```

### 3. Run the App

```bash
swift run AuraBot
```

### 4. Create App Bundle (Optional)

```bash
swift build -c release
# Then package as .app
```

## Architecture

```text
Sources/AuraBot/
├── Core/
│   └── AppDelegate.swift      # App lifecycle & global hotkeys
├── ContextRouting/
│   ├── ContextRouter.swift    # Chooses structured context vs visual fallback
│   ├── BrowserContextCollector.swift
│   ├── ActiveAppCollector.swift
│   ├── TerminalContextCollector.swift
│   └── GitContextCollector.swift
├── Models/
│   ├── Config.swift           # Configuration models
│   ├── Memory.swift           # Memory data models
│   └── ScreenCapture.swift    # Capture models
├── Services/
│   ├── AppService.swift       # Main service orchestrator
│   ├── LLMService.swift       # LLM API client
│   ├── MemoryService.swift    # Memory API client
│   ├── MemoryBackendSupervisor.swift # Managed local memory backend
│   ├── ScreenCaptureService.swift  # ScreenCaptureKit wrapper
│   ├── BrowserContextService.swift  # Browser context cache and fallback state
│   └── BrowserExtensionServer.swift # Local extension API
├── Screens/
│   ├── DashboardView.swift
│   ├── MemoriesView.swift
│   ├── ChatView.swift
│   ├── SettingsView.swift
│   └── PermissionOnboardingView.swift
├── UI/
│   ├── AuraBotApp.swift       # SwiftUI App entry
│   ├── ContentView.swift      # Main window layout
│   ├── OverlayWindow.swift    # Floating button window
│   └── QuickEnhancePanel.swift # Quick enhance popup
└── Utils/
```

## Usage

### Quick Enhance

1. Select text in any app
2. Press **⌘⌥E** (Cmd+Opt+E)
3. Click the floating purple button
4. Your text is enhanced with memories

### Screen Capture

1. Enable capture in settings
2. AuraBot probes for context every 5 seconds by default
3. Structured browser/app/project context is stored directly when available
4. ScreenCaptureKit is used only when visual fallback is needed
5. Visual captures respect a minimum gap and change-detection rules before being stored

### Chat with Memories

1. Open Chat tab
2. Ask questions about your activities
3. AI uses stored memories for context

## Dependencies

- **Vapor** - HTTP server for browser extension API
- **KeyboardShortcuts** - Global hotkey handling
- **ScreenCaptureKit** - Native screen capture (built-in)
- **Memory PGlite** - Managed local Memory v2 backend for storage, search, graph extraction, and markdown brain indexing
- **AuraBot Computer Use** - Embedded computer-use engine managed invisibly by AuraBot

## Configuration

Config is stored at `~/.aurabot/config.json`:

```json
{
  "capture": {
    "intervalSeconds": 30,
    "quality": 60,
    "maxWidth": 1280,
    "maxHeight": 720,
    "enabled": true,
    "probeIntervalSeconds": 5,
    "minCaptureGapSeconds": 20,
    "idleCaptureSeconds": 300,
    "previewWidth": 160,
    "previewHeight": 90,
    "meaningfulChangeThreshold": 10,
    "scrollCaptureCooldownSeconds": 20
  },
  "llm": {
    "baseURL": "https://openrouter.ai/api/v1",
    "model": "google/gemini-flash-1.5",
    "maxTokens": 512,
    "temperature": 0.7,
    "timeoutSeconds": 30,
    "openRouterAPIKey": "",
    "openRouterChatModel": "anthropic/claude-3.5-sonnet",
    "contextCollectorRewrite": {
      "enabled": false,
      "allowedModels": [
        { "label": "Gemini >= 3.1", "minimumVersion": 3.1, "matchPatterns": ["gemini[-_ ]?(\\d+(?:\\.\\d+)?)"], "requiredTokens": [] },
        { "label": "Claude Opus >= 4.5", "minimumVersion": 4.5, "matchPatterns": ["claude[-_ ]?opus[-_ ]?(\\d+(?:\\.\\d+)?)", "claude[-_ ]?(\\d+(?:\\.\\d+)?)[:/_ -]?opus"], "requiredTokens": ["claude", "opus"] },
        { "label": "GPT >= 5.3", "minimumVersion": 5.3, "matchPatterns": ["gpt[-_ ]?(\\d+(?:\\.\\d+)?)"], "requiredTokens": [] },
        { "label": "Kimi >= 2.5", "minimumVersion": 2.5, "matchPatterns": ["kimi[-_ ]?(\\d+(?:\\.\\d+)?)"], "requiredTokens": [] }
      ]
    }
  },
  "memory": {
    "baseURL": "http://127.0.0.1:8766",
    "apiKey": "memory-v2-token"
  },
  "extension": {
    "enabled": true,
    "port": 7345,
    "freshnessSeconds": 15,
    "apiKey": "browser-extension-token",
    "allowedOrigins": [
      "chrome-extension://",
      "moz-extension://",
      "safari-web-extension://",
      "http://localhost:",
      "http://127.0.0.1:"
    ]
  }
}
```

AuraBot starts the local PGlite memory backend automatically. In packaged builds, `scripts/build-app.sh` bundles the built `services/memory-pglite` service into the app resources so users can launch AuraBot like a normal macOS app.

AuraBot embeds its computer-use engine directly into the macOS binary. Settings keeps the feature under the single “Computer Use” surface for enablement, permissions, diagnostics, and trajectory recording.

### Browser Extension Context API

The app listens on `127.0.0.1:7345` by default for browser context updates:

```http
POST /browser/context
Authorization: Bearer browser-extension-token
Content-Type: application/json
```

Extensions may also send `X-AuraBot-Extension-Key: browser-extension-token` instead of the bearer header. The token must match `extension.apiKey` in `~/.aurabot/config.json`; requests without a matching key are rejected. Origins must also match `extension.allowedOrigins`.

### Current Capture Logic

- The context loop runs every `capture.probeIntervalSeconds` seconds, default `5`.
- Browser extension context is preferred when it is fresh.
- Terminal, coding, and several app-specific workflows produce structured context events without requiring a screenshot.
- Visual capture is used as fallback and is gated by change detection, media-session changes, scroll novelty, page changes, and idle checkpoints.

## License

MIT
