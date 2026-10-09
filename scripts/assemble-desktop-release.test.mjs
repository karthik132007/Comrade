import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { gzipSync } from 'node:zlib';
import { assembleRelease } from './assemble-desktop-release.mjs';

const version = '1.0.0';
const tag = 'v1.0.0-internal.42';
const commit = '1234567890123456789012345678901234567890';
const linuxCommit = 'aa5ef7446205d7ab565a15ecdf3f7fa56f38e714';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const nativeSpecs = [
  ['windows', 'x86_64', '-setup.exe'],
  ['macos', 'arm64', '.dmg'],
  ['macos', 'x86_64', '.dmg'],
];
function windows() {
  const bytes = Buffer.alloc(512); bytes.write('MZ'); bytes.writeUInt32LE(128, 60); bytes.write('PE\0\0', 128);
  return bytes;
}
function dmg() { const bytes = Buffer.alloc(1024); bytes.write('koly', bytes.length - 512); return bytes; }
async function fixture(t) {
  const base = await mkdtemp(join(tmpdir(), 'comrade-assembly-test-'));
  t.after(() => rm(base, { recursive: true, force: true }));
  const inputDir = join(base, 'release-input'), outputDir = join(base, 'release-output');
  const native = [];
  for (const [platform, architecture, suffix] of nativeSpecs) {
    const directory = join(inputDir, `desktop-${platform}-${architecture}`, 'nested');
    await mkdir(directory, { recursive: true });
    const name = `Comrade-${version}-${platform}-${architecture}${suffix}`;
    const path = join(directory, name), sumPath = join(directory, `SHA256SUMS-${platform}-${architecture}.txt`);
    const bytes = platform === 'windows' ? windows() : dmg();
    await writeFile(path, bytes); await writeFile(sumPath, `${hash(bytes)}  ${name}\n`);
    await writeFile(join(directory, `BUILD-${platform}-${architecture}.json`), '{}');
    native.push({ name, path, sumPath, bytes });
  }
  // Extras stay out of the output; only explicit installer names are copied.
  await writeFile(join(inputDir, '.env'), 'SECRET=must-not-publish');
  await writeFile(join(inputDir, 'source.zip'), Buffer.from('PK\x03\x04partial-zip'));
  const linuxBytes = new Map([
    [`Comrade-${version}-linux-x86_64.deb`, Buffer.from('!<arch>\nfixture-deb')],
    [`Comrade-${version}-linux-x86_64.tar.gz`, gzipSync(Buffer.from('fixture-portable-tar'))],
  ]);
  const linuxCatalog = { version, downloads: [...linuxBytes].map(([name, bytes]) => ({
    name, url: `https://akdqlmsktfkxzhvttgpi.supabase.co/storage/v1/object/public/comrade-releases/${version}-aa5ef7446205/${name}`,
    sha256: hash(bytes), size: bytes.length, platform: 'linux', architecture: 'x86_64', sourceCommit: linuxCommit,
  })) };
  const requests = [];
  const fetchImpl = async (url, options) => {
    requests.push(url); assert.equal(options.redirect, 'error'); assert.equal(options.headers.Authorization, undefined);
    assert.equal(options.headers.apikey, undefined);
    const name = new URL(url).pathname.split('/').at(-1); const bytes = linuxBytes.get(name);
    return new Response(bytes, { status: 200, headers: { 'Content-Length': String(bytes.length) } });
  };
  return { inputDir, outputDir, version, tag, sourceCommit: commit, linuxCatalog, fetchImpl, requests, native, linuxBytes, base };
}
async function absent(path) { await assert.rejects(readFile(path), { code: 'ENOENT' }); }

test('assembles exactly five byte-verified installers and preserves separate Linux source provenance', async t => {
  const f = await fixture(t); const manifest = await assembleRelease(f);
  assert.equal(manifest.version, version); assert.equal(manifest.tag, tag); assert.equal(manifest.sourceCommit, commit);
  assert.equal(manifest.downloads.length, 5); assert.equal(f.requests.length, 2);
  const names = await readdir(f.outputDir); assert.equal(names.length, 7);
  assert(!names.includes('.env')); assert(!names.includes('source.zip')); assert(!names.some(name => name.startsWith('BUILD-')));
  for (const item of manifest.downloads) {
    const bytes = await readFile(join(f.outputDir, item.name));
    assert.equal(hash(bytes), item.sha256); assert.equal(bytes.length, item.size);
    assert.equal(item.url, `https://github.com/karthik132007/Comrade/releases/download/${tag}/${item.name}`);
    assert.equal(item.sourceCommit, item.platform === 'linux' ? linuxCommit : commit);
  }
  const sums = (await readFile(join(f.outputDir, 'SHA256SUMS'), 'utf8')).trim().split('\n');
  assert.equal(sums.length, 5); for (const item of manifest.downloads) assert(sums.includes(`${item.sha256}  ${item.name}`));
  assert.deepEqual(JSON.parse(await readFile(join(f.outputDir, 'release-manifest.json'), 'utf8')), manifest);
});

