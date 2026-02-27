<p align="center">
  <img src="static/logo-text.png" width="360" alt="OpenCovibe">
</p>

<p align="center">
  <strong>Local-first desktop app for AI-assisted vibe coding</strong>
</p>

<p align="center">
  <a href="#why-opencovibe">Why</a> &middot;
  <a href="#key-capabilities">Capabilities</a> &middot;
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#supported-providers">Providers</a> &middot;
  <a href="#architecture">Architecture</a> &middot;
  <a href="#license">License</a>
</p>

<p align="center">
  <b>English</b> | <a href="README.zh-CN.md">简体中文</a>
</p>

---

<p align="center">
  <img src="static/screenshot.png" width="800" alt="OpenCovibe Screenshot">
</p>

## Why OpenCovibe?

AI coding CLIs like Claude Code are powerful, but they run inside a terminal. That means no persistent dashboard, no visual diff review, no cross-session history, and no multi-provider switching. OpenCovibe wraps these CLIs with a native desktop UI that adds the layers the terminal can't provide — while keeping all your data **stored locally**. (Remote model APIs require network access; the app itself has no cloud backend.)

| Agent | Status |
|-------|--------|
| [Claude Code](https://github.com/anthropics/claude-code) | Supported |
| [Codex](https://github.com/openai/codex) | In progress |

**Platform status**: Currently developed and tested primarily on **macOS**. Windows and Linux builds are functional but have not been thoroughly tested for compatibility — contributions and bug reports are welcome.

**Core principle**: Wrap the CLI, surface the work, keep it local.

## Key Capabilities

### What the CLI doesn't give you

| Capability | What OpenCovibe adds |
|------------|---------------------|
| **Visual Tool Cards** | Every tool call (Read, Edit, Bash, Grep, Write, WebFetch, …) rendered as an inline card with syntax-highlighted diffs, structured output, and one-click copy |
| **Run History & Replay** | Browse all past sessions, full event replay, resume / fork from any point |
| **Multi-Provider Switching** | Use Claude Code with 15+ API providers (DeepSeek, Kimi, Zhipu, Bailian, DouBao, MiniMax, OpenRouter, Ollama, …) — hot-switch without restarting |
| **Agent Management** | Visual editor to create, edit, and manage custom agent definitions (.md files) with form and source modes |
| **Usage Analytics** | Per-model token breakdown, cost tracking, daily heatmap, session-level stats |
| **Team Dashboard** | Read-only view into Claude Code multi-agent teams — task lists, teammate status, message flow |
| **Activity Monitor** | Real-time hook event stream, tool activity timeline, subagent tracking with nested tool cards |
| **Plugin Marketplace** | Browse, install, and manage Claude Code plugins and skills from a visual marketplace |
| **MCP Management** | Discover MCP servers, view per-server status, reconnect / toggle from a panel |
| **Inline Permissions** | Rich permission review UI with Allow/Deny buttons, CLI-suggested "Always Allow" rules, and AskUserQuestion rendering |

### Features

- **Rich Chat UI** — Markdown, syntax highlighting, thinking blocks, image attachments, file diffs, collapsible tool groups
- **Session Control** — Create, resume, fork, rename sessions; plan mode toggle; model hot-switch
- **Inline Slash Commands** — `/model`, `/diff`, `/todos`, `/tasks`, `/doctor`, `/copy`, `/stats`, and more — rendered natively in-app
- **Keyboard Shortcuts** — Fully customizable keybindings with chord support and conflict detection
- **i18n** — English and Chinese (Simplified) with lightweight reactive runtime
- **System Tray** — Hide to tray; background sessions keep running with native notifications
- **Dark / Light Theme** — CSS variable-based theming
- **Setup Wizard** — Guided CLI detection, authentication, and provider configuration on first launch

## Quick Start

> **Pre-built binaries** are not available yet. For now, build from source following the instructions below. See [Releases](https://github.com/AnyiWang/OpenCovibe/releases) for future updates.

### Option A: Automated Setup (macOS)

```bash
git clone https://github.com/AnyiWang/OpenCovibe.git
cd OpenCovibe
./scripts/setup.sh          # add --yes to skip confirmation prompts
npm run tauri dev
```

The setup script detects missing dependencies (Xcode CLI Tools, Homebrew, Node.js, Rust) and installs them automatically.

### Option B: Manual Setup

**Prerequisites:**

- [Node.js](https://nodejs.org/) >= 20
- [Rust](https://rustup.rs/) >= 1.75

**macOS:**
```bash
xcode-select --install
brew install node
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Linux (Debian/Ubuntu):**
```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Windows:**
```powershell
# Install Rust from https://rustup.rs
# Install Node.js from https://nodejs.org
```

**Build & Run:**

```bash
git clone https://github.com/AnyiWang/OpenCovibe.git
cd OpenCovibe
npm install
npm run tauri dev
```

### Setup Wizard

On first launch, OpenCovibe guides you through:

1. **CLI Detection** — Auto-detects Claude Code CLI, offers installation if missing
2. **Authentication** — OAuth login or API key for 15+ providers
3. **Ready** — Start coding

You can re-run the wizard anytime from **Settings > General > Setup Wizard**.

## Supported Providers

### LLM Providers

| Provider | Endpoint | Auth |
|----------|----------|------|
| Anthropic | Official API | API Key |
| DeepSeek | `api.deepseek.com/anthropic` | API Key |
| Kimi (Moonshot) | `api.moonshot.cn/anthropic` | Bearer |
| Kimi For Coding | `api.kimi.com/coding/` | API Key |
| Zhipu (智谱) | `open.bigmodel.cn/api/anthropic` | API Key |
| Bailian (百炼) | `dashscope.aliyuncs.com/apps/anthropic` | Bearer |
| DouBao (豆包) | `ark.cn-beijing.volces.com/api/coding` | API Key |
| MiniMax | `api.minimax.io/anthropic` | API Key |
| MiniMax (China) | `api.minimaxi.com/anthropic` | API Key |
| Xiaomi MiMo (小米) | `api.xiaomimimo.com/anthropic` | API Key |

### API Gateway

| Platform | Endpoint | Auth |
|----------|----------|------|
| Vercel AI Gateway | `ai-gateway.vercel.sh` | Bearer |
| OpenRouter | `openrouter.ai/api` | Bearer |
| AiHubMix | `aihubmix.com` | API Key |

### Local

| Platform | Endpoint |
|----------|----------|
| Ollama | `localhost:11434` |
| Custom | Any Anthropic-compatible endpoint |

## Architecture

<p align="center">
  <img src="static/architecture.svg" width="700" alt="Architecture">
</p>

**Tech Stack:**

| Layer | Technology |
|-------|-----------|
| Framework | [Tauri v2](https://v2.tauri.app/) (Rust backend + WebView) |
| Frontend | [Svelte 5](https://svelte.dev/) + [SvelteKit](https://svelte.dev/docs/kit/) (adapter-static) |
| Styling | [Tailwind CSS](https://tailwindcss.com/) v3 + CSS variables |
| Terminal | [xterm.js](https://xtermjs.org/) |
| Markdown | [marked](https://marked.js.org/) + [highlight.js](https://highlightjs.org/) + [DOMPurify](https://github.com/cure53/DOMPurify) |
| i18n | Custom lightweight runtime (en + zh-CN) |
| Testing | [Vitest](https://vitest.dev/) |

**Agent Communication:**

The app communicates with Claude Code CLI via bidirectional stream-JSON protocol (stdin/stdout). Each session is a long-lived, multi-turn process managed by a per-run session actor. Four communication modes are supported: stream-JSON (primary), PTY (interactive terminal), pipe (Codex), and direct API (HTTP streaming).

**Data Storage:**

All data is stored locally at `~/.opencovibe/` — no cloud, no database.

```
~/.opencovibe/
├── settings.json          # User settings
├── runs/                  # Session history
│   └── {run-id}/
│       ├── meta.json      # Run metadata
│       ├── events.jsonl   # Event log
│       └── artifacts.json # Summary
└── keybindings.json       # Custom shortcuts
```

## Development

```bash
npm install              # Install dependencies
npm run tauri dev        # Dev mode with hot-reload
npm test                 # Run tests
npm run lint:fix         # Lint
npm run format           # Format
```

## Contributing

Contributions are welcome! Please open an [issue](https://github.com/AnyiWang/OpenCovibe/issues) for bug reports or feature requests, and submit pull requests for improvements.

## License

Licensed under the [Apache License 2.0](LICENSE).

Copyright 2025-2026 OpenCovibe Contributors.
