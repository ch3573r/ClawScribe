import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parsePublicKey, releaseAssetNames, verifyAssets, verifyDetached } from './verify-release-assets.mjs';

const script = fileURLToPath(new URL('./verify-release-assets.mjs', import.meta.url));
const version = '1.2.3';
const commit = 'a'.repeat(40);
const repo = 'example/clawscribe';
const names = releaseAssetNames(version);
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

function newKey() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519');
  const keyId = crypto.randomBytes(8);
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const publicKeyBase64 = Buffer.from(`untrusted comment: minisign public key\n${Buffer.concat([Buffer.from('Ed'), keyId, raw]).toString('base64')}\n`).toString('base64');
  return { privateKey, keyId, publicKeyBase64 };
}

function sign(body, { privateKey, keyId }, algo = 'ED', trusted = 'timestamp:1 file:test') {
  const message = algo === 'ED' ? crypto.createHash('blake2b512').update(body).digest() : body;
  const signature = crypto.sign(null, message, privateKey);
  const global = crypto.sign(null, Buffer.concat([signature, Buffer.from(trusted)]), privateKey);
  return Buffer.from(['untrusted comment: signature from test key', Buffer.concat([Buffer.from(algo), keyId, signature]).toString('base64'), `trusted comment: ${trusted}`, global.toString('base64'), ''].join('\n')).toString('base64');
}

function stage(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'verify-release-assets-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dir = path.join(root, 'assets');
  fs.mkdirSync(dir);
  const key = newKey();
  const files = { [names.setup]: crypto.randomBytes(4096), [names.msi]: crypto.randomBytes(4096) };
  const setupSig = sign(files[names.setup], key);
  const write = (name, content) => fs.writeFileSync(path.join(dir, name), content);
  for (const [name, bytes] of Object.entries(files)) write(name, bytes);
  write(names.setupSig, setupSig);
  write(names.msiSig, sign(files[names.msi], key));
  write('SHA256SUMS.txt', `${sha256(files[names.setup])}  nsis/${names.setup}\n${sha256(files[names.msi])}  msi/${names.msi}\n`);
  write('BUILD-METADATA.txt', `product=ClawScribe\nversion=${version}\ninstaller_product_version=${version}\nbuild_commit=${commit}\nbuild_commit_short=${commit.slice(0, 7)}\nbuild_date_utc=2026-01-01T00:00:00Z\n`);
  write('BUILD-METRICS.json', '{}\n');
  write('latest.json', JSON.stringify({ version, platforms: { 'windows-x86_64': { url: `https://github.com/${repo}/releases/download/v${version}/${names.setup}`, signature: setupSig } } }));
  const tauriConf = path.join(root, 'tauri.conf.json');
  fs.writeFileSync(tauriConf, JSON.stringify({ plugins: { updater: { pubkey: key.publicKeyBase64 } } }));
  const options = { dir, version, expectedCommit: commit, tauriConf, repo, minInstallerBytes: 1024 };
  return { root, dir, key, options, write, read: name => fs.readFileSync(path.join(dir, name)) };
}

const failedNames = outcome => outcome.failures.map(entry => entry.name);

test('detached signatures verify and reject tampering, foreign keys and unknown algorithms', () => {
  const key = newKey();
  const publicKey = parsePublicKey(key.publicKeyBase64);
  const body = crypto.randomBytes(5000);
  for (const algo of ['ED', 'Ed']) assert.equal(verifyDetached(body, sign(body, key, algo), publicKey).ok, true, algo);

  const tampered = Buffer.from(body);
  tampered[7] ^= 1;
  assert.equal(verifyDetached(tampered, sign(body, key), publicKey).ok, false);

  const signed = sign(body, key, 'ED', 'timestamp:1 file:real');
  const forgedComment = Buffer.from(Buffer.from(signed, 'base64').toString().replace('file:real', 'file:fake')).toString('base64');
  const forged = verifyDetached(body, forgedComment, publicKey);
  assert.equal(forged.ok, false);
  assert.equal(forged.fileOk, true);
  assert.equal(forged.globalOk, false);

  const other = newKey();
  assert.match(verifyDetached(body, sign(body, other), publicKey).why, /key id/);
  assert.match(verifyDetached(body, sign(body, key, 'XX'), publicKey).why, /unknown algorithm/);
  assert.throws(() => verifyDetached(body, Buffer.from('untrusted comment: x\n').toString('base64'), publicKey), /too few lines/);
  assert.throws(() => parsePublicKey(Buffer.from('not a key').toString('base64')), /public key layout/);
});

