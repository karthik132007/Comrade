#!/usr/bin/env bash
# Manual fallback for Comrade's self-installing bundled Chromium.
# Normally you never run this: the app downloads its own dedicated Chromium
# automatically on first launch / first browser use (see
# core/src/tools/provision.rs, progress in the in-app browser pane). Use this
# script only for offline/air-gapped machines: fetch the build elsewhere and
# unpack it into <comrade-agent>/browser/ yourself, or run this where the
# network works with COMRADE_HOME pointed at the target home.
#
# The agent drives ONLY this browser — never any system browser — and it is
# shown only inside the app (resizable in-app pane, live screenshots). The
# binary lands in <comrade-agent>/browser/ with an isolated profile next to
# it in <comrade-agent>/browser-profile/.
#
# Usage:
#   ./scripts/fetch-chromium.sh [--force]
#   CHROME_VERSION=151.0.7922.62 ./scripts/fetch-chromium.sh  # pin instead of Stable
#
# Layout picked up by core::paths::bundled_chromium_candidates():
#   <home>/browser/chrome-linux64/chrome   (Linux, full build)
#   <home>/browser/chrome-<platform>/...   (macOS/Windows, full build)
# Full `chrome` everywhere (not chrome-headless-shell: it crashes on startup
# in common container setups; full chrome --headless=new is the tested path).
set -euo pipefail

FORCE=0
if [ "${1:-}" = "--force" ]; then FORCE=1; fi

HOME_DIR="${COMRADE_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/comrade-agent}"
DEST="$HOME_DIR/browser"

case "$(uname -s)" in
  Linux)   PLATFORM="linux64" ;;
  Darwin)   PLATFORM="mac-arm64"; [ "$(uname -m)" = "x86_64" ] && PLATFORM="mac-x64" ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT) PLATFORM="win64" ;;
  *) echo "Unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

# Full `chrome` build on every platform.
ASSET="chrome"
if [ -x "$DEST/chrome" ] || [ -x "$DEST/chrome-linux64/chrome" ] || [ -d "$DEST/chrome-$PLATFORM" ]; then
  if [ "$FORCE" -eq 0 ]; then
    echo "Bundled Chromium already present under $DEST (use --force to re-fetch)."
    exit 0
  fi
fi

# Resolve the current Stable build unless pinned (same source as the in-app
# self-installer: Chrome-for-Testing release index).
if [ -z "${CHROME_VERSION:-}" ]; then
  INDEX="https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json"
  URL="$(curl -fSL --retry 3 "$INDEX" | python3 -c "
import json,sys
doc = json.load(sys.stdin)
for e in doc['channels']['Stable']['downloads'].get('$ASSET', []):
    if e['platform'] == '$PLATFORM':
        print(e['url']); break
")"
  CHROME_VERSION="$(echo "$URL" | sed -E 's#.*/([0-9]+\.[0-9]+\.[0-9]+\.[0-9]+)/.*#\1#')"
  echo "Resolved Stable Chromium $CHROME_VERSION for $PLATFORM."
else
  URL="https://storage.googleapis.com/chrome-for-testing-public/$CHROME_VERSION/$PLATFORM/$ASSET-$PLATFORM.zip"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Fetching $URL ..."
curl -fSL --retry 3 -o "$TMP/pkg.zip" "$URL"
mkdir -p "$DEST"
# Full build: unpack the whole directory (resources, locales, binary).
unzip -q -o "$TMP/pkg.zip" -d "$DEST"
BIN="$(find "$DEST" -maxdepth 3 \( -name chrome -o -name chrome.exe -o -name "Google Chrome for Testing" \) -type f | head -1)"
if [ -n "$BIN" ]; then
  chmod +x "$BIN"
  "$BIN" --version || true
  echo "Browser binary: $BIN"
fi
echo "Done: $DEST"
echo "Isolated profile lives at $HOME_DIR/browser-profile/ (created on first run)."
