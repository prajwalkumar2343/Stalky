# AuraBot - AI Memory Assistant for macOS

> [!WARNING]
> **🚧 This project is currently under active development and is not ready for use. Do not run it in its current state.**
> **📄 The documentation is also outdated and does not reflect the current state of the project.**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

AuraBot is a local-first macOS memory assistant that captures useful work context, stores it in a managed PGlite backend, and lets you search or chat over your recent activity. The current app prefers structured browser, terminal, project, and app metadata when available, and only falls back to visual screen capture when it needs richer context.

## How It Works

```
App / Browser / Project Context → Smart Routing → Optional Screen Analysis → Local Memory Storage → Search / Chat
```

1. **Collect**: AuraBot checks the active workspace and browser context on a short probe loop
2. **Route**: The app stores structured context directly for supported browser, terminal, coding, and document workflows
3. **Fallback**: When structured context is not enough, AuraBot captures and analyzes the screen
4. **Store**: Context is written to the local Memory v2 backend for search, recent context, and graph extraction
5. **Retrieve**: You can search your activity history or use memory-aware responses inside the app

### Architecture

- **macOS App**: `apps/macos` handles context routing, optional screen capture, user interaction, and starts the managed local memory backend
- **PGlite Memory Backend**: `services/memory-pglite` provides Memory v2 storage, search, graph extraction, and markdown brain indexing
- **LLM Integration**: OpenRouter powers screen analysis, chat, and prompt enhancement
- **Browser Context Server**: the macOS app exposes a local extension API on `127.0.0.1:7345` by default
- **Computer Use Engine**: AuraBot embeds the reviewed Cua computer-use engine directly and presents it only as AuraBot

### Repository Layout

```text
apps/macos/             # SwiftUI macOS app
services/memory-pglite/ # Local-first PGlite Memory v2 service
tools/                  # Development and demo utilities
config/                 # Example configuration
docs/                   # Project documentation
```

## Prerequisites

- macOS 14.0+ (Sonoma)
- OpenRouter API key ([get one here](https://openrouter.ai/settings/keys))
- Screen Recording permission (prompted on first launch)

## Getting Started

### 1. Clone and Setup

```bash
git clone https://github.com/prajwalkumar2343/aurabot.git
cd aurabot
```

### 2. Configure Environment

```bash
cp .env.example .env
# Add OPENROUTER_API_KEY to .env
```

### 3. Run the App

```bash
cd apps/macos && swift run AuraBot
```

The app starts the PGlite memory backend automatically on `127.0.0.1:8766`.
Packaged builds include the memory service in the `.app` bundle, so users do not need to run a separate server.
Packaged builds include AuraBot Computer Use directly in the macOS binary. Settings exposes it as a single AuraBot feature for permissions, diagnostics, and trajectory recording.

## Usage

- **Menu Bar**: Access capture controls, recent memories, and search
- **Cmd+Opt+E**: Enhance selected text with your memory context
- **Search / Chat**: Query your activity history with natural language

## Capture Behavior

AuraBot does not simply save a screenshot every 30 seconds.

- It probes for new context every 5 seconds by default.
- It keeps at least a 20-second minimum gap between accepted visual captures.
- It stores structured context directly for supported browser, terminal, coding, and document flows.
- It triggers visual capture for meaningful visual changes, page changes, new media sessions, scroll novelty, or long idle checkpoints.
- It takes an initial capture when capture starts, then continues only when the routing and change-detection logic says the update is worth storing.

## Configuration

Key environment variables (see `.env.example`):

| Variable | Description |
|----------|-------------|
| `OPENROUTER_API_KEY` | Your OpenRouter API key (required) |
| `AURABOT_MEMORY_PGLITE_PORT` | Managed local memory backend port (default: 8766) |

Runtime settings live in `~/.aurabot/config.json` after first launch. Relevant defaults in the current app include:

- `capture.probeIntervalSeconds`: `5`
- `capture.minCaptureGapSeconds`: `20`
- `capture.idleCaptureSeconds`: `300`
- `capture.meaningfulChangeThreshold`: `10`
- `extension.port`: `7345`
- `extension.freshnessSeconds`: `15`

## License

MIT License - see [LICENSE](LICENSE) file for details.
