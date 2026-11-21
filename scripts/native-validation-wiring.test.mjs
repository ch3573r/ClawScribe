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
const toolchainScript = read('frontend/scripts/ensure-native-toolchain.ps1');
const cacheScript = read('frontend/scripts/limit-native-test-cache.ps1');
const buildScript = read('frontend/scripts/build-windows-release.ps1');
const selfTest = read('frontend/src-tauri/src/self_test.rs');
// Production code only: everything before the unit-test module.
const selfTestProduction = selfTest
  .slice(0, selfTest.indexOf('#[cfg(test)]\nmod tests'))
  .split('\n')
  .filter(line => !line.trim().startsWith('//'))
  .join('\n');

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
  assert.match(read('frontend/scripts/test-windows-native.ps1'), /self_test::tests::/);
});

test('normal validation builds and lints the frontend and runs the unfiltered native library suite', () => {
  const validation = read('.github/workflows/pr-main-check.yml');
  assert.match(validation, /- name: Lint frontend\n +working-directory: frontend\n +run: pnpm run lint/);
  assert.match(validation, /- name: Build frontend\n +if: .*github.event_name != 'workflow_dispatch'/);
  const native = read('frontend/scripts/test-windows-native.ps1');
  assert.match(native, /& \$testExecutable --test-threads=1\n/);
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
  const production = selfTestProduction;
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

// ---- native workflow: toolchain, frontend assets, permissions -------------------------

const stepBlock = (source, name) => {
  const start = source.indexOf(`- name: ${name}`);
  assert.ok(start >= 0, `missing step: ${name}`);
  const next = source.indexOf('\n      - name:', start + 1);
  return source.slice(start, next < 0 ? undefined : next);
};

test('the native workflow verifies the pinned toolchain instead of installing it on every run', () => {
  assert.ok(!/choco\s+install/i.test(nativeWorkflow), 'no package installs in the workflow');
  assert.ok(!nativeWorkflow.includes('dtolnay/rust-toolchain'), 'no per-run Rust install');
  assert.ok(!/--allow-downgrade/.test(nativeWorkflow));
  const toolchain = stepBlock(nativeWorkflow, 'Verify the pinned native toolchain');
  assert.match(toolchain, /id: toolchain/);
  assert.match(toolchain, /PROVISION: \$\{\{ github\.event_name == 'workflow_dispatch' && inputs\['provision-toolchain'\] \}\}/);
  assert.match(toolchain, /if \(\$env:PROVISION -eq 'true'\) \{\n\s+\.\/frontend\/scripts\/ensure-native-toolchain\.ps1 -Provision -AllowMissingVulkanSdk\n\s+\} else \{\n\s+\.\/frontend\/scripts\/ensure-native-toolchain\.ps1\n\s+\}/);
  // The only way to install anything is the manual input, which defaults to off.
  assert.match(nativeWorkflow, /provision-toolchain:\n\s+description: [^\n]+\n\s+type: boolean\n\s+default: false/);
  assert.equal([...nativeWorkflow.matchAll(/-Provision\b/g)].length, 1);
  // The Vulkan SDK action runs only when the check found none (manual provisioning).
  const stage = stepBlock(nativeWorkflow, 'Stage the pinned Vulkan SDK');
  assert.match(stage, /if: \$\{\{ steps\.toolchain\.outputs\.vulkan_sdk == 'missing' \}\}/);
  assert.match(stage, /uses: \.\/\.github\/actions\/vulkan-sdk/);
  // The runtime installer is reachable only through the manual branch.
  const loader = stepBlock(nativeWorkflow, 'Verify the Vulkan loader');
  assert.equal([...loader.matchAll(/-InstallFromSdk/g)].length, 1);
  assert.match(loader, /if \(\$env:PROVISION -eq 'true'\) \{\n\s+\.\/frontend\/scripts\/ensure-windows-vulkan-runtime\.ps1 -InstallFromSdk/);
  // Verification happens before anything is compiled.
  assert.ok(nativeWorkflow.indexOf('ensure-native-toolchain.ps1') < nativeWorkflow.indexOf('Build llama-helper sidecar'));
  assert.ok(nativeWorkflow.indexOf('Verify the Vulkan loader') < nativeWorkflow.indexOf('Build llama-helper sidecar'));
});

test('the native workflow stays read-only and never publishes, tags or uploads', () => {
  assert.match(nativeWorkflow, /^permissions:\n  contents: read\n/m);
  assert.ok(!/contents:\s*write|packages:|id-token:|pull-requests:\s*write/.test(nativeWorkflow));
  assert.ok(!/gh release|git (tag|push)|action-gh-release|upload-artifact|actions\/cache|gh api|--publish/i.test(nativeWorkflow));
  assert.match(nativeWorkflow, /persist-credentials: false/);
  // Triggers are the trusted ones only.
  assert.ok(!/workflow_run|issue_comment|schedule:/.test(nativeWorkflow));
});

test('the toolchain script installs nothing unless provisioning was requested', () => {
  const lines = toolchainScript.split('\n');
  const installs = lines
    .map((line, index) => ({ line, index }))
    .filter(({ line }) => /^\s*&\s*(choco|rustup)\s+(toolchain\s+)?install\b/.test(line));
  assert.equal(installs.length, 2, 'rustup and LLVM are the only installs');
  for (const { index } of installs) {
    const before = lines.slice(Math.max(0, index - 8), index).join('\n');
    assert.match(before, /if \(\$Provision/, `install at line ${index + 1} must be guarded by -Provision`);
  }
  assert.match(toolchainScript, /--allow-downgrade replaces a newer LLVM, which is why this runs only here/);
  assert.match(toolchainScript, /\$LlvmVersion = "20\.1\.8"/);
  assert.match(toolchainScript, /\$VulkanSdkVersion = "1\.4\.309\.0"/);
  assert.match(toolchainScript, /rust-toolchain\.toml/);
  // The same pins as the release workflow.
  assert.match(releaseWorkflow, /choco install llvm --version=20\.1\.8/);
  assert.match(releaseWorkflow, /version: 1\.4\.309\.0/);
});

test('the native-only preflight rebuilds the embedded frontend assets from a clean folder', () => {
  assert.match(buildScript, /if \(-not \$SkipInstall\) \{\n    pnpm install --frozen-lockfile/);
  assert.ok(!/-not \$SkipInstall -and -not \$NativeOnly/.test(buildScript));
  assert.match(buildScript, /function Build-FrontendAssets \{\n    Remove-GeneratedFrontendFolder "out"\n    pnpm build\n/);
  assert.match(buildScript, /Assert-File \(Join-Path \$frontendRoot "out\/index\.html"\)/);
  assert.match(buildScript, /if \(\$CheckOnly\) \{\n    Build-FrontendAssets\n    Push-Location \$tauriRoot\n    try \{\n        cargo check/);
  assert.match(buildScript, /Build-FrontendAssets\n\n\$cargoMetadata/);
  // The folder is removed only inside the frontend workspace and only without links.
  const remover = buildScript.slice(buildScript.indexOf('function Remove-GeneratedFrontendFolder'), buildScript.indexOf('function Build-FrontendAssets'));
  assert.match(remover, /StartsWith\(\$frontendPrefix/);
  assert.match(remover, /ReparsePoint/);
  // No skip or stand-in distributable.
  assert.ok(!/mkdir.*out|New-Item.*\bout\b|index\.html"? -Value|Set-Content.*index\.html/i.test(buildScript));
});

test('the native workflow checks the volume that holds the Cargo target folder', () => {
  assert.match(cacheScript, /"Cargo target volume" = \$targetDirectory/);
  assert.match(cacheScript, /"Checkout volume" = \$repoRoot/);
  assert.match(cacheScript, /DriveType -ne \[System\.IO\.DriveType\]::Fixed/);
  assert.match(cacheScript, /\[A-Za-z\]:\\\\\$/);
  assert.ok(!/DriveInfo\]::new\(\$root\)\s*\n\s*\$freeGiB/.test(cacheScript));
});

// ---- packaged acceptance: isolation, failure reporting, cleanup -------------------------

test('packaged acceptance reports only safe categories and keeps the first failure', () => {
  // Exit codes are always shown; a missing report is a categorised failure, not a crash.
  assert.match(acceptanceScript, /function Format-ExitCode/);
  assert.match(acceptanceScript, /Write-Host "\[\$Phase\] exit code: \$\(Format-ExitCode \$result\.ExitCode\)"/);
  assert.match(acceptanceScript, /no readable report; exit code/);
  assert.match(acceptanceScript, /\$knownChecks = @\(/);
  // The application's own output and error text are never kept or printed.
  assert.ok(!/\.Output\b|\.Errors\b|\$output\.Result|\$errors\.Result/.test(acceptanceScript));
  assert.ok(!/\$check\.detail\)?"|\$\(\$check\.detail\)/.test(acceptanceScript), 'failure detail text is not printed');
  assert.match(acceptanceScript, /function Get-SafeDetail/);
  // Only deliberate messages reach the log; anything else is reduced to its type name.
  assert.match(acceptanceScript, /class AcceptanceFailure : System\.Exception/);
  assert.match(acceptanceScript, /stopped unexpectedly \(\$\(\$_\.Exception\.GetType\(\)\.Name\)\)/);
  // Cleanup cannot replace the first failure.
  assert.match(acceptanceScript, /\} catch \{\n    # The first failure is kept\./);
  assert.match(acceptanceScript, /try \{ Remove-OwnedWorkFolder -Folder \$work -Base \$workBase \} catch \{ \$cleanupFailure = /);
  assert.ok(acceptanceScript.indexOf('if ($firstFailure) {') > acceptanceScript.indexOf('} finally {'));
  assert.match(acceptanceScript, /Also: \$cleanupFailure/);
  // The work folder is on a fixed local drive with enough room.
  assert.match(acceptanceScript, /DriveType -ne \[System\.IO\.DriveType\]::Fixed -or \$volume\.AvailableFreeSpace -lt 3GB/);
});

test('packaged acceptance checks real refusals with exact exit codes and untouched folders', () => {
  for (const [label, code] of [
    ['no phase', 2], ['no profile folder', 2], ['unknown phase', 2], ['extra argument', 2],
    ['relative profile folder', 4], ['non-empty profile folder', 4], ['profile folder with only a stale marker', 4],
    ['verify on an unmarked folder', 4], ['verify with a forged marker and state', 4],
  ]) {
    assert.match(acceptanceScript, new RegExp(`Test-Refusal "${label}" @\\([^\\n]*\\) ${code}\\b`), label);
  }
  assert.match(acceptanceScript, /\$ok = \(-not \$result\.TimedOut\) -and \(\$result\.ExitCode -eq \$ExpectedExit\) -and \(\$before -ceq \$after\)/);
  assert.match(acceptanceScript, /New-Item -ItemType Junction -Path \$link -Target \$outside/);
  assert.match(acceptanceScript, /nothing was written through the link/);
  assert.match(acceptanceScript, /\[System\.IO\.Directory\]::Delete\(\$Path\)/);
  // The child process gets a disposable temporary folder.
  assert.match(acceptanceScript, /\$info\.Environment\["TEMP"\] = \$TempFolder/);
});

test('the self-test cannot write outside an owned profile or touch a credential store', () => {
  // Credential store: only the isolated open path, never the production one.
  assert.ok(selfTestProduction.includes('DatabaseManager::new_isolated('));
  assert.ok(!selfTestProduction.includes('DatabaseManager::new('));
  const manager = read('frontend/src-tauri/src/database/manager.rs');
  assert.equal([...manager.matchAll(/migrate_provider_credentials/g)].length, 1);
  assert.match(manager, /if migrate_credentials \{\n\s+crate::database::repositories::setting::migrate_provider_credentials\(&pool\)\.await;\n\s+\}/);
  assert.match(manager, /Self::open\(tauri_db_path, backend_db_path, true\)/);
  assert.match(manager, /Self::open\(tauri_db_path, backend_db_path, false\)/);
  // The profile: marker, state and report are created, never overwritten or followed.
  assert.match(selfTestProduction, /write_new\(&plain\.join\(PROFILE_MARKER\)/);
  assert.match(selfTestProduction, /write_new\(&profile\.join\(STATE_FILE\)/);
  assert.match(selfTestProduction, /write_new\(&path, &bytes\)/);
  assert.ok(!/fs::write\(\s*(plain|profile)\.join\((PROFILE_MARKER|STATE_FILE)/.test(selfTestProduction));
  assert.match(selfTestProduction, /\.create_new\(true\)/);
  // Verify trusts only a marked, link-free folder and relative state paths inside it.
  assert.match(selfTestProduction, /let run_id = read_marker\(&plain\)\?;\n\s+reject_links\(&plain\)\?;/);
  assert.match(selfTestProduction, /profile_file\(profile, relative\)/);
  assert.ok(!/PathBuf::from\(path\)/.test(selfTestProduction), 'state paths are never used as absolute paths');
  assert.match(selfTestProduction, /The saved state does not belong to this profile/);
  assert.match(selfTestProduction, /The profile holds no database from a run phase/);
  // Recording and import folders are inside the profile, never the application's.
  assert.match(selfTestProduction, /let root = profile\.join\("recordings"\)/);
  assert.match(selfTestProduction, /Recovery wrote audio outside the profile folder/);
  assert.match(selfTestProduction, /\.no_proxy\(\)/);
});

test('the self-test tests exercise real behaviour, not constants', () => {
  const tests = selfTest.slice(selfTest.indexOf('#[cfg(test)]\nmod tests'));
  assert.ok(!tests.includes('Phase::Run.name() != Phase::Verify.name()'), 'the vacuous usage test is gone');
  for (const name of [
    'malformed_self_test_command_lines_are_usage_errors', 'a_non_empty_profile_folder_is_refused_untouched',
    'verify_refuses_a_profile_that_contains_a_link', 'verify_refuses_unmarked_and_forged_markers',
    'saved_state_paths_stay_inside_the_profile', 'saved_state_must_belong_to_this_run',
    'a_refused_run_reports_the_refusal_exit_code_and_changes_nothing', 'each_missing_or_wrong_package_element_is_named',
    'legacy_database_upgrades_imports_summarizes_and_reopens', 'recent_schema_with_seeded_rows_upgrades_imports_summarizes_and_reopens',
    'bundled_template_list_matches_the_source_folder_and_the_bundle_config',
  ]) assert.ok(tests.includes(`fn ${name}`), name);
  // The migrate/import/summary/reopen fixture runs in the required PR test group.
  assert.match(read('frontend/scripts/test-windows-native.ps1'), /self_test::tests::/);
  assert.match(tests, /summary_stub\(manager\.pool\(\), &meeting_id\)/);
  assert.match(tests, /DatabaseManager::new_isolated\(/);
  // The seeded recent-schema fixture leaves the newest migrations to apply and checks them.
  assert.match(selfTest, /const RECENT_SCHEMA_VERSION: i64 = 20261010000000;/);
  assert.match(selfTestProduction, /column_exists\(pool, "meetings", "calendar_link"\)/);
  assert.match(selfTestProduction, /column_exists\(pool, "transcript_file_sync", "retranscription_json"\)/);
});

test('the docs describe the toolchain, frontend and acceptance behaviour accurately', () => {
  const docs = read('docs/windows-release.md');
  assert.ok(docs.includes('ensure-native-toolchain.ps1'));
  assert.ok(docs.includes('provision-toolchain'));
  assert.ok(!docs.includes('`-NativeOnly` skips frontend install'), 'NativeOnly now installs and builds the frontend assets');
  assert.ok(docs.includes('rebuilds the exported frontend assets'));
  assert.ok(docs.includes('volume that holds the Cargo target folder'));
  assert.ok(docs.includes('forged marker'));
  assert.ok(docs.includes('exit code'));
  assert.ok(read('.github/workflows/WORKFLOWS_OVERVIEW.md').includes('provision-toolchain'));
});

// ---- Vulkan SDK acquisition: one isolated implementation ----------------------------------

const composite = read('.github/actions/vulkan-sdk/action.yml');
const workflowFiles = ['clawscribe-windows-release', 'native-library-tests', 'summary-chunking-tests', 'windows-native-loader-diagnostics', 'windows-candidate', 'release-readiness']
  .map(name => [name, read(`.github/workflows/${name}.yml`)]);

test('only the vulkan-sdk action unpacks the SDK, and it stages it in its own folder', () => {
  for (const [name, source] of workflowFiles) {
    assert.ok(!source.includes('install-vulkan-sdk'), `${name} must use the isolated vulkan-sdk action`);
  }
  assert.equal([...composite.matchAll(/humbletim\/install-vulkan-sdk@v1\.2/g)].length, 1);
  assert.ok(!/actions\/(cache|upload-artifact)|Swatinem\/rust-cache/.test(composite));
  // Order: resolve by content, prepare the isolated folder, unpack, verify and promote, clean up.
  const order = ['resolve-vulkan-sdk.ps1', '-Phase Prepare', 'humbletim/install-vulkan-sdk', '-Phase Promote', '-Phase Clean']
    .map(marker => composite.indexOf(marker));
  assert.ok(order.every(index => index > 0) && order.every((index, i) => i === 0 || index > order[i - 1]), order.join());
  // Everything that writes runs only when no matching SDK was found.
  for (const name of ['Ensure the SDK extractor', 'Prepare an isolated SDK staging folder', 'Extract the pinned Vulkan SDK into the staging folder', 'Verify the staged SDK and move it into its per-version folder']) {
    const block = stepBlock(composite.replace(/\n    - name:/g, '\n      - name:').replace(/^    - name:/m, '      - name:'), name);
    assert.match(block, /steps\.resolve\.outputs\.found != 'true'/, name);
  }
  assert.match(composite, /env\.CLAWSCRIBE_VULKAN_STAGING != ''/);
  assert.match(composite, /if: \$\{\{ always\(\) && steps\.prepare\.outcome == 'success' \}\}/);
  // Without install-missing a missing SDK fails (read-only use).
  assert.match(composite, /install-missing:\n\s+description: [^\n]+\n\s+default: 'false'/);
  assert.match(composite, /if \(\$env:INSTALL_MISSING -eq 'true'\) \{\n\s+& "\$\{\{ github\.workspace \}\}\/frontend\/scripts\/resolve-vulkan-sdk\.ps1" -Version \$env:PINNED_VERSION -AllowMissing\n\s+\} else \{\n\s+& "\$\{\{ github\.workspace \}\}\/frontend\/scripts\/resolve-vulkan-sdk\.ps1" -Version \$env:PINNED_VERSION\n\s+\}/);
});

test('release builds reuse a matching SDK or stage it in isolation', () => {
  const step = stepBlock(releaseWorkflow, 'Use the pinned Vulkan SDK');
  assert.match(step, /if: \$\{\{ inputs\.feature == 'windows-gpu' \|\| inputs\.feature == 'vulkan' \}\}/);
  assert.match(step, /uses: \.\/\.github\/actions\/vulkan-sdk/);
  assert.match(step, /version: 1\.4\.309\.0/);
  assert.match(step, /install-missing: 'true'/);
  // The old unconditional extraction and configuration steps are gone.
  assert.ok(!releaseWorkflow.includes('Install Vulkan SDK prerequisites'));
  assert.ok(!/choco install 7zip/.test(releaseWorkflow));
  // windows-candidate shares this implementation through the reusable release workflow.
  assert.match(read('.github/workflows/windows-candidate.yml'), /uses: \.\/\.github\/workflows\/clawscribe-windows-release\.yml/);
  assert.ok(releaseWorkflow.indexOf('Use the pinned Vulkan SDK') > releaseWorkflow.indexOf('uses: actions/checkout@'));
});

test('the staging script only ever writes to a fresh folder of its own', () => {
  const stage = read('frontend/scripts/stage-vulkan-sdk.ps1');
  assert.match(stage, /\$env:VULKAN_SDK \(or VULKAN_SDK_VERSION\)|VULKAN_SDK_VERSION/);
  assert.match(stage, /\$stagingPattern = '\^\\.staging-\[0-9a-f\]\{32\}\$'/);
  // Deletion: only the staging folder, after name, parent and link checks.
  assert.equal([...stage.matchAll(/Remove-Item/g)].length, 1);
  assert.match(stage, /Assert-NoLinkInPath \$stagingFull\n\s+if \(Get-ChildItem[^\n]+ReparsePoint[^\n]+\{ throw "contains a link" \}\n\s+Remove-Item -LiteralPath \$stagingFull -Recurse -Force/);
  // The per-version folder is created by a rename of the verified staging folder.
  assert.match(stage, /Move-Item -LiteralPath \$stagingFull -Destination \$final/);
  assert.match(stage, /The SDK installer did not use the isolated staging folder/);
  assert.match(stage, /The inherited SDK changed while the pinned SDK was staged/);
  assert.match(stage, /never overwritten or deleted automatically/);
});

test('the registered loader validation has a bounded read-only verify mode', () => {
  const loader = read('.github/workflows/windows-native-loader-diagnostics.yml');
  assert.deepEqual(workflowPolicyErrors('windows-native-loader-diagnostics.yml', loader), []);
  assert.match(loader, /^permissions:\n  contents: read\n/m);
  assert.match(loader, /workflow_dispatch:\n    inputs:\n      verify-only:\n        description: [^\n]+\n        type: boolean\n        default: false/);
  assert.match(loader, /timeout-minutes: 10/);
  assert.match(loader, /persist-credentials: false/);
  const readOnly = ['Verify the pinned native toolchain', 'Find the pinned Vulkan SDK without installing it', 'Probe the Vulkan loader without installing it', 'Summarize the read-only verification'];
  const block = name => stepBlock(loader, name);
  for (const name of readOnly) assert.match(block(name), /if: \$\{\{ inputs\['verify-only'\]/, name);
  // The read-only steps never install, provision or stage anything.
  for (const name of readOnly) {
    assert.ok(!/-Provision|-InstallFromSdk|install-missing: 'true'|choco|Move-Item|Remove-Item/.test(block(name)), name);
  }
  assert.match(block('Find the pinned Vulkan SDK without installing it'), /install-missing: 'false'/);
  assert.match(block('Verify the pinned native toolchain'), /\.\/frontend\/scripts\/ensure-native-toolchain\.ps1$/);
  // Everything that installs, extracts or builds is skipped in that mode.
  for (const name of ['Use the exact release SDK', 'Build a native load-time Vulkan dependency fixture', 'Verify missing-runtime rejection, installation and idempotence']) {
    assert.match(block(name), /if: \$\{\{ !inputs\['verify-only'\] \}\}/, name);
  }
  // The same summary evidence is produced by the SDK search.
  assert.match(read('frontend/scripts/vulkan-sdk-common.ps1'), /Vulkan SDK candidates/);
});
