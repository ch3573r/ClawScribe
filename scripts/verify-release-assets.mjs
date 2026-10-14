// Verifies the assets of a staged or published Windows release from a local directory.
//
//   node scripts/verify-release-assets.mjs --dir <assets> --version <x.y.z> --commit <sha> \
//     [--tauri-conf frontend/src-tauri/tauri.conf.json] [--repo <owner/name> --release-id <id>]
//
// Checks asset presence, SHA256SUMS, BUILD-METADATA identity, latest.json, and the detached
// Tauri updater signatures (Ed25519 over a BLAKE2b-512 prehash) against the public key pinned in
// tauri.conf.json. With --repo and --release-id it also compares the local files with the digest
// and size GitHub reports for every uploaded asset (requires an authenticated `gh`).
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');
const DEFAULT_MIN_INSTALLER_BYTES = 10 * 1024 * 1024;
// Added after verification, so they may be present on a published release but are never required.
const OPTIONAL_ASSETS = ['VERIFICATION.json', 'RELEASE-VERIFICATION.json', 'STABLE-PROMOTION.json'];

const sha256 = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');

export function releaseAssetNames(version) {
  return {
    setup: `ClawScribe_${version}_x64-setup.exe`,
    setupSig: `ClawScribe_${version}_x64-setup.exe.sig`,
    msi: `ClawScribe_${version}_x64_en-US.msi`,
    msiSig: `ClawScribe_${version}_x64_en-US.msi.sig`,
  };
}

// A minisign public key is base64 of "untrusted comment: ...\n<base64 of 'Ed' + key id(8) + key(32)>\n".
export function parsePublicKey(base64) {
  const lines = Buffer.from(base64, 'base64').toString('utf8').split(/\r?\n/).filter(Boolean);
  const raw = Buffer.from(lines[1] ?? '', 'base64');
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== 'Ed') throw new Error('unexpected public key layout');
  return {
    keyId: raw.subarray(2, 10),
    key: crypto.createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, raw.subarray(10)]), format: 'der', type: 'spki' }),
  };
}

// A minisign signature file is base64 of four lines: the untrusted comment, algorithm + key id +
// signature, the trusted comment, and a signature over (signature + trusted comment).
export function verifyDetached(fileBytes, signatureBase64, publicKey) {
  const lines = Buffer.from(signatureBase64.trim(), 'base64').toString('utf8').split(/\r?\n/).filter(Boolean);
  if (lines.length < 4) throw new Error('signature file has too few lines');
  const raw = Buffer.from(lines[1], 'base64');
  if (raw.length !== 74) throw new Error(`unexpected signature length ${raw.length}`);
  const algo = raw.subarray(0, 2).toString();
  const keyId = raw.subarray(2, 10);
  const signature = raw.subarray(10);
  if (algo !== 'ED' && algo !== 'Ed') return { ok: false, why: `unknown algorithm ${algo}` };
  if (!keyId.equals(publicKey.keyId)) return { ok: false, why: 'key id differs from the pinned updater key' };
  const message = algo === 'ED' ? crypto.createHash('blake2b512').update(fileBytes).digest() : fileBytes;
  const fileOk = crypto.verify(null, message, publicKey.key, signature);
  const trustedComment = lines[2].replace(/^trusted comment: ?/, '');
  const globalOk = crypto.verify(null, Buffer.concat([signature, Buffer.from(trustedComment)]), publicKey.key, Buffer.from(lines[3], 'base64'));
  return { ok: fileOk && globalOk, fileOk, globalOk, algo, trustedComment };
}

function parseKeyValueFile(text) {
  return Object.fromEntries(text.split(/\r?\n/).filter(line => line.includes('=')).map(line => [line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1)]));
}

