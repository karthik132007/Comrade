#!/usr/bin/env node
// Explicit allowlist packaging: never copy .env, preferences or account state.
import { createHash } from 'node:crypto';
import {
  copyFileSync, chmodSync, existsSync, mkdirSync, readFileSync, readdirSync,
  realpathSync, rmSync, writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const release = join(root, 'target', 'release');
const stage = join(release, 'runtime', 'linux');
const licenses = join(stage, 'licenses');
const output = join(release, 'distributions');
const libraries = ['libsherpa-onnx-c-api.so', 'libonnxruntime.so'];
const noticeSources = [
  {
    name: 'ONNXRUNTIME-LICENSE',
    url: 'https://raw.githubusercontent.com/microsoft/onnxruntime/v1.17.1/LICENSE',
    sha256: '2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c',
  },
  {
    name: 'ONNXRUNTIME-ThirdPartyNotices.txt',
    url: 'https://raw.githubusercontent.com/microsoft/onnxruntime/v1.17.1/ThirdPartyNotices.txt',
    sha256: '4f9e2bb7b4b407d710a68168615bbc6f70e3d7cc8ba9410fb6c65d92fa71accf',
  },
];

function fail(message) { throw new Error(message); }
function command(binary, args, options = {}) {
  return execFileSync(binary, args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, ...options });
}
function digest(path) { return createHash('sha256').update(readFileSync(path)).digest('hex'); }
function needed(path) {
  return [...command('readelf', ['-d', path]).matchAll(/\(NEEDED\).*\[([^\]]+)\]/g)].map(match => match[1]);
}
function assertOriginRpath(path) {
  const dynamic = command('readelf', ['-d', path]);
  const paths = [...dynamic.matchAll(/\((?:RUNPATH|RPATH)\).*\[([^\]]+)\]/g)].flatMap(match => match[1].split(':'));
  if (!paths.length || paths.some(path => !path.startsWith('$ORIGIN'))) {
    fail(`Nonportable or missing runtime search path in ${path}. Build with npm run release:linux, which removes developer runtime paths.`);
  }
  return paths;
}

function fetchNotices() {
  const cache = join(root, 'target', 'release-license-cache');
  mkdirSync(cache, { recursive: true });
  for (const notice of noticeSources) {
    const path = join(cache, notice.name);
    if (!existsSync(path)) {
      console.log(`Fetching pinned public notice: ${notice.name}`);
      command('curl', ['--fail', '--location', '--silent', '--show-error', '--retry', '2', '--max-time', '60', notice.url, '-o', path]);
    }
    if (digest(path) !== notice.sha256) fail(`License checksum mismatch: ${path}. Remove the bad cache file and retry.`);
    copyFileSync(path, join(licenses, notice.name));
  }
  // The Cargo dependency carries the exact Apache license for sherpa-onnx.
  // Fail on an upgrade until the native library versions/notices are reviewed.
  const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
  const sherpa = lock.split('[[package]]').find(record => /^name = "sherpa-rs-sys"$/m.test(record));
  if (!sherpa || !/^version = "0\.6\.8"$/m.test(sherpa)) fail('Review the bundled voice licenses after changing sherpa-rs-sys.');
  const registry = join(process.env.CARGO_HOME || join(homedir(), '.cargo'), 'registry', 'src');
  const sherpaLicense = readdirSync(registry).map(directory => join(registry, directory, 'sherpa-rs-sys-0.6.8', 'sherpa-onnx', 'LICENSE'))
    .find(path => existsSync(path));
  if (!sherpaLicense || digest(sherpaLicense) !== 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30') {
    fail('Missing or unexpected sherpa-onnx license in Cargo registry.');
  }
  copyFileSync(sherpaLicense, join(licenses, 'SHERPA-ONNX-LICENSE'));
  for (const name of ['ESPEAK-NG-COPYING', 'THIRD_PARTY_NOTICES.md']) {
    copyFileSync(join(root, 'licenses', name), join(licenses, name));
  }
}

