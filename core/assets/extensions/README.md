# Bundled uBlock Origin Lite

Comrade embeds the **unmodified official Chromium release** of uBlock Origin
Lite, version `2026.930.1227`.

- Release: https://github.com/uBlockOrigin/uBOL-home/releases/tag/2026.930.1227
- Package: https://github.com/uBlockOrigin/uBOL-home/releases/download/2026.930.1227/uBOLite_2026.930.1227.chromium.zip
- SHA-256: `13dd17bcc9720abbf5006793b0a4f7b672d34b394db2e6198daa7ef282b20361`
- Source: https://github.com/uBlockOrigin/uBOL-home and https://github.com/gorhill/uBlock/tree/master/platform/mv3
- License: GPL-3.0; the upstream `LICENSE.txt`, source files, and filter
  attribution remain inside the archive and its installed directory.

The archive is embedded in the Rust executable and verified before extraction
into `<comrade-home>/browser-extensions/ublock-origin-lite-2026.930.1227/`.
No network download is needed to install this extension. Comrade configures
Complete filtering using the extension's own options API; it does not modify
its manifest, rules, scripts, or source.

To update, obtain an official Chromium release, verify its release checksum,
replace the archive and pinned version/checksum in `browser_extension.rs`, and
run the package and live browser tests. Filters update with this package;
unpacked pinned releases do not automatically update from the Web Store.
