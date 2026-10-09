#!/usr/bin/env node
// Native Windows/macOS builders. Linux packaging lives in release-linux.mjs.
import { createHash } from 'node:crypto';
import { spawn, execFileSync } from 'node:child_process';
import {
  copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync,
  readdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const targetIndex = args.indexOf('--target');
const target = (targetIndex >= 0 ? args[targetIndex + 1] : null)
  || process.env.COMRADE_RELEASE_TARGET || process.env.TAURI_ENV_TARGET_TRIPLE;
const supported = {
  'x86_64-pc-windows-msvc': { platform: 'win32', arch: 'x86_64', label: 'windows', bundle: 'nsis', ext: '.exe' },
  'aarch64-apple-darwin': { platform: 'darwin', arch: 'arm64', label: 'macos', bundle: 'dmg', ext: '.dmg' },
  'x86_64-apple-darwin': { platform: 'darwin', arch: 'x86_64', label: 'macos', bundle: 'dmg', ext: '.dmg' },
};
const spec = supported[target];
const stage = join(root, 'target', 'release', 'runtime', 'desktop');
const release = target ? join(root, 'target', target, 'release') : null;
const generatedConfig = join(stage, 'bundle-config.json');
const notices = [
  ['ONNXRUNTIME-LICENSE', 'LICENSE', '2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c'],
  ['ONNXRUNTIME-ThirdPartyNotices.txt', 'ThirdPartyNotices.txt', '4f9e2bb7b4b407d710a68168615bbc6f70e3d7cc8ba9410fb6c65d92fa71accf'],
];

function fail(message) { throw new Error(message); }
function run(command, argv, options = {}) {
  return execFileSync(command, argv, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, ...options });
}
function tauri(argv, env = process.env) {
  // Direct Node entry point works in PowerShell, cmd.exe and Unix shells.
  run(process.execPath, [join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), ...argv], { env, stdio: 'inherit' });
}
function digest(path) { return createHash('sha256').update(readFileSync(path)).digest('hex'); }
function walk(path) {
  return readdirSync(path, { withFileTypes: true }).flatMap(entry => {
    const full = join(path, entry.name);
    return entry.isDirectory() ? walk(full) : [full];
  });
}
function binary() { return join(release, spec.platform === 'win32' ? 'comrade-desktop.exe' : 'comrade-desktop'); }
function assertNative() {
  if (!spec) fail('Use --target x86_64-pc-windows-msvc, aarch64-apple-darwin, or x86_64-apple-darwin.');
  if (spec.platform !== process.platform) fail('Windows and macOS releases require their respective native build hosts.');
  if (args.some((arg, i) => !['--target', '--stage', '--collect', '--smoke'].includes(arg) && i !== (targetIndex >= 0 ? targetIndex + 1 : -1))) fail('Unknown release argument.');
}

function peArchitecture(path) {
  const bytes = readFileSync(path);
  if (bytes.length < 64 || bytes.toString('ascii', 0, 2) !== 'MZ') fail(`Invalid PE binary: ${path}`);
  const offset = bytes.readUInt32LE(60);
  if (bytes.toString('ascii', offset, offset + 4) !== 'PE\0\0' || bytes.readUInt16LE(offset + 4) !== 0x8664) {
    fail(`Expected Windows x64 PE architecture: ${path}`);
  }
}
function dylibDependencies(path) {
  return run('otool', ['-L', path]).split('\n').slice(1)
    .map(line => line.trim().split(' (')[0]).filter(Boolean);
}
function assertArchitecture(path) {
  if (spec.platform === 'win32') peArchitecture(path);
  else {
    run('lipo', ['-verify_arch', spec.arch, path]);
    const info = run('otool', ['-arch', spec.arch, '-l', path]);
    const required = [...info.matchAll(/cmd LC_(?:BUILD_VERSION|VERSION_MIN_MACOSX)[\s\S]*?(?:minos|version)\s+([0-9.]+)/g)].map(match => match[1]);
    const configured = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.macos.conf.json'), 'utf8')).bundle.macOS.minimumSystemVersion;
    const later = (left, right) => {
      const a = left.split('.').map(Number), b = right.split('.').map(Number);
      for (let i = 0; i < Math.max(a.length, b.length); i++) {
        if ((a[i] || 0) !== (b[i] || 0)) return (a[i] || 0) > (b[i] || 0);
      }
      return false;
    };
    if (!required.length || required.some(version => later(version, configured))) fail(`Native minimum macOS version exceeds configured ${configured}: ${path}`);
  }
}