function stageRuntime() {
  if (process.platform !== 'linux') fail('This release command requires a Linux build host.');
  if (process.env.TAURI_ENV_DEBUG === 'true') fail('Release staging requires a release build, not --debug.');
  const nativeTarget = command('rustc', ['-vV']).match(/^host: (.+)$/m)?.[1];
  if (process.env.TAURI_ENV_TARGET_TRIPLE && process.env.TAURI_ENV_TARGET_TRIPLE !== nativeTarget) {
    fail('Cross compilation uses a different target layout; this release script currently supports native Linux builds.');
  }
  const binary = join(release, 'comrade-desktop');
  if (!existsSync(binary)) fail('Desktop release executable is missing. Build the native release before staging.');
  if (!assertOriginRpath(binary).includes('$ORIGIN/../lib/comrade-desktop')) {
    fail('The executable lacks its private Debian/portable library search path.');
  }
  for (const library of libraries) {
    if (!existsSync(join(release, library))) fail(`Missing ${library} in target/release. Build the desktop release first.`);
    assertOriginRpath(join(release, library));
  }
  if (!command('strings', [join(release, 'libonnxruntime.so')]).split('\n').includes('1.17.1')) {
    fail('Review ONNX Runtime license pins for the newly built runtime version.');
  }
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(licenses, { recursive: true });
  for (const library of libraries) copyFileSync(join(release, library), join(stage, library));
  fetchNotices();
  console.log(`Staged ${libraries.length} private voice libraries and their license notices.`);
}

function buildEnvironment() {
  const env = { ...process.env };
  delete env.RUSTFLAGS;
  // Encoded flags have higher priority than .cargo/config.toml rustflags.
  // The rootless development sysroot remains link-time-only.
  const appHome = env.COMRADE_HOME || join(env.XDG_CONFIG_HOME || join(homedir(), '.config'), 'comrade-agent');
  const linkDir = env.COMRADE_RELEASE_LINK_DIR || join(appHome, 'sysroot', 'usr', 'lib');
  const flags = ['-C', 'debuginfo=0'];
  if (existsSync(linkDir)) flags.push('-C', `link-arg=-L${resolve(linkDir)}`);
  env.CARGO_ENCODED_RUSTFLAGS = flags.join('\x1f');
  return env;
}

function nativeRequirements(binary) {
  const versionInfo = command('readelf', ['--version-info', binary]);
  const versions = [...new Set([...versionInfo.matchAll(/Name: (GLIBC_[0-9.]+)\s+Flags: (\S+)/g)]
    .filter(match => match[2] !== 'WEAK').map(match => match[1].replace('GLIBC_', '')))];
  versions.sort((left, right) => {
    const a = left.split('.').map(Number), b = right.split('.').map(Number);
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      if ((a[i] || 0) !== (b[i] || 0)) return (a[i] || 0) - (b[i] || 0);
    }
    return 0;
  });
  return { highest_required_glibc_symbol: versions.at(-1) || null, direct_shared_libraries: needed(binary) };
}

function portableLauncher() {
  return `#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd)

# System installations need no developer paths. On rootless installations,
# make the already installed WebKit libraries available to the ELF loader;
# the native app then re-execs with its existing helper-path shim.
if [ ! -f /usr/lib/webkit2gtk-4.1/WebKitNetworkProcess ]; then
  if [ -n "\${COMRADE_HOME:-}" ]; then
    app_home=$COMRADE_HOME
  elif [ -n "\${XDG_CONFIG_HOME:-}" ]; then
    app_home=$XDG_CONFIG_HOME/comrade-agent
  else
    app_home=$HOME/.config/comrade-agent
  fi
  if [ -n "\${COMRADE_WEBKIT_DIR:-}" ]; then
    webkit_dir=$COMRADE_WEBKIT_DIR
  elif [ -f "$app_home/sysroot/usr/lib/webkit2gtk-4.1/WebKitNetworkProcess" ]; then
    webkit_dir=$app_home/sysroot/usr/lib/webkit2gtk-4.1
  else
    webkit_dir=$HOME/comrade-sysroot/usr/lib/webkit2gtk-4.1
  fi
  if [ -n "\${COMRADE_SHIM:-}" ]; then
    shim=$COMRADE_SHIM
  elif [ -f "$app_home/shim/webkit-shim.so" ]; then
    shim=$app_home/shim/webkit-shim.so
  else
    shim=$HOME/.local/lib/comrade/webkit-shim.so
  fi
  if [ -f "$webkit_dir/WebKitNetworkProcess" ] && [ -f "$shim" ]; then
    webkit_lib=$(CDPATH= cd -P "$webkit_dir/.." && pwd)
    LD_LIBRARY_PATH="$webkit_lib\${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LD_LIBRARY_PATH
    export COMRADE_WEBKIT_DIR="$webkit_dir" COMRADE_SHIM="$shim"
  fi
fi
exec "$script_dir/bin/comrade-desktop" "$@"
`;
}

