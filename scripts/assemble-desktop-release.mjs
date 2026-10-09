#!/usr/bin/env node
// Assemble only the five verified installers; never publish repository trees.
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import {
  copyFile, lstat, mkdir, mkdtemp, open, readFile, readdir, rename, rm,
} from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const linuxOrigin = 'https://akdqlmsktfkxzhvttgpi.supabase.co';
const linuxSourceCommit = '7ba6cfe7183cd88ffc9da13dd5ef654bf9e28be8';
const maxArtifactSize = 1024 * 1024 * 1024;
const platforms = [
  { platform: 'windows', architecture: 'x86_64', suffix: '-setup.exe' },
  { platform: 'macos', architecture: 'arm64', suffix: '.dmg' },
  { platform: 'macos', architecture: 'x86_64', suffix: '.dmg' },
];
function fail(message) { throw new Error(message); }
function validateVersion(version) {
  if (typeof version !== 'string' || !/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/.test(version)) fail('Invalid release version.');
}
function validateCommit(commit) {
  if (typeof commit !== 'string' || !/^[a-f0-9]{40}$/.test(commit)) fail('Expected a complete lowercase Git source commit.');
}
function validateTag(tag, version) {
  const prefix = `v${version}`;
  if (typeof tag !== 'string' || tag.length > 100 || (tag !== prefix && !(tag.startsWith(`${prefix}-`) && /^[A-Za-z0-9]+(?:[.-][A-Za-z0-9]+)*$/.test(tag.slice(prefix.length + 1))))) fail('RELEASE_TAG must match the package version and use a safe release suffix.');
}
function validateFilename(name) {
  if (typeof name !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,149}$/.test(name) || name.includes('..') || basename(name) !== name) fail('Unsafe release filename.');
}
function inside(parent, path) {
  const rel = relative(parent, path);
  return rel === '' || (rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}
async function listInput(input) {
  const files = [];
  async function visit(directory, depth) {
    if (depth > 12) fail('Artifact input nesting is too deep.');
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) fail('Artifact input must not contain symbolic links.');
      if (entry.isDirectory()) await visit(path, depth + 1);
      else if (entry.isFile()) files.push(path);
      else fail('Artifact input must contain only regular files and directories.');
      if (files.length > 10000) fail('Too many artifact input files.');
    }
  }
  const info = await lstat(input);
  if (info.isSymbolicLink() || !info.isDirectory()) fail('Artifact input must be a real directory.');
  await visit(input, 0);
  return files;
}
async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
async function packageFormat(path, name) {
  const file = await open(path, 'r');
  try {
    const { size } = await file.stat();
    if (!Number.isSafeInteger(size) || size < 1 || size > maxArtifactSize) fail(`Invalid artifact size: ${name}`);
    const header = Buffer.alloc(64);
    const { bytesRead } = await file.read(header, 0, 64, 0);
    if (name.endsWith('.deb')) {
      if (bytesRead < 8 || !header.subarray(0, 8).equals(Buffer.from('!<arch>\n'))) fail(`Invalid Debian package bytes: ${name}`);
    } else if (name.endsWith('.tar.gz')) {
      if (size < 18 || header[0] !== 0x1f || header[1] !== 0x8b || header[2] !== 8) fail(`Invalid gzip archive bytes: ${name}`);
    } else if (name.endsWith('.exe')) {
      if (bytesRead < 64 || header.toString('ascii', 0, 2) !== 'MZ') fail(`Invalid Windows installer bytes: ${name}`);
      const peOffset = header.readUInt32LE(60);
      if (peOffset < 64 || peOffset + 4 > size) fail(`Truncated Windows installer: ${name}`);
      const pe = Buffer.alloc(4);
      await file.read(pe, 0, 4, peOffset);
      if (!pe.equals(Buffer.from('PE\0\0'))) fail(`Invalid Windows PE header: ${name}`);
    } else if (name.endsWith('.dmg')) {
      if (size < 512) fail(`Truncated macOS disk image: ${name}`);
      const footer = Buffer.alloc(4);
      await file.read(footer, 0, 4, size - 512);
      if (footer.toString('ascii') !== 'koly') fail(`Invalid macOS disk-image footer: ${name}`);
    } else fail(`Unsupported release artifact: ${name}`);
    return size;
  } finally { await file.close(); }
}
function linuxItems(catalog, version) {
  if (!catalog || catalog.version !== version || !Array.isArray(catalog.downloads) || catalog.downloads.length !== 2) fail('Linux catalog must match the release version and contain exactly two downloads.');
  const expected = new Set([`Comrade-${version}-linux-x86_64.deb`, `Comrade-${version}-linux-x86_64.tar.gz`]);
  return catalog.downloads.map(item => {
    validateFilename(item.name);
    if (!expected.delete(item.name)) fail('Unexpected or duplicate Linux artifact.');
    const url = `${linuxOrigin}/storage/v1/object/public/comrade-releases/${version}/${item.name}`;
    if (item.url !== url) fail('Linux download URL must exactly match the pinned Supabase object.');
    if (!/^[a-f0-9]{64}$/.test(item.sha256) || !Number.isSafeInteger(item.size) || item.size < 1 || item.size > maxArtifactSize) fail('Invalid pinned Linux digest or size.');
    if (item.platform !== 'linux' || item.architecture !== 'x86_64' || item.sourceCommit !== linuxSourceCommit) fail('Invalid Linux provenance.');
    return { ...item };
  });
}
async function fetchLinux(item, target, fetchImpl) {
  const response = await fetchImpl(item.url, {
    redirect: 'error', signal: AbortSignal.timeout(300000),
    headers: { 'Accept-Encoding': 'identity' },
  });
  if (response.status !== 200 || response.redirected || (response.url && response.url !== item.url) || !response.body) fail(`Linux download failed or redirected: ${item.name}`);
  const length = response.headers.get('content-length');
  if (length !== null && (!/^\d+$/.test(length) || Number(length) !== item.size)) fail(`Linux download Content-Length mismatch: ${item.name}`);
  const hash = createHash('sha256');
  const file = await open(target, 'wx', 0o644);
  let size = 0;
  try {
    for await (const chunk of response.body) {
      size += chunk.length;
      if (size > item.size) fail(`Linux download exceeds pinned size: ${item.name}`);
      hash.update(chunk);
      await file.writeFile(chunk);
    }
  } finally { await file.close(); }
  if (size !== item.size || hash.digest('hex') !== item.sha256) fail(`Linux download checksum or size mismatch: ${item.name}`);
  await packageFormat(target, item.name);
}
async function nativeItem(spec, version, files, stage, sourceCommit) {
  const name = `Comrade-${version}-${spec.platform}-${spec.architecture}${spec.suffix}`;
  const sumsName = `SHA256SUMS-${spec.platform}-${spec.architecture}.txt`;
  const candidates = files.filter(path => basename(path) === name);
  const sums = files.filter(path => basename(path) === sumsName);
  if (candidates.length !== 1 || sums.length !== 1) fail(`Expected exactly one installer and matching checksum file: ${name}`);
  if (dirname(candidates[0]) !== dirname(sums[0])) fail(`Installer and checksum must belong to the same artifact: ${name}`);
  const checksumInfo = await lstat(sums[0]);
  if (!checksumInfo.isFile() || checksumInfo.size > 1024) fail(`Invalid checksum file: ${sumsName}`);
  const match = (await readFile(sums[0], 'utf8')).match(/^([a-f0-9]{64}) {2}([A-Za-z0-9._-]+)\r?\n?$/);
  if (!match || match[2] !== name) fail(`Checksum filename mismatch or invalid checksum record: ${sumsName}`);
  const size = await packageFormat(candidates[0], name);
  if (await digest(candidates[0]) !== match[1]) fail(`Native checksum mismatch: ${name}`);
  const target = join(stage, name);
  await copyFile(candidates[0], target);
  if (await digest(target) !== match[1] || await packageFormat(target, name) !== size) fail(`Copied native artifact changed: ${name}`);
  return { name, sha256: match[1], size, platform: spec.platform, architecture: spec.architecture, sourceCommit,
    signing: spec.platform === 'windows' ? 'unsigned-internal-testing' : 'ad-hoc-not-notarized' };
}