// release: the parsed GitHub release object (GET /repos/{repo}/releases/{id}), or null to skip the
// GitHub comparison. Returns every check with its result; ok is true only when all of them pass.
export function verifyAssets({ dir, version, expectedCommit, tauriConf, repo, release = null, minInstallerBytes = DEFAULT_MIN_INSTALLER_BYTES }) {
  const checks = [];
  const check = (name, ok, detail = '') => { checks.push({ name, ok: Boolean(ok), detail }); };
  const names = releaseAssetNames(version);
  const required = ['BUILD-METADATA.txt', 'BUILD-METRICS.json', 'SHA256SUMS.txt', 'latest.json', ...Object.values(names)];
  const present = fs.readdirSync(dir);
  for (const name of required) check(`asset present: ${name}`, present.includes(name));
  const extras = present.filter(name => !required.includes(name) && !OPTIONAL_ASSETS.includes(name));
  check('no unexpected extra assets', extras.length === 0, extras.join(', '));

  const missing = required.filter(name => !present.includes(name));
  const digests = {};
  for (const name of present.filter(entry => required.includes(entry) || OPTIONAL_ASSETS.includes(entry))) digests[name] = sha256(path.join(dir, name));
  const result = () => {
    const failures = checks.filter(entry => !entry.ok);
    return { ok: failures.length === 0, version, expectedCommit, checked: checks.length, failed: failures.length, digests, signatureResults: {}, failures, checks };
  };
  if (missing.length) return result();

  const sums = fs.readFileSync(path.join(dir, 'SHA256SUMS.txt'), 'utf8').split(/\r?\n/).filter(Boolean)
    .map(line => { const [hash, file = ''] = line.split(/\s+/); return { hash: hash.toLowerCase(), base: path.basename(file.replace(/^\*/, '')) }; });
  for (const installer of [names.setup, names.msi]) {
    const entry = sums.find(sum => sum.base === installer);
    check(`SHA256SUMS lists ${installer}`, Boolean(entry));
    check(`SHA256SUMS matches local hash of ${installer}`, entry && entry.hash === digests[installer]);
  }
  check('SHA256SUMS lists exactly the two installers', sums.length === 2);

  if (release) {
    const uploaded = new Map(release.assets.map(asset => [asset.name, asset]));
    for (const asset of release.assets) {
      if (!digests[asset.name]) { check(`GitHub asset ${asset.name} has a local copy`, false); continue; }
      check(`GitHub digest matches local hash: ${asset.name}`, asset.digest === `sha256:${digests[asset.name]}`, asset.digest || 'no digest reported');
      check(`GitHub asset state uploaded with the local size: ${asset.name}`, asset.state === 'uploaded' && asset.size === fs.statSync(path.join(dir, asset.name)).size);
    }
    check('every required asset is uploaded to the release', required.every(name => uploaded.has(name)), `${release.assets.length} assets; draft=${release.draft} prerelease=${release.prerelease}`);
  }

  const meta = parseKeyValueFile(fs.readFileSync(path.join(dir, 'BUILD-METADATA.txt'), 'utf8'));
  check('metadata product is ClawScribe', meta.product === 'ClawScribe');
  check('metadata version', meta.version === version, meta.version);
  check('metadata installer_product_version', meta.installer_product_version === version, meta.installer_product_version);
  check('metadata build_commit equals expected commit', meta.build_commit === expectedCommit, meta.build_commit);
  check('metadata build_commit_short', meta.build_commit_short === expectedCommit.slice(0, 7), meta.build_commit_short);
  try { JSON.parse(fs.readFileSync(path.join(dir, 'BUILD-METRICS.json'), 'utf8')); check('BUILD-METRICS.json parses', true); } catch (error) { check('BUILD-METRICS.json parses', false, error.message); }

  const latest = JSON.parse(fs.readFileSync(path.join(dir, 'latest.json'), 'utf8'));
  const platform = latest.platforms?.['windows-x86_64'];
  check('latest.json version', latest.version === version, latest.version);
  check('latest.json has exactly windows-x86_64', Object.keys(latest.platforms || {}).join() === 'windows-x86_64');
  check('latest.json installer URL names this version and installer', platform?.url === `https://github.com/${repo}/releases/download/v${version}/${names.setup}`, platform?.url);
  check('latest.json signature equals the setup .sig file', platform?.signature?.trim() === fs.readFileSync(path.join(dir, names.setupSig), 'utf8').trim());

  const publicKey = parsePublicKey(JSON.parse(fs.readFileSync(tauriConf, 'utf8')).plugins.updater.pubkey);
  const signatureResults = {};
  for (const [file, signatureFile] of [[names.setup, names.setupSig], [names.msi, names.msiSig]]) {
    try {
      const outcome = verifyDetached(fs.readFileSync(path.join(dir, file)), fs.readFileSync(path.join(dir, signatureFile), 'utf8'), publicKey);
      signatureResults[file] = { ok: outcome.ok, algo: outcome.algo, trustedComment: outcome.trustedComment };
      check(`Ed25519 signature verifies against the pinned updater key: ${file}`, outcome.ok, outcome.why || `${outcome.algo}; trusted comment: ${outcome.trustedComment}`);
    } catch (error) {
      check(`signature check ran: ${file}`, false, error.message);
    }
  }
  for (const installer of [names.setup, names.msi]) check(`installer is non-trivial in size: ${installer}`, fs.statSync(path.join(dir, installer)).size > minInstallerBytes);

  return { ...result(), signatureResults, buildDateHint: meta.build_date_utc };
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    if (!flag.startsWith('--') || argv[index + 1] === undefined) throw new Error(`Unexpected argument: ${flag}`);
    options[flag.slice(2)] = argv[index + 1];
  }
  return options;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  for (const required of ['dir', 'version', 'commit']) if (!options[required]) throw new Error(`Missing --${required}`);
  const repo = options.repo || process.env.GITHUB_REPOSITORY;
  if (!repo) throw new Error('Missing --repo (or GITHUB_REPOSITORY)');
  const release = options['release-id']
    ? JSON.parse(execFileSync('gh', ['api', `repos/${repo}/releases/${options['release-id']}`], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 }))
    : null;
  const outcome = verifyAssets({
    dir: options.dir,
    version: options.version,
    expectedCommit: options.commit,
    tauriConf: options['tauri-conf'] || 'frontend/src-tauri/tauri.conf.json',
    repo,
    release,
  });
  console.log(JSON.stringify(outcome, null, 2));
  process.exitCode = outcome.ok ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 2; }
}