function verifyTree(tree) {
  const binary = join(tree, 'bin', 'comrade-desktop');
  const privateLib = join(tree, 'lib', 'comrade-desktop');
  assertOriginRpath(binary);
  for (const library of libraries) {
    const path = join(privateLib, library);
    if (!existsSync(path) || digest(path) !== digest(join(stage, library))) fail(`Missing or altered bundled ${library}.`);
    assertOriginRpath(path);
  }
  for (const path of [binary, ...libraries.map(library => join(privateLib, library))]) {
    const diagnostics = command('ldd', [path], { env: { ...process.env, LD_LIBRARY_PATH: '' } });
    for (const library of libraries.filter(library => needed(path).includes(library))) {
      const line = diagnostics.split('\n').find(line => line.includes(`${library} => `));
      const resolved = line?.match(/=>\s+(\S+)/)?.[1];
      if (!resolved || !existsSync(resolved) || realpathSync(resolved) !== realpathSync(join(privateLib, library))) {
        fail(`Loader did not resolve ${library} inside the unpacked package.`);
      }
    }
    // System WebKit/GTK/ALSA are intentionally not bundled. Record omissions
    // instead of silently borrowing the developer's private LD_LIBRARY_PATH.
    writeFileSync(join(output, `${tree.endsWith('usr') ? 'deb' : 'portable'}-${path === binary ? 'binary' : path.split('/').at(-1)}-dependencies.txt`), diagnostics);
  }
}

