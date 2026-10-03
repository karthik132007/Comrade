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

Comrade bundles the unmodified official **uBlock Origin Lite** extension
(version `2026.930.1227`) and enables **Complete** filtering by default, including
network rules, cosmetic filters, and the extension's own site scriptlets.
Current Chrome for Testing supports Manifest V3; full uBlock Origin uses
Manifest V2, so it cannot run in this browser. See the
[official Lite FAQ](https://github.com/uBlockOrigin/uBOL-home/wiki/Frequently-asked-questions-(FAQ))
for differences from full uBlock Origin and filtering modes.

The package is embedded in the executable, checksum-verified and extracted
offline into `comrade-agent/browser-extensions/`. It runs only in Comrade's
isolated Chromium profile. The driver loads it through Chromium's extension
API and waits for Complete-mode rules and document-start scriptlets before
opening a page. An already-running owned Chromium process can load the extension while
retaining its profile and active page; reload that page to apply scriptlets.

Change **Settings → Browser → Block ads and trackers**, or set
`adblock_enabled = false` in the `[browser]` section of `comrade.conf`.
Saved settings apply without restarting Chromium. **Reload the page after
changing protection**: scripts and styles already injected into a document
remain until navigation. Existing preferences that omit the setting default
to enabled. The previous custom Rust request interception and cosmetic
injection have been removed.

Filters and scriptlets ship with the pinned extension package; updates arrive
with Comrade's bundled-extension updates, rather than background list downloads.
Package source, checksum, and GPL attribution are in
[`core/assets/extensions/README.md`](../core/assets/extensions/README.md).
YouTube changes its ad delivery frequently; this integration does not promise
that every live ad variant is blocked.

## Verification

```bash
npm test
npm run test:ui
cargo test -p comrade-core --lib tools::
```

Live checks use only a local HTML fixture and a fresh isolated scratch
profile. Supply an existing Chromium executable, then run:

```bash
COMRADE_HOME=/tmp/comrade-browser-tests \
COMRADE_CHROMIUM_BIN=/absolute/path/to/chrome \
cargo test -p comrade-core --test browser_cdp live_ -- \
  --ignored --skip live_self_install --nocapture
```

The ad-block test proves that blocked script requests never reach its local
HTTP server, allowed scripts execute, dynamic ads are hidden, and disabling
then re-enabling protection takes effect after reload. A YouTube-origin fixture
checks the extension's actual fetch and XHR scriptlets: player ad fields are
removed while content stream URLs and video metadata survive. It runs with
protection on, off, and on again; no live YouTube playback is claimed. UI checks
cover the default and saved setting.

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
