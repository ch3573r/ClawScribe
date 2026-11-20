// Fixture tests for the isolated Vulkan SDK staging: the extraction action may only
// write to a fresh folder, an unrelated inherited SDK must never be touched, and
// anything unexpected stops the run. Runs the real PowerShell script on fake SDK
// folders in a temporary directory; nothing is installed or downloaded.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { makeSdk, pwsh, scratch } from './vulkan-sdk-fixtures.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const script = path.join(root, 'frontend', 'scripts', 'stage-vulkan-sdk.ps1');
const options = { skip: process.platform !== 'win32' && 'The Vulkan SDK scripts run on Windows only' };
const PIN = '1.4.309.0';

const parseEnv = text => Object.fromEntries(text.split(/\r?\n/).filter(line => line.includes('=')).map(line => {
  const index = line.indexOf('=');
  return [line.slice(0, index), line.slice(index + 1)];
}));

// One workflow job: the environment file carries values from phase to phase.
function job(base, { inherited, root: toolchain = path.join(base, 'toolchain') } = {}) {
  const files = { env: path.join(base, 'github-env.txt'), path: path.join(base, 'github-path.txt') };
  writeFileSync(files.env, ''); writeFileSync(files.path, '');
  let carried = {};
  return {
    files,
    carried: () => carried,
    run(phase, extra = {}) {
      const result = pwsh(`& $env:STAGE -Phase ${phase}`, {
        STAGE: script, GITHUB_ENV: files.env, GITHUB_PATH: files.path,
        CLAWSCRIBE_TOOLCHAIN_ROOT: toolchain, RUNNER_TOOL_CACHE: '', VULKAN_SDK: inherited ?? '',
        ...carried, ...extra,
      });
      carried = { ...carried, ...parseEnv(readFileSync(files.env, 'utf8')) };
      return result;
    },
  };
}

const snapshot = folder => {
  const out = [];
  const walk = current => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full); else out.push(`${path.relative(folder, full)}:${readFileSync(full, 'utf8')}`);
    }
  };
  walk(folder);
  return out.sort().join('\n');
};

const stagingFolders = parent => (existsSync(parent) ? readdirSync(parent).filter(name => name.startsWith('.staging-')) : []);