// Dependencies are injectable only for local tests; the CLI always loads its
// committed Linux pins and uses Node's HTTPS fetch implementation.
export async function assembleRelease({ inputDir, outputDir, version, tag, sourceCommit, linuxCatalog, fetchImpl = fetch }) {
  validateVersion(version); validateTag(tag, version); validateCommit(sourceCommit);
  const input = resolve(inputDir), output = resolve(outputDir);
  if (inside(input, output) || inside(output, input)) fail('Artifact input and release output must be separate directory trees.');
  try { await lstat(output); fail('Release output already exists; use a fresh output directory.'); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  const linux = linuxItems(linuxCatalog, version);
  const files = await listInput(input);
  await mkdir(dirname(output), { recursive: true });
  const stage = await mkdtemp(join(dirname(output), '.comrade-release-'));
  try {
    const downloads = [];
    for (const spec of platforms) downloads.push(await nativeItem(spec, version, files, stage, sourceCommit));
    for (const item of linux) {
      await fetchLinux(item, join(stage, item.name), fetchImpl);
      const { url: _oldUrl, ...metadata } = item;
      downloads.push(metadata);
    }
    downloads.sort((a, b) => a.name.localeCompare(b.name, 'en'));
    for (const item of downloads) {
      validateFilename(item.name);
      item.url = `https://github.com/karthik132007/Comrade/releases/download/${tag}/${item.name}`;
    }
    const manifest = { version, tag, sourceCommit, downloads };
    const sums = downloads.map(item => `${item.sha256}  ${item.name}`).join('\n') + '\n';
    const checksum = await open(join(stage, 'SHA256SUMS'), 'wx');
    await checksum.writeFile(sums); await checksum.close();
    const manifestFile = await open(join(stage, 'release-manifest.json'), 'wx');
    await manifestFile.writeFile(JSON.stringify(manifest, null, 2) + '\n'); await manifestFile.close();
    if ((await readdir(stage)).length !== 7) fail('Release output must contain exactly five installers and two metadata files.');
    await rename(stage, output);
    return manifest;
  } catch (error) { await rm(stage, { recursive: true, force: true }); throw error; }
}
async function main() {
  if (process.argv.length !== 2) fail('Usage: RELEASE_TAG=<tag> GITHUB_SHA=<commit> node scripts/assemble-desktop-release.mjs');
  const { version } = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  validateVersion(version);
  const linuxCatalog = JSON.parse(await readFile(join(root, 'releases', `linux-${version}.json`), 'utf8'));
  const manifest = await assembleRelease({
    inputDir: join(root, 'release-input'), outputDir: join(root, 'release-output'), version,
    tag: process.env.RELEASE_TAG, sourceCommit: process.env.GITHUB_SHA, linuxCatalog,
  });
  console.log(`Assembled ${manifest.downloads.length} verified installers for ${manifest.tag}.`);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(`Release assembly failed: ${error.message}`); process.exitCode = 1; });
}