test('tampered native installer is rejected before Linux downloads and leaves no partial release', async t => {
  const f = await fixture(t); const bad = windows(); bad[300] ^= 1;
  await writeFile(f.native[0].path, bad);
  await assert.rejects(assembleRelease(f), /Native checksum mismatch/);
  assert.equal(f.requests.length, 0); await absent(f.outputDir);
  assert(!(await readdir(f.base)).some(name => name.startsWith('.comrade-release-')));
});

test('missing native artifact cannot be replaced by a partial GitHub zip download', async t => {
  const f = await fixture(t); await rm(f.native[1].path);
  await writeFile(join(dirname(f.native[1].path), 'artifact.zip'), Buffer.from('PK\x03\x04truncated'));
  await assert.rejects(assembleRelease(f), /exactly one installer/); await absent(f.outputDir);
});

test('missing, mismatched and extra checksum records are rejected', async t => {
  for (const mode of ['missing', 'wrong-name', 'wrong-digest', 'extra-line', 'traversal']) {
    await t.test(mode, async t => {
      const f = await fixture(t); const native = f.native[0];
      if (mode === 'missing') await rm(native.sumPath);
      else if (mode === 'wrong-name') await writeFile(native.sumPath, `${hash(native.bytes)}  unrelated.exe\n`);
      else if (mode === 'wrong-digest') await writeFile(native.sumPath, `${'0'.repeat(64)}  ${native.name}\n`);
      else if (mode === 'traversal') await writeFile(native.sumPath, `${hash(native.bytes)}  ../../${native.name}\n`);
      else await writeFile(native.sumPath, `${hash(native.bytes)}  ${native.name}\n${hash(native.bytes)}  .env\n`);
      await assert.rejects(assembleRelease(f), /checksum|exactly one installer/i); await absent(f.outputDir);
    });
  }
});

test('renamed ZIP, truncated PE and missing DMG footer fail despite matching replacement checksums', async t => {
  for (const kind of ['zip-as-exe', 'truncated-pe', 'zip-as-dmg', 'partial-dmg']) {
    await t.test(kind, async t => {
      const f = await fixture(t); const index = kind.includes('dmg') ? 1 : 0; const native = f.native[index];
      let bytes;
      if (kind === 'truncated-pe') { bytes = windows().subarray(0, 80); }
      else if (kind === 'partial-dmg') bytes = dmg().subarray(0, 800);
      else { bytes = Buffer.alloc(1024); bytes.write('PK\x03\x04'); }
      await writeFile(native.path, bytes); await writeFile(native.sumPath, `${hash(bytes)}  ${native.name}\n`);
      await assert.rejects(assembleRelease(f), /Invalid Windows|Truncated Windows|disk.image footer/); await absent(f.outputDir);
    });
  }
});

test('duplicate platform artifacts and symbolic links are rejected', async t => {
  for (const mode of ['duplicate', 'symlink']) {
    await t.test(mode, async t => {
      const f = await fixture(t); const directory = join(f.inputDir, 'unexpected'); await mkdir(directory);
      if (mode === 'duplicate') await writeFile(join(directory, f.native[0].name), f.native[0].bytes);
      else await symlink(f.native[0].path, join(directory, 'linked-installer'));
      await assert.rejects(assembleRelease(f), /exactly one installer|symbolic links/); await absent(f.outputDir);
    });
  }
});