test('staging gives the extraction action a fresh folder and leaves the inherited SDK alone', options, () => {
  const base = scratch();
  try {
    const inherited = makeSdk(path.join(base, 'system', '1.4.350.0'), { patch: 350 });
    const before = snapshot(inherited);
    const run = job(base, { inherited });
    const prepared = run.run('Prepare');
    assert.equal(prepared.status, 0, prepared.stderr + prepared.stdout);
    const env = run.carried();
    const parent = path.join(base, 'toolchain', 'vulkan-sdk');
    assert.match(path.basename(env.CLAWSCRIBE_VULKAN_STAGING), /^\.staging-[0-9a-f]{32}$/);
    assert.equal(realpathSync.native(path.dirname(env.CLAWSCRIBE_VULKAN_STAGING)), realpathSync.native(parent));
    assert.equal(path.normalize(env.VULKAN_SDK).toLowerCase(), path.normalize(env.CLAWSCRIBE_VULKAN_STAGING).toLowerCase(), 'the action is pointed at the staging folder');
    assert.equal(env.VULKAN_SDK_VERSION, PIN, 'the version override the action prefers is pinned');
    assert.equal(env.CLAWSCRIBE_INHERITED_VULKAN_SDK, inherited);
    assert.equal(existsSync(path.join(parent, PIN)), false, 'the per-version folder does not exist before promotion');
    assert.equal(snapshot(inherited), before);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('a verified staged SDK is moved into its per-version folder and exported', options, () => {
  const base = scratch();
  try {
    const inherited = makeSdk(path.join(base, 'system', '1.4.350.0'), { patch: 350 });
    const before = snapshot(inherited);
    const run = job(base, { inherited });
    assert.equal(run.run('Prepare').status, 0);
    // What the extraction action would do: fill the folder VULKAN_SDK names.
    makeSdk(path.normalize(run.carried().CLAWSCRIBE_VULKAN_STAGING));
    const promoted = run.run('Promote');
    assert.equal(promoted.status, 0, promoted.stderr + promoted.stdout);
    const parent = path.join(base, 'toolchain', 'vulkan-sdk');
    const final = path.join(parent, PIN);
    assert.ok(existsSync(path.join(final, 'Include', 'vulkan', 'vulkan_core.h')));
    assert.deepEqual(stagingFolders(parent), [], 'the staging folder is gone');
    const env = run.carried();
    assert.equal(realpathSync.native(env.VULKAN_SDK), realpathSync.native(final));
    assert.match(env.Vulkan_LIBRARY, /vulkan-1\.lib$/);
    assert.match(readFileSync(run.files.path, 'utf8'), /Bin/);
    assert.equal(snapshot(inherited), before, 'the inherited SDK is byte-for-byte unchanged');
    // A later run finds it by content, with the inherited SDK still the wrong one.
    const found = pwsh('. $env:COMMON; (Find-VulkanSdk -Version $env:PIN).Match.Label', {
      COMMON: path.join(root, 'frontend', 'scripts', 'vulkan-sdk-common.ps1'), PIN,
      CLAWSCRIBE_TOOLCHAIN_ROOT: path.join(base, 'toolchain'), VULKAN_SDK: inherited,
    });
    assert.equal(found.stdout.trim(), 'per-version tool folder');
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('promotion refuses an action that wrote somewhere else', options, () => {
  const base = scratch();
  try {
    const inherited = makeSdk(path.join(base, 'system', '1.4.350.0'), { patch: 350 });
    const run = job(base, { inherited });
    assert.equal(run.run('Prepare').status, 0);
    makeSdk(path.normalize(run.carried().CLAWSCRIBE_VULKAN_STAGING));
    // The action ignored the staging folder and reported the inherited one.
    const promoted = run.run('Promote', { VULKAN_SDK: inherited });
    assert.notEqual(promoted.status, 0);
    assert.match(promoted.stdout + promoted.stderr, /did not use the isolated staging folder/);
    assert.equal(existsSync(path.join(base, 'toolchain', 'vulkan-sdk', PIN)), false);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('promotion refuses a staged SDK that is not the pinned version', options, () => {
  const base = scratch();
  try {
    const run = job(base, { inherited: '' });
    assert.equal(run.run('Prepare').status, 0);
    makeSdk(path.normalize(run.carried().CLAWSCRIBE_VULKAN_STAGING), { patch: 350 });
    const promoted = run.run('Promote');
    assert.notEqual(promoted.status, 0);
    assert.match(promoted.stdout + promoted.stderr, /not the pinned SDK 1\.4\.309\.0 \(headers are 1\.4\.350/);
    assert.equal(existsSync(path.join(base, 'toolchain', 'vulkan-sdk', PIN)), false);
    // Cleanup removes only the owned staging folder.
    assert.equal(run.run('Clean').status, 0);
    assert.deepEqual(stagingFolders(path.join(base, 'toolchain', 'vulkan-sdk')), []);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('promotion refuses when the inherited SDK changed during staging', options, () => {
  const base = scratch();
  try {
    const inherited = makeSdk(path.join(base, 'system', '1.4.350.0'), { patch: 350 });
    const run = job(base, { inherited });
    assert.equal(run.run('Prepare').status, 0);
    makeSdk(path.normalize(run.carried().CLAWSCRIBE_VULKAN_STAGING));
    writeFileSync(path.join(inherited, 'Lib', 'vulkan-1.lib'), 'rewritten by something');
    const promoted = run.run('Promote');
    assert.notEqual(promoted.status, 0);
    assert.match(promoted.stdout + promoted.stderr, /inherited SDK changed/);
    assert.equal(existsSync(path.join(base, 'toolchain', 'vulkan-sdk', PIN)), false);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('an existing per-version folder with other content is never overwritten or deleted', options, () => {
  const base = scratch();
  try {
    const final = makeSdk(path.join(base, 'toolchain', 'vulkan-sdk', PIN), { patch: 350 });
    writeFileSync(path.join(final, 'keep.txt'), 'precious');
    const before = snapshot(final);
    const run = job(base, { inherited: '' });
    const prepared = run.run('Prepare');
    assert.notEqual(prepared.status, 0);
    assert.match(prepared.stdout + prepared.stderr, /Remove it manually/);
    assert.equal(snapshot(final), before);
    assert.deepEqual(stagingFolders(path.join(base, 'toolchain', 'vulkan-sdk')), []);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('a matching per-version folder is reused without staging anything', options, () => {
  const base = scratch();
  try {
    makeSdk(path.join(base, 'toolchain', 'vulkan-sdk', PIN));
    const run = job(base, { inherited: '' });
    assert.equal(run.run('Prepare').status, 0);
    assert.deepEqual(stagingFolders(path.join(base, 'toolchain', 'vulkan-sdk')), []);
    assert.equal(run.run('Promote').status, 0);
    assert.match(run.carried().VULKAN_SDK, new RegExp(`${PIN.replace(/\./g, '\\.')}$`));
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('staging refuses unsafe locations', options, () => {
  const base = scratch();
  try {
    // Inside the inherited SDK.
    const inherited = makeSdk(path.join(base, 'system', 'sdk'), { patch: 350 });
    const inside = job(base, { inherited, root: path.join(inherited, 'tool-folder') });
    const refused = inside.run('Prepare');
    assert.notEqual(refused.status, 0);
    assert.match(refused.stdout + refused.stderr, /inside the inherited VULKAN_SDK folder/);
    assert.equal(existsSync(path.join(inherited, 'tool-folder')), false);
    // A path with spaces would be split by the extraction action.
    const spaced = job(base, { inherited: '', root: path.join(base, 'with space') });
    const spacedResult = spaced.run('Prepare');
    assert.notEqual(spacedResult.status, 0);
    assert.match(spacedResult.stdout + spacedResult.stderr, /must not contain spaces/);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('cleanup only removes a staging folder it owns', options, () => {
  const base = scratch();
  try {
    const parent = path.join(base, 'toolchain', 'vulkan-sdk');
    const foreign = path.join(parent, 'not-a-staging-folder');
    mkdirSync(foreign, { recursive: true });
    writeFileSync(path.join(foreign, 'file.txt'), 'x');
    const outside = path.join(base, 'elsewhere');
    mkdirSync(outside, { recursive: true });
    const run = job(base, { inherited: '' });
    for (const target of [foreign, outside]) {
      const cleaned = run.run('Clean', { CLAWSCRIBE_VULKAN_STAGING: target });
      assert.equal(cleaned.status, 0, 'cleanup warns instead of failing');
      assert.ok(existsSync(target), `${target} is not removed`);
    }
  } finally { rmSync(base, { recursive: true, force: true }); }
});
