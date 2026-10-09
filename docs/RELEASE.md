# Linux desktop release

Build the native desktop release with:

```sh
npm ci
npm run release:linux
```

Run from a Linux host with the Rust toolchain, a C/C++ compiler, CMake,
pkg-config, WebKitGTK 4.1 and ALSA development packages. The packaging command
also needs `curl`, `readelf`, `strings`, `ldd`, GNU `tar` and `dpkg-deb`. It uses
the committed Cargo/npm lockfiles. The first staging run retrieves the pinned
ONNX Runtime 1.17.1 license and third party notices; later runs verify and use
the copies cached in `target/release-license-cache`. Updating the native voice
dependencies requires reviewing those license/version pins.

The command overrides development Cargo runtime paths for this build. A
rootless development sysroot remains available as a link-time search path;
its absolute path is not embedded in the release. `COMRADE_RELEASE_LINK_DIR`
can explicitly select a different native library search directory. The
executable locates bundled voice libraries using `$ORIGIN` and
`$ORIGIN/../lib/comrade-desktop`. The command does not modify the developer's
`.cargo/config.toml`.

## Outputs

`target/release/distributions` contains:

- `Comrade-<version>-linux-<architecture>.deb`
- `Comrade-<version>-linux-<architecture>.tar.gz`
- `SHA256SUMS`, `BUILD.json`, and extracted dependency/package reports

The Debian package installs the application under `/usr/bin`, its two private
voice libraries under `/usr/lib/comrade-desktop`, and license notices under
`/usr/share/doc/comrade`. The portable archive has the same relative `bin` and
`lib/comrade-desktop` layout. Extract it and run `./comrade` from inside the
extracted folder. Keep the directory layout intact. Directly running
`./bin/comrade-desktop` is also supported with installed system WebKit.

The portable launcher can reuse an existing rootless WebKit installation
under the configured Comrade home (`sysroot` and `shim/webkit-shim.so`),
including the older home-directory fallback paths. It preserves existing
library paths and honors explicit `COMRADE_WEBKIT_DIR` and `COMRADE_SHIM`.
It does not download or bundle WebKit; the native app applies its existing
helper-path shim after startup when needed.

Both distributions require system WebKitGTK 4.1, GTK 3, ALSA, libstdc++, glibc,
and their normal transitive dependencies. Chromium, browser profiles, local
voice model downloads and developer sysroot libraries are not bundled.
Application provisioning downloads optional browser/model assets into the
user's own Comrade directory. An account is required before using the app;
hosted services also need an internet connection. Internal testing currently
has unlimited usage.

Only the executable, portable launcher, named voice libraries, and documentation/license files
are staged. Supabase secrets, provider keys, `.env`, preferences, account
sessions and user databases must never be included. The script checks the
package for private state filenames and executable strings for secret-shaped
keys. These checks supplement the explicit file allowlist.

## Verification and compatibility

The script unpacks the Debian package, verifies voice library checksums and
license files, checks origin-relative runtime paths, and records `ldd`
results for both distributions with the developer `LD_LIBRARY_PATH` cleared.
The private voice libraries must resolve from inside the extracted package.
Any missing system dependencies remain visible in the reports and must be
installed on the target machine. Verify the checksums before transferring:

```sh
cd target/release/distributions
sha256sum --check SHA256SUMS
```

Current artifacts are internal native-host builds produced on Arch Linux
with glibc 2.44. They have been checked on that build host; they are not an
Ubuntu/Debian compatibility baseline. `BUILD.json` records both the host libc
and the executable's highest required non-weak GLIBC symbol. A Debian file
format and dependency declarations alone do not establish compatibility with
older systems. Before a public Linux release, select the oldest supported
distribution, build there in CI, and run install/launch/account/browser/voice
checks on each supported target. The locally generated packages are unsigned.
This follows Tauri's [Linux compatibility guidance](https://v2.tauri.app/distribute/debian/#limitations).

Setting `SOURCE_DATE_EPOCH` controls archive timestamps. The script uses the
latest committed timestamp by default, stable ordering and numeric archive
ownership. This makes the archive layout repeatable; it does not claim that
native binaries built on different hosts are byte-identical.

After a manually completed Tauri build with the correct Cargo flags, run
`node scripts/release-linux.mjs --collect` to restage and verify existing
outputs. This mode refuses executables with absolute development runtime
paths. Do not use it to relabel an old binary as a new release.