test('Linux download redirects, error pages, truncated streams and digest mismatches abort atomically', async t => {
  for (const mode of ['redirect', 'html', 'truncated', 'digest', 'wrong-length']) {
    await t.test(mode, async t => {
      const f = await fixture(t); f.fetchImpl = async () => {
        const bytes = f.linuxBytes.values().next().value;
        if (mode === 'redirect') return new Response(null, { status: 302, headers: { Location: 'https://foreign.invalid/file' } });
        if (mode === 'html') return new Response('<html>not a package</html>', { status: 200 });
        if (mode === 'truncated') return new Response(bytes.subarray(0, bytes.length - 1), { status: 200 });
        if (mode === 'wrong-length') return new Response(bytes, { status: 200, headers: { 'Content-Length': String(bytes.length + 1) } });
        const corrupt = Buffer.from(bytes); corrupt[corrupt.length - 1] ^= 1; return new Response(corrupt, { status: 200 });
      };
      await assert.rejects(assembleRelease(f), /failed or redirected|checksum or size mismatch|exceeds pinned size|Content-Length mismatch/);
      await absent(f.outputDir);
    });
  }
});

test('Linux bytes with matching test pins still need Debian/gzip magic', async t => {
  for (const suffix of ['.deb', '.tar.gz']) {
    await t.test(suffix, async t => {
      const f = await fixture(t); const item = f.linuxCatalog.downloads.find(item => item.name.endsWith(suffix));
      const bytes = Buffer.alloc(32, 0x41); item.sha256 = hash(bytes); item.size = bytes.length; f.linuxBytes.set(item.name, bytes);
      await assert.rejects(assembleRelease(f), /Invalid Debian|Invalid gzip/); await absent(f.outputDir);
    });
  }
});

test('release version/tag/commit, unsafe URLs and provenance fail before filesystem writes or fetch', async t => {
  const cases = [
    f => { f.version = '../1.0.0'; }, f => { f.tag = 'v1.0.0/../../main'; }, f => { f.tag = 'v2.0.0'; },
    f => { f.sourceCommit = 'main'; }, f => { f.linuxCatalog.downloads[0].name = '../secret'; },
    f => { f.linuxCatalog.downloads[0].url += '?token=private'; },
    f => { f.linuxCatalog.downloads[0].url = 'https://foreign.invalid/pkg.deb'; },
    f => { f.linuxCatalog.downloads[0].url = f.linuxCatalog.downloads[0].url.replace('1.0.0-aa5ef7446205/', '1.0.0/'); },
    f => { f.linuxCatalog.downloads[0].url = f.linuxCatalog.downloads[0].url.replace('aa5ef7446205', '7ba6cfe7183c'); },
    f => { f.linuxCatalog.downloads[0].sourceCommit = commit; },
    f => { f.linuxCatalog.downloads[0].sourceCommit = '7ba6cfe7183cd88ffc9da13dd5ef654bf9e28be8'; },
    f => { f.linuxCatalog.downloads[0].size = -1; },
  ];
  for (const change of cases) { const f = await fixture(t); change(f); await assert.rejects(assembleRelease(f)); assert.equal(f.requests.length, 0); await absent(f.outputDir); }
});

test('existing output and overlapping input/output are preserved and rejected', async t => {
  const f = await fixture(t); await mkdir(f.outputDir); await writeFile(join(f.outputDir, 'keep.txt'), 'keep');
  await assert.rejects(assembleRelease(f), /already exists/); assert.equal(await readFile(join(f.outputDir, 'keep.txt'), 'utf8'), 'keep');
  f.outputDir = join(f.inputDir, 'output'); await assert.rejects(assembleRelease(f), /separate directory trees/);
});

test('committed catalog retains the independently verified Linux SHA-256 and byte counts', async () => {
  const catalog = JSON.parse(await readFile(new URL('../releases/linux-1.0.0.json', import.meta.url), 'utf8'));
  assert.equal(catalog.version, version);
  assert.deepEqual(catalog.downloads.map(item => [item.name, item.sha256, item.size]), [
    ['Comrade-1.0.0-linux-x86_64.deb', 'eabb57246b3d06f72a33d21dce032104f7cf261cf6f1da26f3a10d5f0b2c13fc', 30525572],
    ['Comrade-1.0.0-linux-x86_64.tar.gz', 'f98d7b41c92f39294e7bb5bc76cf8f9bc9dbe96603a687c55c467c9f59defd85', 28821517],
  ]);
  for (const item of catalog.downloads) {
    assert.equal(item.sourceCommit, linuxCommit);
    assert.equal(item.url, `https://akdqlmsktfkxzhvttgpi.supabase.co/storage/v1/object/public/comrade-releases/1.0.0-aa5ef7446205/${item.name}`);
  }
});