test('a complete release passes every check', t => {
  const staged = stage(t);
  const outcome = verifyAssets(staged.options);
  assert.deepEqual(failedNames(outcome), []);
  assert.equal(outcome.ok, true);
  assert.equal(outcome.failed, 0);
  assert.equal(outcome.checked, 28, "a check was added or dropped; update this count deliberately");
  assert.equal(outcome.signatureResults[names.setup].algo, 'ED');
});

test('a tampered installer fails its checksum and signature checks', t => {
  const staged = stage(t);
  const bytes = staged.read(names.setup);
  bytes[10] ^= 1;
  staged.write(names.setup, bytes);
  const failed = failedNames(verifyAssets(staged.options));
  assert.ok(failed.includes(`SHA256SUMS matches local hash of ${names.setup}`));
  assert.ok(failed.includes(`Ed25519 signature verifies against the pinned updater key: ${names.setup}`));
});

test('identity, manifest and asset-set mismatches are reported', t => {
  const staged = stage(t);
  assert.ok(failedNames(verifyAssets({ ...staged.options, expectedCommit: 'b'.repeat(40) })).includes('metadata build_commit equals expected commit'));
  assert.ok(failedNames(verifyAssets({ ...staged.options, version: '9.9.9' })).includes(`asset present: ${releaseAssetNames('9.9.9').setup}`));

  staged.write('latest.json', JSON.stringify({ version, platforms: { 'windows-x86_64': { url: 'https://example.invalid/setup.exe', signature: 'other' } } }));
  const manifest = failedNames(verifyAssets(staged.options));
  assert.ok(manifest.includes('latest.json installer URL names this version and installer'));
  assert.ok(manifest.includes('latest.json signature equals the setup .sig file'));

  staged.write('notes.txt', 'unexpected');
  assert.ok(failedNames(verifyAssets(staged.options)).includes('no unexpected extra assets'));
  fs.rmSync(path.join(staged.dir, 'notes.txt'));

  staged.write('VERIFICATION.json', '{}');
  assert.ok(!failedNames(verifyAssets(staged.options)).includes('no unexpected extra assets'));

  fs.rmSync(path.join(staged.dir, names.msi));
  const missing = verifyAssets(staged.options);
  assert.equal(missing.ok, false);
  assert.ok(failedNames(missing).includes(`asset present: ${names.msi}`));
});

test('GitHub digests and sizes must match the local files', t => {
  const staged = stage(t);
  const assets = fs.readdirSync(staged.dir).map(name => ({ name, state: 'uploaded', size: staged.read(name).length, digest: `sha256:${sha256(staged.read(name))}` }));
  const release = { draft: true, prerelease: false, assets };
  assert.deepEqual(failedNames(verifyAssets({ ...staged.options, release })), []);

  const wrongDigest = assets.map(asset => (asset.name === names.msi ? { ...asset, digest: `sha256:${'0'.repeat(64)}` } : asset));
  assert.ok(failedNames(verifyAssets({ ...staged.options, release: { ...release, assets: wrongDigest } })).includes(`GitHub digest matches local hash: ${names.msi}`));

  const partial = assets.filter(asset => asset.name !== 'latest.json');
  assert.ok(failedNames(verifyAssets({ ...staged.options, release: { ...release, assets: partial } })).includes('every required asset is uploaded to the release'));

  const unknown = [...assets, { name: 'surprise.zip', state: 'uploaded', size: 1, digest: 'sha256:00' }];
  assert.ok(failedNames(verifyAssets({ ...staged.options, release: { ...release, assets: unknown } })).includes('GitHub asset surprise.zip has a local copy'));
});

test('the command line exits 0 for a valid release, 1 for a failing one and 2 for bad input', t => {
  const staged = stage(t);
  const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });
  const base = ['--dir', staged.dir, '--version', version, '--commit', commit, '--tauri-conf', staged.options.tauriConf, '--repo', repo];

  // Synthetic installers are smaller than the real-size floor, which is the only failing check.
  const small = run(...base);
  assert.equal(small.status, 1);
  assert.deepEqual(JSON.parse(small.stdout).failures.map(entry => entry.name), [`installer is non-trivial in size: ${names.setup}`, `installer is non-trivial in size: ${names.msi}`]);

  assert.equal(run(...base.slice(0, 4)).status, 2);
  assert.equal(run('--dir').status, 2);
});
