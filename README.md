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
  comrade.conf          preferences (in-app browser pane, voice, coding agents)
  comrade-memory.db   SQLite vector memory (auto-moved from the old
                      ~/.local/share/com.comrade.desktop/ on first run)
  history.db          chat sessions + transcripts (☰ sidebar: browse, reopen, delete)
  browser/            Comrade's own bundled Chromium — installed automatically
                      on first launch / first browser use (one-time download,
                      progress in the in-app pane; override: COMRADE_CHROMIUM_BIN;
                      offline fallback: ./scripts/fetch-chromium.sh)
  browser-profile/    its single isolated profile (override: COMRADE_PROFILE_DIR)
  webview/            webview cache/storage (was ~/.local/share/com.comrade.desktop/)
  sysroot/ + shim/    vendored webkit (rootless Linux dev only)
```

Flow: UI → `send_message`/`voice_input` → classify intent → agent loop
(LLM → permission check → tool → verify) → streamed events → transcript saved
to history, facts distilled to memory → optional TTS reply.

Dangerous actions (writes, clicks, `sudo`/`rm -rf`/`git push`, OpenCode tasks)
raise a modal approval; deny-by-default on 30s timeout.

## Setup

Install the stable Rust toolchain first. On macOS, install Apple's command-line
tools and CMake; Tauri uses the system WebKit framework, so the Linux setup
script is not needed:

```bash
xcode-select --install  # skip if already installed
brew install cmake      # or provide cmake another way
cp .env.example .env
cargo test -p comrade-core
cargo run -p comrade-desktop
```

On Linux, install WebKitGTK and the native audio/build dependencies through the
setup script, then build and run:

```bash
cp .env.example .env   # DEEPSEEK_API_KEY + OPENROUTER_API_KEY (memory embeddings)
./scripts/setup-linux.sh   # apt/dnf/pacman; rootless WebKit fallback on Arch
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

- First launch shows onboarding: tick which coding agents Comrade may use
  (all enabled by default). Everything lives in `comrade.conf` and can be
  changed in Settings. The browser needs no setup — it is bundled.
- Coding tasks route to your default enabled agent (`coding.executeTask`):
  opencode, Claude Code, Codex, Copilot CLI, Qwen Code have real adapters;
  others appear for tracking until their CLIs support non-interactive runs.
- Voice is 100% local (sherpa-onnx runtime: streaming Zipformer-int8 STT,
  Silero VAD, Kokoro-82M-int8 TTS; cpal mic/speakers). Models download once to
  `<comrade-agent>/models/` (~185 MB), then work offline. Hold 🎙 to talk,
  ⟳ for hands-free conversation, talk over Comrade to interrupt. See
  `docs/VOICE.md` for architecture, models, tests, and troubleshooting.
- DeepSeek validates function names strictly (`^[a-zA-Z0-9_-]+$`), so dotted
  tool names go on the wire as `namespace_tool` and are decoded back.
- Browser: Comrade drives its own bundled Chromium
  (`comrade-agent/browser/`, single isolated profile — your system browsers
  are never touched). The browser is a core feature, not an add-on: the app
  installs it itself on first launch / first browser use (one-time download,
  live progress in the pane), so there is nothing to set up. It always runs
  headless, so nothing ever opens outside the app: the only visible surface
  is the resizable in-app browser pane (🌐 in the header, auto-shown on
  browser use), rendering live views of the same tab the agent drives. Drag
  the divider to resize; the address bar, back/forward/reload, and refresh
  controls drive that same tab. One tab is reused across navigation, reading,
  and clicking. If the self-install itself fails (e.g. offline), the task
  reports it honestly instead of opening another browser.
- No system tray / wake-word yet — push-to-talk via the mic button.