function windowsImports(path) {
  const bytes = readFileSync(path);
  const pe = bytes.readUInt32LE(60), optional = pe + 24;
  const sections = pe + 24 + bytes.readUInt16LE(pe + 20);
  const sectionCount = bytes.readUInt16LE(pe + 6);
  const fileOffset = rva => {
    for (let i = 0; i < sectionCount; i++) {
      const section = sections + i * 40;
      const start = bytes.readUInt32LE(section + 12);
      const size = Math.max(bytes.readUInt32LE(section + 8), bytes.readUInt32LE(section + 16));
      if (rva >= start && rva < start + size) return bytes.readUInt32LE(section + 20) + rva - start;
    }
    fail(`Invalid PE import table: ${path}`);
  };
  const importRva = bytes.readUInt32LE(optional + (bytes.readUInt16LE(optional) === 0x20b ? 112 : 96) + 8);
  if (!importRva) return [];
  let descriptor = fileOffset(importRva);
  const imports = [];
  while (descriptor + 20 <= bytes.length) {
    const nameRva = bytes.readUInt32LE(descriptor + 12);
    if (!nameRva) break;
    const name = fileOffset(nameRva), end = bytes.indexOf(0, name);
    if (end < 0) fail(`Invalid PE dependency name: ${path}`);
    imports.push(bytes.toString('ascii', name, end));
    descriptor += 20;
  }
  return imports;
}
function libraryNames() {
  const pattern = spec.platform === 'win32'
    ? /^(?:lib)?(?:sherpa-onnx|onnxruntime|portaudio)[\w.-]*\.dll$/i
    : /^lib(?:sherpa-onnx|onnxruntime)[\w.-]*\.dylib$/;
  const names = readdirSync(release).filter(name => pattern.test(name));
  if (!names.some(name => /sherpa-onnx-c-api/i.test(name)) || !names.some(name => /onnxruntime/i.test(name))) {
    fail('Native voice libraries are missing beside the release binary. sherpa-rs-sys must finish building first.');
  }
  return names;
}
function stageLicenses() {
  const licenses = join(stage, 'licenses');
  mkdirSync(licenses, { recursive: true });
  const cache = join(root, 'target', 'release-license-cache');
  mkdirSync(cache, { recursive: true });
  for (const [name, source, sha] of notices) {
    const path = join(cache, name);
    if (!existsSync(path)) run(spec.platform === 'win32' ? 'curl.exe' : 'curl', [
      '--fail', '--location', '--silent', '--show-error', '--retry', '2', '--max-time', '60',
      `https://raw.githubusercontent.com/microsoft/onnxruntime/v1.17.1/${source}`, '-o', path,
    ]);
    if (digest(path) !== sha) fail(`Pinned license checksum mismatch: ${name}`);
    copyFileSync(path, join(licenses, name));
  }
  const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
  const sherpa = lock.split('[[package]]').find(record => /^name = "sherpa-rs-sys"$/m.test(record));
  if (!sherpa || !/^version = "0\.6\.8"$/m.test(sherpa)) fail('Review native voice license pins after upgrading sherpa-rs-sys.');
  const registry = join(process.env.CARGO_HOME || join(homedir(), '.cargo'), 'registry', 'src');
  const license = readdirSync(registry).map(directory => join(registry, directory, 'sherpa-rs-sys-0.6.8', 'sherpa-onnx', 'LICENSE')).find(existsSync);
  if (!license || digest(license) !== 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30') fail('Missing or unexpected sherpa-onnx license.');
  copyFileSync(license, join(licenses, 'SHERPA-ONNX-LICENSE'));
  copyFileSync(join(root, 'LICENSE'), join(licenses, 'COMRADE-LICENSE'));
  copyFileSync(join(root, 'docs', 'RELEASE.md'), join(licenses, 'RELEASE.md'));
}
function stageRuntime() {
  if (!existsSync(binary())) fail('Build the native desktop executable before staging.');
  assertArchitecture(binary());
  const names = libraryNames();
  for (const name of names) assertArchitecture(join(release, name));
  const onnx = names.find(name => /^(?:lib)?onnxruntime(?:\.dll|(?:\.[0-9]+)*\.dylib)$/i.test(name));
  if (!onnx || !readFileSync(join(release, onnx)).includes(Buffer.from('1.17.1\0'))) {
    fail('Review ONNX Runtime 1.17.1 license pins for this native runtime.');
  }
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  for (const name of names) copyFileSync(join(release, name), join(stage, name));
  stageLicenses();
  const config = { bundle: {} };
  if (spec.platform === 'win32') {
    config.bundle.resources = Object.fromEntries(names.map(name => [join(stage, name), name]));
    for (const name of readdirSync(join(stage, 'licenses'))) config.bundle.resources[join(stage, 'licenses', name)] = `licenses/${name}`;
  } else {
    // Rebase both the executable and the staged libraries before signing.
    for (const path of [binary(), ...names.map(name => join(stage, name))]) {
      if (path !== binary()) run('install_name_tool', ['-id', `@rpath/${basename(path)}`, path]);
      for (const dependency of dylibDependencies(path)) {
        if (names.includes(basename(dependency)) && dependency !== `@rpath/${basename(dependency)}`) {
          run('install_name_tool', ['-change', dependency, `@rpath/${basename(dependency)}`, path]);
        }
      }
    }
    config.bundle.macOS = { frameworks: names.map(name => join(stage, name)), files: {} };
    for (const name of readdirSync(join(stage, 'licenses'))) config.bundle.macOS.files[`SharedSupport/licenses/${name}`] = join(stage, 'licenses', name);
  }
  writeFileSync(generatedConfig, `${JSON.stringify(config, null, 2)}\n`);
  console.log(`Staged ${names.length} ${spec.label} voice libraries with pinned licenses.`);
}

function verifyMacApp(app) {
  const contents = join(app, 'Contents');
  const executable = join(contents, 'MacOS', 'comrade-desktop');
  assertArchitecture(executable);
  for (const name of libraryNames()) {
    const path = join(contents, 'Frameworks', name);
    if (!existsSync(path)) fail(`macOS application is missing ${name}.`);
    assertArchitecture(path);
    for (const dependency of dylibDependencies(path)) {
      if (!dependency.startsWith('/System/Library/') && !dependency.startsWith('/usr/lib/') && !dependency.startsWith('@rpath/')) {
        fail(`Nonportable native dependency in ${name}: ${dependency}`);
      }
      if (dependency.startsWith('@rpath/') && !existsSync(join(contents, 'Frameworks', basename(dependency)))) fail(`Missing bundled macOS dependency: ${dependency}`);
    }
  }
  for (const dependency of dylibDependencies(executable)) {
    if (!dependency.startsWith('/System/Library/') && !dependency.startsWith('/usr/lib/') && !dependency.startsWith('@rpath/')) fail(`Nonportable executable dependency: ${dependency}`);
    if (dependency.startsWith('@rpath/') && !existsSync(join(contents, 'Frameworks', basename(dependency)))) fail(`Missing executable dependency: ${dependency}`);
  }
  for (const name of readdirSync(join(stage, 'licenses'))) {
    if (digest(join(contents, 'SharedSupport', 'licenses', name)) !== digest(join(stage, 'licenses', name))) fail(`Bundled license mismatch: ${name}`);
  }
  run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', app]);
}
function collect() {
  const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
  const output = join(release, 'distributions');
  mkdirSync(output, { recursive: true });
  const candidate = walk(join(release, 'bundle', spec.bundle)).find(path => path.endsWith(spec.ext));
  if (!candidate) fail(`No ${spec.bundle} installer was produced.`);
  const filename = `Comrade-${version}-${spec.label}-${spec.arch}${spec.platform === 'win32' ? '-setup' : ''}${spec.ext}`;
  const installer = join(output, filename);
  copyFileSync(candidate, installer);
  if (spec.platform === 'darwin') verifyMacApp(join(release, 'bundle', 'macos', 'Comrade.app'));
  const report = {
    version, target, platform: spec.label, architecture: spec.arch,
    signing: spec.platform === 'darwin' ? 'Ad-hoc; not notarized for public distribution.' : 'Unsigned internal testing installer.',
    native_libraries: libraryNames().map(name => ({ name, sha256_before_bundle: digest(join(stage, name)) })),
    installer: filename, installer_sha256: digest(installer),
    live_account_verification: 'Requires separate live testing with an account.',
  };
  const suffix = `${spec.label}-${spec.arch}`;
  writeFileSync(join(output, `SHA256SUMS-${suffix}.txt`), `${digest(installer)}  ${filename}\n`);
  writeFileSync(join(output, `BUILD-${suffix}.json`), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`Collected ${installer}`);
  return installer;
}

async function smoke(installer) {
  if (process.env.GITHUB_ACTIONS !== 'true') fail('--smoke installs or mounts packages only on disposable GitHub Actions runners.');
  const temp = mkdtempSync(join(tmpdir(), 'comrade-native-smoke-'));
  const env = { ...process.env, COMRADE_HOME: join(temp, 'state'), HOME: join(temp, 'home'),
    XDG_CONFIG_HOME: join(temp, 'config'), XDG_DATA_HOME: join(temp, 'data'), XDG_CACHE_HOME: join(temp, 'cache'),
    APPDATA: join(temp, 'appdata'), LOCALAPPDATA: join(temp, 'localappdata'), USERPROFILE: join(temp, 'home') };
  for (const path of Object.values(env).filter(value => typeof value === 'string' && value.startsWith(temp))) mkdirSync(path, { recursive: true });
  let executable;
  if (spec.platform === 'win32') {
    const install = join(temp, 'installed');
    run(installer, ['/S', `/D=${install}`], { timeout: 120000 });
    executable = join(install, 'comrade-desktop.exe');
    if (!existsSync(executable)) fail('NSIS installer did not create the expected executable.');
    assertArchitecture(executable);
    for (const name of libraryNames()) {
      if (digest(join(install, name)) !== digest(join(stage, name))) fail(`Installed DLL missing or changed: ${name}`);
      assertArchitecture(join(install, name));
    }
    for (const name of readdirSync(join(stage, 'licenses'))) {
      if (digest(join(install, 'licenses', name)) !== digest(join(stage, 'licenses', name))) fail(`Installed license missing or changed: ${name}`);
    }
    for (const path of [executable, ...libraryNames().map(name => join(install, name))]) {
      for (const imported of windowsImports(path)) {
        const local = join(install, imported);
        if (/^(?:msvcp|vcruntime)[\w.-]*\.dll$/i.test(imported) && !existsSync(local)) fail(`Visual C++ runtime was not bundled: ${imported}`);
        if (!existsSync(local) && !/^(?:api|ext)-ms-/i.test(imported)
          && !existsSync(join(process.env.SystemRoot || 'C:\\Windows', 'System32', imported))) fail(`Unresolved installed DLL dependency: ${imported}`);
      }
    }
  } else {
    const mounted = join(temp, 'dmg');
    mkdirSync(mounted);
    run('hdiutil', ['attach', installer, '-readonly', '-nobrowse', '-mountpoint', mounted]);
    const installed = join(temp, 'Comrade.app');
    try { cpSync(join(mounted, 'Comrade.app'), installed, { recursive: true, verbatimSymlinks: true }); }
    finally { run('hdiutil', ['detach', mounted]); }
    verifyMacApp(installed);
    executable = join(installed, 'Contents', 'MacOS', 'comrade-desktop');
  }
  const child = spawn(executable, [], { cwd: temp, env, stdio: ['ignore', 'pipe', 'pipe'] });
  const output = [];
  child.stdout.on('data', data => output.push(data));
  child.stderr.on('data', data => output.push(data));
  let failed;
  child.on('error', error => { failed = error; });
  let exited = false;
  child.on('exit', () => { exited = true; });
  await new Promise(done => setTimeout(done, 12000));
  const wasAlive = !failed && !exited;
  if (wasAlive) {
    if (spec.platform === 'win32') run('taskkill', ['/PID', String(child.pid), '/T', '/F']);
    else child.kill('SIGTERM');
  }
  const log = join(temp, 'launch.log');
  writeFileSync(log, Buffer.concat(output));
  if (!wasAlive) fail(`Packaged native startup failed; see ${log}: ${failed?.message || child.exitCode}`);
  if (walk(join(temp, 'state')).some(path => /[/\\](?:browser|models|voice-models)[/\\]/.test(path))) fail('Browser/model assets were provisioned before sign in.');
  const suffix = `${spec.label}-${spec.arch}`;
  const outputDir = dirname(installer);
  copyFileSync(log, join(outputDir, `startup-${suffix}.log`));
  writeFileSync(join(outputDir, `SMOKE-${suffix}.json`), `${JSON.stringify({ target, packaged_startup_seconds: 12,
    startup_stayed_running: true, isolated_user_state: true, browser_model_downloads_before_sign_in: false,
    native_dependency_architecture_verified: true, live_authenticated_workflows_verified: false }, null, 2)}\n`);
  console.log(`Packaged native startup stayed up 12 seconds with isolated state. Log: ${log}`);
}

try {
  assertNative();
  if (args.includes('--stage')) stageRuntime();
  else {
    if (!args.includes('--collect')) {
      const env = { ...process.env, COMRADE_RELEASE_TARGET: target };
      if (spec.platform === 'darwin') {
        env.MACOSX_DEPLOYMENT_TARGET = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.macos.conf.json'), 'utf8')).bundle.macOS.minimumSystemVersion;
      }
      tauri(['build', '--no-bundle', '--target', target, '--ci', '--', '--locked'], env);
      stageRuntime();
      tauri(['bundle', '--target', target, '--bundles', spec.bundle, '--config', generatedConfig, '--ci'], env);
    }
    const installer = collect();
    if (args.includes('--smoke')) await smoke(installer);
  }
} catch (error) {
  console.error(`Native release failed: ${error.message}`);
  process.exitCode = 1;
}
