// Fixture tests for content-based Vulkan SDK detection. They build small fake SDK
// folders in a temporary directory and run the real PowerShell scripts against
// them: nothing is installed, downloaded or written outside that directory.
// The scripts are Windows-only, so the suite is skipped elsewhere.
import assert from 'node:assert/strict';
import { readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { makeSdk, pwsh, scratch } from './vulkan-sdk-fixtures.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const scripts = path.join(root, 'frontend', 'scripts');
const windows = process.platform === 'win32';
const options = { skip: !windows && 'The Vulkan SDK scripts run on Windows only' };
const PIN = '1.4.309.0';

const common = path.join(scripts, 'vulkan-sdk-common.ps1');
const testSdk = (folder, version = PIN) => {
  const result = pwsh(`. $env:COMMON; Test-VulkanSdk -Path $env:SDK -Version $env:PIN | ConvertTo-Json -Compress`, { COMMON: common, SDK: folder, PIN: version });
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
};

// Search roots pointed at the fixture folder so the real machine's folders are not consulted.
const isolatedEnv = base => ({
  CLAWSCRIBE_TOOLCHAIN_ROOT: path.join(base, 'toolchain'),
  RUNNER_TOOL_CACHE: path.join(base, 'tool-cache'),
  ProgramData: path.join(base, 'program-data'),
  CLAWSCRIBE_VULKAN_SDK_ROOT: path.join(base, 'drive', 'VulkanSDK'),
  VULKAN_SDK: '',
});
const find = (base, env) => {
  const result = pwsh(`. $env:COMMON; Find-VulkanSdk -Version $env:PIN | ConvertTo-Json -Compress -Depth 5`, { COMMON: common, PIN: PIN, ...isolatedEnv(base), ...env });
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
};

test('pwsh is available for the fixture tests', options, () => {
  assert.equal(pwsh('$PSVersionTable.PSVersion.Major').status, 0);
});

test('an SDK is accepted by its headers, library and tools under any folder name', options, () => {
  const base = scratch();
  try {
    const odd = makeSdk(path.join(base, 'not-the-version-at-all'));
    const result = testSdk(odd);
    assert.equal(result.Matches, true, result.Reason);
    assert.equal(result.HeaderVersion, '1.4.309');
    // The same content in a folder carrying a different version name is just as good.
    assert.equal(testSdk(makeSdk(path.join(base, '1.4.350.0'))).Matches, true);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('a wrong version is rejected even in a folder with the pinned name', options, () => {
  const base = scratch();
  try {
    const wrongPatch = testSdk(makeSdk(path.join(base, PIN), { patch: 350 }));
    assert.equal(wrongPatch.Matches, false);
    assert.match(wrongPatch.Reason, /headers are 1\.4\.350, not 1\.4\.309/);
    const wrongMinor = testSdk(makeSdk(path.join(base, 'a', PIN), { minor: 3 }));
    assert.equal(wrongMinor.Matches, false);
    assert.match(wrongMinor.Reason, /headers are 1\.3\.309/);
    // A tool of another version contradicts matching headers.
    const wrongTool = testSdk(makeSdk(path.join(base, 'b', PIN), { tool: true }));
    assert.equal(wrongTool.Matches, false);
    assert.match(wrongTool.Reason, /tool version is .+, not 1\.4\.309\.0/);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('missing or unusable parts are rejected with the reason', options, () => {
  const base = scratch();
  try {
    const cases = [
      [{ header: false }, /no readable Vulkan headers/],
      [{ library: false }, /no vulkan-1\.lib/],
      [{ library: 'empty' }, /no vulkan-1\.lib/],
      [{ bin: false }, /no Bin folder/],
    ];
    cases.forEach(([flags, reason], index) => {
      const result = testSdk(makeSdk(path.join(base, `case-${index}`), flags));
      assert.equal(result.Matches, false, JSON.stringify(flags));
      assert.match(result.Reason, reason);
    });
    const missing = testSdk(path.join(base, 'does-not-exist'));
    assert.equal(missing.Matches, false);
    assert.equal(missing.Reason, 'not a folder');
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('a persistent per-version tool folder is found even when VULKAN_SDK names another SDK', options, () => {
  const base = scratch();
  try {
    const wrong = makeSdk(path.join(base, 'inherited', '1.4.350.0'), { patch: 350 });
    makeSdk(path.join(base, 'toolchain', 'vulkan-sdk', PIN));
    const found = find(base, { VULKAN_SDK: wrong });
    assert.equal(found.Match.Label, 'per-version tool folder');
    assert.equal(found.Match.Matches, true);
    // The tool-cache and ProgramData locations are searched too.
    rmSync(path.join(base, 'toolchain'), { recursive: true, force: true });
    makeSdk(path.join(base, 'tool-cache', 'clawscribe', 'vulkan-sdk', PIN));
    assert.equal(find(base, { VULKAN_SDK: wrong }).Match.Label, 'per-version tool folder');
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('VULKAN_SDK and the default folder are used when their content matches', options, () => {
  const base = scratch();
  try {
    const inherited = makeSdk(path.join(base, 'whatever-name'));
    const viaEnvironment = find(base, { VULKAN_SDK: inherited });
    assert.equal(viaEnvironment.Match.Label, 'VULKAN_SDK (process)');
    const defaultFolder = makeSdk(path.join(base, 'drive', 'VulkanSDK', PIN));
    const viaDefault = find(base, { VULKAN_SDK: path.join(base, 'inherited', '1.4.350.0') });
    assert.ok(viaDefault.Match, 'a matching SDK is found');
    if (viaDefault.Match.Label === 'default SDK folder') assert.equal(viaDefault.Match.Path.toLowerCase(), defaultFolder.toLowerCase());
    else assert.match(viaDefault.Match.Label, /machine|user/);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('nothing is accepted when every candidate has the wrong content', options, () => {
  const base = scratch();
  try {
    const wrong = makeSdk(path.join(base, 'inherited', PIN), { patch: 350 });
    const found = find(base, { VULKAN_SDK: wrong });
    // Only a matching machine- or user-level VULKAN_SDK of the real host could be accepted here.
    if (found.Match) assert.match(found.Match.Label, /machine|user/);
    const processEntry = found.Checked.find(entry => entry.Label === 'VULKAN_SDK (process)');
    assert.equal(processEntry.Matches, false);
    assert.match(processEntry.Reason, /headers are 1\.4\.350/);
  } finally { rmSync(base, { recursive: true, force: true }); }
});

function runResolver(base, args, env) {
  const files = { env: path.join(base, 'github-env.txt'), path: path.join(base, 'github-path.txt'), output: path.join(base, 'github-output.txt'), summary: path.join(base, 'github-summary.txt') };
  for (const file of Object.values(files)) writeFileSync(file, '');
  const result = pwsh(`& (Join-Path $env:SCRIPTS 'resolve-vulkan-sdk.ps1') ${args}`, {
    SCRIPTS: scripts, GITHUB_ENV: files.env, GITHUB_PATH: files.path, GITHUB_OUTPUT: files.output, GITHUB_STEP_SUMMARY: files.summary, ...isolatedEnv(base), ...env,
  });
  return { result, env: readFileSync(files.env, 'utf8'), path: readFileSync(files.path, 'utf8'), output: readFileSync(files.output, 'utf8'), summary: existsSync(files.summary) ? readFileSync(files.summary, 'utf8') : '' };
}

test('the resolver exports the matching SDK and fails or defers when none matches', options, () => {
  const base = scratch();
  try {
    const sdk = makeSdk(path.join(base, 'any-name'));
    const ok = runResolver(base, '', { VULKAN_SDK: sdk });
    assert.equal(ok.result.status, 0, ok.result.stderr + ok.result.stdout);
    assert.match(ok.output, /found=true/);
    assert.match(ok.env, new RegExp(`VULKAN_SDK=${sdk.replace(/[\\]/g, '\\\\')}`));
    assert.match(ok.env, /Vulkan_LIBRARY=.*vulkan-1\.lib/);
    assert.match(ok.env, /Vulkan_INCLUDE_DIR=.*Include/);
    assert.match(ok.path, /Bin/);
    assert.match(ok.summary, /ACCEPT Vulkan SDK candidate \[VULKAN_SDK \(process\)\]/);

    const wrong = path.join(base, 'wrong', PIN);
    makeSdk(wrong, { patch: 350 });
    const refused = runResolver(base, '', { VULKAN_SDK: wrong });
    if (refused.result.status === 0) assert.match(refused.output, /found=true/, 'only a matching host-level SDK may be accepted');
    else {
      assert.match(refused.result.stdout + refused.result.stderr, /not provisioned/);
      assert.match(refused.summary, /REJECT Vulkan SDK candidate \[VULKAN_SDK \(process\)\].*headers are 1\.4\.350/);
      assert.equal(refused.env, '', 'nothing is exported when no SDK matches');
    }
    const deferred = runResolver(base, '-AllowMissing', { VULKAN_SDK: wrong });
    assert.equal(deferred.result.status, 0, deferred.result.stderr);
    if (!/found=true/.test(deferred.output)) {
      assert.match(deferred.output, /found=false/);
      assert.equal(deferred.env, '');
    }
  } finally { rmSync(base, { recursive: true, force: true }); }
});

test('the detection never consults the folder name', options, () => {
  const source = readFileSync(common, 'utf8');
  assert.ok(!/Split-Path -Leaf/.test(source));
  assert.match(source, /VK_HEADER_VERSION_COMPLETE/);
  assert.match(source, /vulkan-1\.lib/);
  assert.match(source, /vulkaninfoSDK\.exe/);
  assert.ok(existsSync(path.join(scripts, 'resolve-vulkan-sdk.ps1')));
});
