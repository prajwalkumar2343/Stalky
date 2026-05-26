# AuraBot - AI Memory Assistant for macOS

> [!WARNING]
> **🚧 This project is currently under active development and is not ready for use. Do not run it in its current state.**
> **📄 The documentation is also outdated and does not reflect the current state of the project.**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

AuraBot is an intelligent screen capture and memory system that learns your context and activities over time. It uses vision AI to understand your screen content and stores meaningful memories using a vector database, enabling you to query your digital history and get AI-assisted responses with personal context.

## How It Works

```
Screen Capture (every 30s) → Vision AI Analysis → Vector Storage → Memory Search → AI Responses
```

1. **Capture**: AuraBot periodically screenshots your screen
2. **Analyze**: Vision AI understands what's on screen
3. **Store**: Memories are embedded and stored in a vector database
4. **Retrieve**: Search your activity history with natural language
5. **Respond**: Get AI-powered answers with your personal context

### Architecture

- **macOS App**: `apps/macos` handles screen capture, user interaction, and starts the managed local memory backend
- **PGlite Memory Backend**: `services/memory-pglite` provides Memory v2 storage, search, graph extraction, and markdown brain indexing
- **LLM Integration**: OpenRouter for vision and chat capabilities
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
- **Search**: Query your activity history with natural language

## Configuration

Key environment variables (see `.env.example`):

| Variable | Description |
|----------|-------------|
| `OPENROUTER_API_KEY` | Your OpenRouter API key (required) |
| `AURABOT_MEMORY_PGLITE_PORT` | Managed local memory backend port (default: 8766) |

Capture settings and matching API keys can be adjusted in `~/.aurabot/config.json` after first launch.

## License

MIT License - see [LICENSE](LICENSE) file for details.
