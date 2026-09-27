<p align="center">
  <img src="assets/logo.webp" alt="Comrade logo" width="180" />
</p>

# Comrade — Desktop AI Agent (Rust + Tauri v2)

Local-first personal AI desktop agent. Pure Rust backend (`core` library +
Tauri shell), vanilla HTML/CSS/JS frontend — no Node, no Electron.

Brain: DeepSeek-direct (`deepseek-flash`, reasoning model). Voice STT/TTS and
memory embeddings ride on OpenRouter (DeepSeek has no audio/embedding APIs).

Two stores, two jobs:
- `history.db` — verbatim chat sessions, browsed from the ☰ sidebar.
- `comrade-memory.db` — distilled knowledge (facts, preferences, projects),
  recalled by vector similarity. Raw chat is never embedded; after each task
  the brain extracts only durable facts (`learned`). Manage both from the
  ⚙ settings panel (manual entries, ChatGPT `conversations.json` import,
  search, forget).

## Layout

```text
core/            → agent loop, LLM (DeepSeek), STT/TTS + embeddings, tools,
                   vector memory, history, paths
src-tauri/       → Tauri shell: window, invoke commands, events, approvals
frontend/        → vanilla UI (window.__TAURI__ bridge only, no secrets)
shim/            → rootless WebKit path shim (C, LD_PRELOAD)
scripts/         → setup-linux.sh (webkit sysroot when sudo is unavailable)
```

Everything Comrade owns on disk lives in one folder ("comrade-agent home"):

```text
Linux/macOS:  $COMRADE_HOME, else $XDG_CONFIG_HOME/comrade-agent, else ~/.config/comrade-agent
Windows:      %APPDATA%/comrade-agent

comrade-agent/
  comrade-memory.db   SQLite vector memory (auto-moved from the old
                      ~/.local/share/com.comrade.desktop/ on first run)
  history.db          chat sessions + transcripts (☰ sidebar: browse, reopen, delete)
  brave-profile/      persistent browser profile (override: COMRADE_PROFILE_DIR)
  webview/            webview cache/storage (was ~/.local/share/com.comrade.desktop/)
  sysroot/ + shim/    vendored webkit (rootless Linux dev only)
```

Flow: UI → `send_message`/`voice_input` → classify intent → agent loop
(LLM → permission check → tool → verify) → streamed events → transcript saved
to history, facts distilled to memory → optional TTS reply.

Dangerous actions (writes, clicks, `sudo`/`rm -rf`/`git push`, OpenCode tasks)
raise a modal approval; deny-by-default on 30s timeout.

## Setup

```bash
cp .env.example .env   # fill in DEEPSEEK_API_KEY (+ OPENROUTER_API_KEY for voice/embeddings)
./scripts/setup-linux.sh   # webkit2gtk-4.1 (pacman, or vendored into the comrade-agent home w/o sudo)
cargo test -p comrade-core
cargo run -p comrade-desktop
```

Rootless WebKit: distro WebKitGTK hard-codes `/usr/lib/webkit2gtk-4.1` for its
helper processes (verified in the WebKit source — no env override exists in
distro builds). With sudo the setup script installs it normally; without sudo
it vendors the package into the comrade-agent home and the app re-execs itself
under `shim/webkit-path-shim.c` (LD_PRELOAD redirect) automatically. Nothing to
configure — `cargo run` just works either way.

Useful commands: `cargo check --workspace`, `cargo clippy --workspace`,
`cargo test -p comrade-core -- --ignored` (live DeepSeek/OpenRouter checks).

## Notes

- TTS defaults to `mistralai/voxtral-mini-tts-2603` (mp3) with local
  `espeak-ng` fallback; STT uses `openai/whisper-large-v3`. Override via `.env`.
- DeepSeek validates function names strictly (`^[a-zA-Z0-9_-]+$`), so dotted
  tool names go on the wire as `namespace_tool` and are decoded back.
- Browser tools are honest stubs until the automation phase.
- No system tray / wake-word yet — push-to-talk via the mic button.