function collectPackages() {
  const config = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
  const version = config.version;
  const arch = command('uname', ['-m']).trim();
  const debArch = { x86_64: 'amd64', aarch64: 'arm64' }[arch];
  if (!debArch) fail(`Unsupported native architecture: ${arch}`);
  const originalDeb = join(release, 'bundle', 'deb', `${config.productName}_${version}_${debArch}.deb`);
  if (!existsSync(originalDeb)) fail(`Tauri Debian output was not produced: ${originalDeb}`);
  const binary = join(release, 'comrade-desktop');
  const rpaths = assertOriginRpath(binary);
  if (!rpaths.includes('$ORIGIN/../lib/comrade-desktop')) fail('The executable lacks its private Debian/portable library search path.');
  const strings = command('strings', [binary]);
  if (/sb_secret_[A-Za-z0-9_-]{15,}|sk-[A-Za-z0-9_-]{25,}/.test(strings)) fail('A secret-looking key was found in the executable. Do not distribute it.');
  mkdirSync(output, { recursive: true });
  const name = `Comrade-${version}-linux-${arch}`;
  const portable = join(output, name);
  rmSync(portable, { recursive: true, force: true });
  mkdirSync(join(portable, 'bin'), { recursive: true });
  mkdirSync(join(portable, 'lib', 'comrade-desktop'), { recursive: true });
  mkdirSync(join(portable, 'licenses'), { recursive: true });
  copyFileSync(binary, join(portable, 'bin', 'comrade-desktop'));
  chmodSync(join(portable, 'bin', 'comrade-desktop'), 0o755);
  writeFileSync(join(portable, 'comrade'), portableLauncher());
  chmodSync(join(portable, 'comrade'), 0o755);
  command('sh', ['-n', join(portable, 'comrade')]);
  for (const library of libraries) copyFileSync(join(stage, library), join(portable, 'lib', 'comrade-desktop', library));
  for (const license of readdirSync(licenses)) copyFileSync(join(licenses, license), join(portable, 'licenses', license));
  copyFileSync(join(root, 'LICENSE'), join(portable, 'LICENSE'));
  copyFileSync(join(root, 'docs', 'RELEASE.md'), join(portable, 'RELEASE.md'));
  const report = {
    version, architecture: arch,
    build_host: command('uname', ['-sr']).trim(),
    build_host_glibc: command('getconf', ['GNU_LIBC_VERSION']).trim(),
    build_host_distribution: readFileSync('/etc/os-release', 'utf8').match(/^PRETTY_NAME=(.*)$/m)?.[1] || 'unknown',
    compatibility: 'Internal native-host build. Older distribution compatibility has not been validated.',
    runtime_search_paths: rpaths,
    ...nativeRequirements(binary),
    bundled_voice_libraries: libraries.map(name => ({ name, sha256: digest(join(stage, name)) })),
    usage_mode: 'unlimited',
  };
  writeFileSync(join(portable, 'BUILD.json'), `${JSON.stringify(report, null, 2)}\n`);
  verifyTree(portable);
  const deb = join(output, `${name}.deb`);
  copyFileSync(originalDeb, deb);
  const extracted = join(output, 'deb-unpacked');
  rmSync(extracted, { recursive: true, force: true });
  command('dpkg-deb', ['--extract', deb, extracted]);
  verifyTree(join(extracted, 'usr'));
  for (const notice of ['SHERPA-ONNX-LICENSE', ...noticeSources.map(notice => notice.name)]) {
    if (!existsSync(join(extracted, 'usr', 'share', 'doc', 'comrade', 'third-party', notice))) fail(`Debian package is missing ${notice}.`);
  }
  const contents = command('dpkg-deb', ['--contents', deb]);
  if (/\/\.env(?:\s|$)|auth-session\.json|comrade\.conf|comrade-memory/.test(contents)) fail('User configuration or account state was included in the package.');
  writeFileSync(join(output, 'deb-contents.txt'), contents);
  writeFileSync(join(output, 'deb-control.txt'), command('dpkg-deb', ['--info', deb]));
  const archive = join(output, `${name}.tar.gz`);
  const epoch = process.env.SOURCE_DATE_EPOCH || command('git', ['log', '-1', '--format=%ct']).trim();
  if (!/^\d+$/.test(epoch)) fail('SOURCE_DATE_EPOCH must be an integer Unix timestamp.');
  command('tar', ['--sort=name', `--mtime=@${epoch}`, '--owner=0', '--group=0', '--numeric-owner', '-czf', archive, '-C', output, name]);
  const portableUnpacked = join(output, 'portable-unpacked');
  rmSync(portableUnpacked, { recursive: true, force: true });
  mkdirSync(portableUnpacked, { recursive: true });
  command('tar', ['-xzf', archive, '-C', portableUnpacked]);
  verifyTree(join(portableUnpacked, name));
  if (digest(join(portableUnpacked, name, 'comrade')) !== digest(join(portable, 'comrade'))) {
    fail('Archive launcher verification failed.');
  }
  for (const notice of readdirSync(licenses)) {
    const included = join(portableUnpacked, name, 'licenses', notice);
    if (!existsSync(included) || digest(included) !== digest(join(licenses, notice))) fail(`Archive license verification failed: ${notice}.`);
  }
  writeFileSync(join(output, 'SHA256SUMS'), [deb, archive].map(path => `${digest(path)}  ${path.split('/').at(-1)}`).join('\n') + '\n');
  writeFileSync(join(output, 'BUILD.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`Verified Debian and portable packages: ${output}`);
  console.log('Review BUILD.json and dependency reports before distribution. System WebKit/GTK/ALSA dependencies are required.');
}

try {
  const args = process.argv.slice(2);
  if (process.platform !== 'linux') fail('This release command requires Linux.');
  if (args.length === 1 && args[0] === '--stage') stageRuntime();
  else if (args.length === 1 && args[0] === '--collect') { stageRuntime(); collectPackages(); }
  else if (args.length === 0) {
    console.log('Building native Linux release with portable runtime search paths.');
    command('npm', ['run', 'tauri', '--', 'build', '--bundles', 'deb', '--ci', '--', '--locked'], { env: buildEnvironment(), stdio: 'inherit' });
    collectPackages();
  } else fail('Usage: node scripts/release-linux.mjs [--stage | --collect]');
} catch (error) {
  console.error(`Linux release failed: ${error.message}`);
  process.exitCode = 1;
}
