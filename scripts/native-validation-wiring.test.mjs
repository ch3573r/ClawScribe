// Static checks that keep the native validation wiring from regressing:
// the PR/main native test workflow, the packaged-binary acceptance step in the
// release workflow, and the isolation guarantees of the headless self-test.
// They read source text only; native execution happens on the designated runner.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { trustedPullRequestCondition, workflowPolicyErrors } from './verify-workflow-policy.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = relative => readFileSync(path.join(root, relative), 'utf8').replace(/\r\n/g, '\n');

const nativeWorkflow = read('.github/workflows/native-library-tests.yml');
const releaseWorkflow = read('.github/workflows/clawscribe-windows-release.yml');
const acceptanceScript = read('frontend/scripts/test-packaged-acceptance.ps1');
const selfTest = read('frontend/src-tauri/src/self_test.rs');

test('native library tests run for trusted pull requests and main pushes on the local runner', () => {
  assert.deepEqual(workflowPolicyErrors('native-library-tests.yml', nativeWorkflow), []);
  assert.match(nativeWorkflow, /^on:\n  pull_request:\n/m);
  assert.match(nativeWorkflow, /^  push:\n    branches:\n      - main\n/m);
  assert.ok(nativeWorkflow.includes(`if: ${trustedPullRequestCondition}`));
  assert.ok(!nativeWorkflow.includes('pull_request_target'));
  assert.match(nativeWorkflow, /build-windows-release\.ps1 -Feature windows-gpu -CheckOnly -NativeOnly/);
  assert.match(nativeWorkflow, /limit-native-test-cache\.ps1\n/);
  assert.match(nativeWorkflow, /limit-native-test-cache\.ps1 -NoFailOnLowSpace/);
  const cleanup = nativeWorkflow.slice(nativeWorkflow.indexOf('- name: Trim stale test executables'), nativeWorkflow.indexOf('- name: Generate native test summary'));
  assert.match(cleanup, /steps\.verify_runner\.outcome == 'success'/);
  assert.match(cleanup, /steps\.checkout\.outcome == 'success'/);
  // Dispatch needs an immutable reviewed commit, like the other manual validation.
  assert.match(nativeWorkflow, /\^\[0-9a-f\]\{40\}\$/);
});

test('the native-only preflight is reachable only with -CheckOnly and reuses the required groups', () => {
  const build = read('frontend/scripts/build-windows-release.ps1');
  assert.match(build, /\[switch\]\$NativeOnly/);
  assert.match(build, /if \(\$NativeOnly -and -not \$CheckOnly\) \{\n    throw/);
  assert.match(build, /if \(-not \$NativeOnly\) \{\n        pnpm exec tsc --noEmit/);
  assert.match(build, /test-windows-native\.ps1/);
  assert.match(read('frontend/scripts/test-windows-native.ps1'), /"self_test::tests"/);
});

test('release builds run packaged-binary acceptance after the native tests and before metadata', () => {
  const native = releaseWorkflow.indexOf('- name: Run native summary and resource regression tests');
  const acceptance = releaseWorkflow.indexOf('- name: Run packaged-binary acceptance');
  const metadata = releaseWorkflow.indexOf('- name: Write build metadata');
  assert.ok(native > 0 && acceptance > native && metadata > acceptance);
  const step = releaseWorkflow.slice(acceptance, metadata);
  assert.match(step, /if: \$\{\{ !inputs\['check-only'\] \}\}/);
  assert.match(step, /EXPECTED_BUILD_RUNNER: \$\{\{ vars\.CLAWSCRIBE_BUILD_RUNNER \}\}/);
  assert.match(step, /test-packaged-acceptance\.ps1 -Installer/);
  assert.ok(!step.includes('continue-on-error'));
});

test('packaged acceptance unpacks instead of installing and stays on the runner', () => {
  assert.match(acceptanceScript, /\$env:COMPUTERNAME -ine \$env:EXPECTED_BUILD_RUNNER/);
  assert.match(acceptanceScript, /\$env:RUNNER_NAME -ine \$env:EXPECTED_BUILD_RUNNER/);
  assert.match(acceptanceScript, /& \$sevenZip x -y "-o\$extract" -- \$installerPath/);
  // The installer is never executed, and nothing is stopped by name or pattern.
  assert.ok(!/&\s*\$installerPath/.test(acceptanceScript));
  assert.ok(!/Start-Process|msiexec|Stop-Process|taskkill|Get-Process/i.test(acceptanceScript));
  // No access to the user's own profile or to the network.
  assert.ok(!/APPDATA|LOCALAPPDATA|USERPROFILE|Invoke-WebRequest|Invoke-RestMethod|curl/i.test(acceptanceScript));
  assert.match(acceptanceScript, /'--self-test'|"--self-test"/);
  assert.match(acceptanceScript, /clawscribe-packaged-acceptance-\[0-9a-f\]\{32\}/);
});

test('the self-test entry point is wired before the desktop app starts', () => {
  const main = read('frontend/src-tauri/src/main.rs');
  assert.ok(main.indexOf('app_lib::self_test::dispatch()') > 0);
  assert.ok(main.indexOf('app_lib::self_test::dispatch()') < main.indexOf('app_lib::run()'));
  assert.match(read('frontend/src-tauri/src/lib.rs'), /^pub mod self_test;$/m);
});

test('the self-test cannot reach the real profile, credentials or providers', () => {
  const production = selfTest
    .slice(0, selfTest.indexOf('#[cfg(test)]'))
    .split('\n')
    .filter(line => !line.trim().startsWith('//'))
    .join('\n');
  // Data stays inside the explicit profile folder, which must pass the isolation check.
  assert.match(production, /fn reject_application_folders/);
  assert.match(production, /fn prepare_profile/);
  assert.match(production, /Phase::Run => \{\n\s+let mut entries/);
  for (const forbidden of [
    'platform_store', 'credentials::', 'get_template(', 'app_data_dir', 'tauri::', 'AppHandle',
    'download', 'keyring', 'Invoke-', 'std::env::var',
  ]) {
    assert.ok(!production.includes(forbidden), `self_test.rs must not use ${forbidden}`);
  }
  // The only network use is the loopback stub and the provider call to it.
  const urls = [...production.matchAll(/https?:\/\/[^\s"']+/g)].map(match => match[0]);
  assert.ok(urls.every(url => url.startsWith('http://{}') || url.startsWith('http://127.0.0.1')), urls.join(', '));
  assert.match(production, /127\.0\.0\.1:0/);
  // The shared lookup may download an encoder; the check requires the packaged one first.
  assert.match(production, /fn packaged_encoder/);
  assert.ok(production.indexOf('packaged_encoder()') < production.indexOf('recording_recovery(profile)'));
});
