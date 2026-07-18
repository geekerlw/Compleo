# Compleo

> *complēre* — Latin: to complete, to fill in.

AI reply assistant for macOS. Reads your chat window, generates a contextual reply, copies it to your clipboard.

![icon](src-tauri/icons/128x128.png)

## How it works

1. Press **⌘ .** in any chat app (WeChat, QQ, DingTalk, Slack…)
2. Compleo screenshots the chat area, runs OCR with position detection
3. LLM generates a reply matching your style
4. Reply is copied to clipboard — just **⌘V** to paste

The overlay window appears without stealing focus. It shows the thinking process and final reply, then auto-dismisses.

## Features

- **Screenshot + OCR** — Vision Framework with Chinese/English/Japanese/Korean support
- **Position-aware OCR** — Marks messages as `←` (them) or `→` (you) based on screen position
- **Streaming LLM** — Real-time display with `<think>` tag filtering
- **Style memory** — SQLite stores your accepted replies; LLM learns your tone per app
- **Non-intrusive overlay** — Floating window that doesn't steal focus (NSPanel)
- **Tray popover** — Left-click for quick status, right-click for menu
- **Configurable** — API key, model, base URL via settings panel
- **macOS vibrancy** — Native blur materials, follows system appearance

## Requirements

- macOS 13+ (Ventura or later)
- Screen Recording permission
- Accessibility permission (optional, for input field detection)
- An OpenAI-compatible API key

## Setup

```bash
# Install dependencies
npm install

# Build and install to /Applications
./install.sh

# Or manually:
npx tauri build --debug
cp -R src-tauri/target/debug/bundle/macos/Compleo.app /Applications/
```

On first launch:
1. Grant Screen Recording permission in System Settings → Privacy & Security
2. Restart the app
3. Left-click tray icon → Settings → configure your API key and model

## Configuration

Settings are stored at `~/.config/compleo/config.json`:

```json
{
  "api_key": "sk-...",
  "base_url": "https://api.openai.com/v1",
  "model": "gpt-4o-mini",
  "hotkey": "Cmd+."
}
```

Environment variables (`OPENAI_API_KEY`, `COMPLEO_BASE_URL`, `COMPLEO_MODEL`) work as fallbacks.

## Architecture

```
Tauri 2.0 + Rust backend + React frontend

src-tauri/src/
├── lib.rs          # App shell: tray, hotkey, window management, pipeline
├── config.rs       # JSON config persistence
├── storage.rs      # SQLite memory (conversations + style learning)
├── platform/
│   ├── mod.rs      # PlatformProvider trait
│   └── macos.rs    # Screenshot (screencapture) + OCR (Swift helper)
└── llm/
    ├── mod.rs      # LLM abstraction
    └── openai.rs   # OpenAI-compatible streaming with SSE parsing

src-tauri/swift-ocr/
└── main.swift      # Vision Framework OCR with CJK + position output

src/
├── App.tsx         # Overlay window (streaming reply display)
├── theme.css       # Design tokens
└── windows/
    ├── MainApp.*   # Full app window with sidebar nav
    ├── Settings.*  # API key, model, base URL configuration
    └── Popover.*   # Tray popover (status, last reply)
```

## Data

- Config: `~/.config/compleo/config.json`
- Database: `~/Library/Application Support/Compleo/compleo.db`
- Logs: `~/Library/Logs/com.compleo.app/Compleo.log`
- Screenshots: `/tmp/compleo/screenshot.png` (overwritten each trigger)

Records auto-delete after 30 days.

## License

MIT
