# Browser rendering and performance

Comrade runs its isolated Chromium process alongside the Tauri shell. The
agent and the person use the same page and profile. The app's browser pane
receives a live CDP screencast over a Tauri channel. It does not poll PNG
screenshots after each interaction.

## What caused the lag

The previous UI waited 350–900 ms after input, batched wheel events for 120 ms,
and only displayed a new picture after requesting browser state and a PNG.
The backend opened a new WebSocket for every CDP command, repeatedly listed
targets and queried the viewport, and decoded/re-encoded screenshot base64.
Toolbar back/forward added a fixed 1.2-second wait. Chromium was launched with
GPU rendering disabled.

## Current path

- A persistent CDP connection routes concurrent replies by request id and
  broadcasts browser events. Target discovery and HTTP connections are reused.
- Chromium pushes JPEG frames when its page changes. The backend forwards
  them directly through Tauri's streaming IPC channel. Exported screenshots
  still use PNG; they are separate from the display path.
- The frontend holds at most one decoding frame and one pending frame. It
  acknowledges painted or discarded frames, preventing stale frames from
  piling up. Opening, closing, startup races, and resize are covered by tests.
- The browser viewport matches the pane's dimensions. Resizing changes the
  actual page layout and frame coordinate space.
- Pointer presses, releases, hover, dragging, wheel events, modifier keys,
  typing, and larger pastes use the Chromium input pipeline. Wheel events are
  coalesced over 16 ms and target nested elements under the pointer.
- Hiding the pane or backgrounding the window stops streaming. A visible pane
  reads only small URL/title metadata once per second. Input does not request
  screenshots; toolbar navigation returns without waiting for page assets.
- GPU acceleration follows Chromium's normal selection instead of being
  forcibly disabled.

## Comparison with OpenAI's published architecture

[ChatGPT's browser documentation](https://learn.chatgpt.com/docs/browser)
describes a shared embedded browser with an isolated profile and split view.
It does not document the current desktop app's rendering internals.

[OpenAI's Atlas/OWL engineering article](https://openai.com/index/building-chatgpt-atlas/)
describes a separate Chromium host communicating through Mojo, with native
macOS CALayer composition and forwarded renderer input. It does not supply a
Tauri-compatible OWL implementation. Comrade applies process isolation,
continuous rendering, synchronized geometry, and direct input forwarding.
Its CDP JPEG stream still incurs encoding and decoding costs; it is **not**
OWL's native shared GPU surface implementation. This also does not turn the
pane into a complete native browser UI (for example, browser-owned popups
are not native Tauri widgets).

## Browser-first workspace

Use **Make browser the main view** in the browser toolbar to give the browser
all available space. Hover **Chat** to reveal a floating chat panel; it closes
when the pointer leaves unless you are typing. Click **Chat** to keep the panel
open, or use its pin button to place chat beside the browser. Close the panel
with its X button. Browser-first and pinned-chat preferences restore on launch.
Chat drafts and the active browser stream survive layout changes.

The fullscreen toolbar button or **F11** uses Tauri's native window fullscreen.
**Escape** closes the chat panel first, then returns to the regular layout;
while fullscreen it exits fullscreen and restores the regular layout directly.
Closing the browser also exits fullscreen. Settings and action-approval dialogs
retain their own Escape behavior.

## Default ad and tracker blocking

Comrade uses Brave's [`adblock-rust`](https://github.com/brave/adblock-rust)
engine (`adblock` 0.13.3) with bundled **EasyList** and **EasyPrivacy**. Protection
is enabled even for existing preferences that omit the new setting. Change
**Settings → Browser → Block ads and trackers**, or set
`adblock_enabled = false` in the `[browser]` section of `comrade.conf`.
Saved settings apply without restarting Chromium; reload a page to restore
resources blocked before disabling protection.

The controlled tab enables CDP Fetch interception before navigating. Every
paused request is evaluated and either cancelled with `BlockedByClient` or
continued directly in the persistent transport; paused requests never use the
lossy screencast event queue. Request type, frame source, third-party rules,
and filter exceptions are passed to Brave's engine. Service-worker responses
are bypassed to keep those cached resources on the interception path.

An isolated content-script world applies site-specific and generic CSS hiding
rules, including dynamic class/id changes, while respecting cosmetic
exceptions. DOM checks are throttled, with bounded selector payloads. This
integration does not include Brave Shields features such as fingerprinting
protection, redirect resources, procedural cosmetics, or scriptlet injection;
CDP interception is scoped to the controlled page target, not browser-wide
worker or out-of-process iframe targets.

Validated cached filters live in `comrade-agent/adblock/`. Background refresh
checks hourly and downloads from the official EasyList HTTPS endpoints when
snapshots are older than four days. Downloads are size-limited and both lists
must validate before replacing the engine. Compilation runs off the browser
transport task. Offline or invalid downloads preserve the current engine and
bundled fallback. `COMRADE_ADBLOCK_UPDATE=0` disables background downloads for
local tests or managed offline deployments. Filter attribution and license
are in `core/assets/adblock/`.

## Verification

```bash
npm test
npm run test:ui
cargo test -p comrade-core --lib tools::
```

Live checks use only a local HTML fixture and a fresh isolated scratch
profile. Supply an existing Chromium executable, then run:

```bash
COMRADE_ADBLOCK_UPDATE=0 \
COMRADE_HOME=/tmp/comrade-browser-tests \
COMRADE_CHROMIUM_BIN=/absolute/path/to/chrome \
cargo test -p comrade-core --test browser_cdp live_ -- \
  --ignored --skip live_self_install --nocapture
```

The ad-block test proves that blocked script requests never reach its local
HTTP server, allowed scripts execute, dynamic ads are hidden, and disabling
then re-enabling protection takes effect. Engine tests cover filter exceptions
and request types; UI checks cover the default and saved setting.

The stream test verifies first-frame delivery, nested scrolling, trusted
clicks and typing, larger pastes, viewport changes, and stream restart. It
also reports control-transport overhead against the old reconnect-per-call
implementation on the same browser/page. Timing output is a local diagnostic,
not an end-to-end desktop or internet-page benchmark. UI integration tests
use a mock Tauri channel; the live Rust checks use real Chromium.

Rebuild the embedded frontend before rebuilding the executable:

```bash
npm run build
cargo build --release -p comrade-desktop
```
