const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');

module.exports = async ({ github, context, core }) => {
  const expectedCommit = '2ab2a6574e56062712fcbd85bc2e8a5cc52d3467';
  const buildRun = 33988883245;
  const version = '0.5.36';
  const tag = `v${version}`;
  if (context.repo.owner !== 'ch3573r' || context.repo.repo !== 'ClawScribe') {
    throw new Error('This verification is scoped to ch3573r/ClawScribe.');
  }

  // Never publish based on only the frontend job or a partly completed build.
  let completed = false;
  for (let attempt = 0; attempt < 75; attempt++) {
    const { data: run } = await github.rest.actions.getWorkflowRun({ ...context.repo, run_id: buildRun });
    if (run.head_sha !== expectedCommit) throw new Error('Unexpected build source commit.');
    if (run.status === 'completed') {
      if (run.conclusion !== 'success') throw new Error(`Windows build did not pass: ${run.conclusion}`);
      completed = true;
      break;
    }
    core.info(`Waiting for the pinned Windows build; current status: ${run.status}`);
    await new Promise(resolve => setTimeout(resolve, 30000));
  }
  if (!completed) throw new Error('The pinned build has not completed; no publication performed.');

  const { data: jobPage } = await github.rest.actions.listJobsForWorkflowRun({ ...context.repo, run_id: buildRun, per_page: 100 });
  const nativeJob = jobPage.jobs.find(job => job.name.includes('Build ClawScribe Windows installers'));
  const requiredSteps = ['Build release installers', 'Run native summary and resource regression tests', 'Write build metadata', 'Generate updater manifest (latest.json)', 'Publish GitHub Release'];
  if (!nativeJob || nativeJob.conclusion !== 'success' || requiredSteps.some(name => !nativeJob.steps.some(step => step.name === name && step.conclusion === 'success'))) {
    throw new Error('A required native build/staging gate did not pass.');
  }
  const comparison = await github.rest.repos.compareCommits({ ...context.repo, base: expectedCommit, head: 'main' });
  if (!['identical', 'ahead'].includes(comparison.data.status)) {
    throw new Error('The tested source is not merged into main.');
  }

  async function bytesFromResponse(response) {
    if (response.headers.location) {
      // The redirected signed download never receives the GitHub credential.
      const download = await fetch(response.headers.location);
      if (!download.ok) throw new Error(`Download failed: ${download.status}`);
      return Buffer.from(await download.arrayBuffer());
    }
    if (typeof response.data === 'string') return Buffer.from(response.data, 'utf8');
    if (Buffer.isBuffer(response.data) || response.data instanceof ArrayBuffer || ArrayBuffer.isView(response.data)) {
      return Buffer.from(response.data);
    }
    throw new Error('Expected downloaded bytes, not a metadata response.');
  }
  const logResponse = await github.request('GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs', { ...context.repo, job_id: nativeJob.id });
  const nativeLog = (await bytesFromResponse(logResponse)).toString('utf8').replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '');
  const groups = Object.fromEntries([...nativeLog.matchAll(/Running (\d+) required native tests: ([^\r\n]+)/g)].map(match => [match[2].trim(), Number(match[1])]));
  const expectedGroups = ['summary::processor::tests', 'summary::chunking::tests', 'audio::async_logger::tests', 'audio::hardware_detector::tests'];
  if (expectedGroups.some(name => !(groups[name] > 0))) throw new Error('Missing actual test-execution counts.');
  const nativeTestCount = expectedGroups.reduce((sum, name) => sum + groups[name], 0);
  const results = [...nativeLog.matchAll(/test result: ok\. (\d+) passed; (\d+) failed;/g)];
  if (results.length !== expectedGroups.length || results.some(match => Number(match[2]) !== 0) || results.reduce((sum, match) => sum + Number(match[1]), 0) !== nativeTestCount) {
    throw new Error('Native test results do not match the required discovered suites.');
  }

  const { data: release } = await github.rest.repos.getReleaseByTag({ ...context.repo, tag });
  if (!release.draft) throw new Error('Refusing to replace or change an already public release.');
  if (release.target_commitish !== expectedCommit) throw new Error('Draft release targets a different commit.');
  const { data: previousStable } = await github.rest.repos.getLatestRelease(context.repo);

  const nsis = `ClawScribe_${version}_x64-setup.exe`;
  const msi = `ClawScribe_${version}_x64_en-US.msi`;
  const filenames = [nsis, msi, 'SHA256SUMS.txt', 'BUILD-METADATA.txt', 'BUILD-METRICS.json', 'latest.json'];
  const output = path.resolve('verified-release');
  fs.mkdirSync(output, { recursive: true });
  const downloaded = new Map();
  const digests = {};
  for (const name of filenames) {
    const matches = release.assets.filter(asset => asset.name === name && asset.state === 'uploaded');
    if (matches.length !== 1 || matches[0].size <= 0) throw new Error(`Missing or ambiguous release asset: ${name}`);
    const asset = matches[0];
    const response = await github.request('GET /repos/{owner}/{repo}/releases/assets/{asset_id}', {
      ...context.repo, asset_id: asset.id, headers: { accept: 'application/octet-stream' }
    });
    const bytes = await bytesFromResponse(response);
    if (bytes.length !== asset.size) throw new Error(`Asset size mismatch: ${name}`);
    const digest = crypto.createHash('sha256').update(bytes).digest('hex');
    if (asset.digest && asset.digest !== `sha256:${digest}`) throw new Error(`GitHub digest mismatch: ${name}`);
    fs.writeFileSync(path.join(output, name), bytes);
    downloaded.set(name, bytes);
    digests[name] = digest;
    core.info(`Verified download ${name}: ${bytes.length} bytes; SHA-256 ${digest}`);
  }
  const text = name => downloaded.get(name).toString('utf8').replace(/^\uFEFF/, '');
  const checksums = new Map();
  const checksumAssets = new Map([
    [nsis, nsis], [msi, msi],
    [`nsis/${nsis}`, nsis], [`msi/${msi}`, msi]
  ]);
  for (const line of text('SHA256SUMS.txt').trim().split(/\r?\n/)) {
    const match = line.match(/^([a-fA-F0-9]{64})\s+\*?([^\r\n]+)$/);
    const assetName = match && checksumAssets.get(match[2]);
    if (!match || !assetName || checksums.has(assetName)) throw new Error('Unexpected checksum manifest entry.');
    checksums.set(assetName, match[1].toLowerCase());
  }
  if (checksums.size !== 2 || [...checksums].some(([name, digest]) => digests[name] !== digest)) {
    throw new Error('Installer SHA-256 verification failed.');
  }
  const metadata = Object.fromEntries(text('BUILD-METADATA.txt').trim().split(/\r?\n/).map(line => {
    const index = line.indexOf('=');
    if (index < 1) throw new Error('Malformed build metadata.');
    return [line.slice(0, index), line.slice(index + 1).trim()];
  }));
  const metrics = JSON.parse(text('BUILD-METRICS.json'));
  if (metadata.build_commit !== expectedCommit || metadata.version !== version || metadata.installer_product_version !== version || metrics.build_commit !== expectedCommit || metrics.version !== version || metrics.requested_feature !== 'windows-gpu') {
    throw new Error('Release version, feature, or source identity mismatch.');
  }
  const manifest = JSON.parse(text('latest.json'));
  const platform = manifest.platforms?.['windows-x86_64'];
  const expectedUrl = `https://github.com/${context.repo.owner}/${context.repo.repo}/releases/download/${tag}/${nsis}`;
  if (manifest.version !== version || platform?.url !== expectedUrl || !platform.signature) throw new Error('Invalid updater manifest.');
  const config = JSON.parse(fs.readFileSync('frontend/src-tauri/tauri.conf.json', 'utf8'));
  fs.writeFileSync(path.join(output, 'updater-public-key.pub'), Buffer.from(config.plugins.updater.pubkey, 'base64'));
  fs.writeFileSync(path.join(output, 'updater-signature.minisig'), Buffer.from(platform.signature, 'base64'));
  core.info(execFileSync('minisign', ['-V', '-m', path.join(output, nsis), '-p', path.join(output, 'updater-public-key.pub'), '-x', path.join(output, 'updater-signature.minisig')], { encoding: 'utf8' }));

  // Inspect whether the setup PE has an embedded Authenticode certificate.
  // This is intentionally separate from its verified Tauri updater signature.
  const setup = downloaded.get(nsis);
  if (setup.toString('ascii', 0, 2) !== 'MZ') throw new Error('Setup is not a PE executable.');
  const pe = setup.readUInt32LE(0x3c);
  if (setup.toString('binary', pe, pe + 4) !== 'PE\u0000\u0000') throw new Error('Invalid setup PE header.');
  const optional = pe + 24;
  const magic = setup.readUInt16LE(optional);
  if (![0x10b, 0x20b].includes(magic)) throw new Error('Unknown PE optional header.');
  const securityDirectory = optional + (magic === 0x20b ? 112 : 96) + 4 * 8;
  const authenticodePresent = setup.readUInt32LE(securityDirectory) !== 0 && setup.readUInt32LE(securityDirectory + 4) !== 0;

  const report = {
    version, tag, channel: 'prerelease', source_commit: expectedCommit, build_run: buildRun,
    native_test_count: nativeTestCount, native_test_groups: groups,
    installer_sha256: { [nsis]: digests[nsis], [msi]: digests[msi] },
    nsis_updater_signature: 'verified against the application public key',
    nsis_embedded_authenticode_certificate: authenticodePresent,
    previous_stable_tag: previousStable.tag_name,
    live_capture_acceptance: 'not confirmed for this build',
    graphical_install_upgrade_acceptance: 'not performed',
    target_notebook_benchmark: 'not performed',
    verified_at_utc: new Date().toISOString()
  };
  const reportBytes = Buffer.from(JSON.stringify(report, null, 2) + '\n');
  fs.writeFileSync(path.join(output, 'RELEASE-VERIFICATION.json'), reportBytes);
  if (release.assets.some(asset => asset.name === 'RELEASE-VERIFICATION.json')) throw new Error('A verification report already exists; inspect before replacing it.');
  await github.rest.repos.uploadReleaseAsset({
    ...context.repo, release_id: release.id, name: 'RELEASE-VERIFICATION.json',
    data: reportBytes, headers: { 'content-type': 'application/json', 'content-length': reportBytes.length }
  });

  const notes = fs.readFileSync('docs/releases/0.5.36.md', 'utf8');
  const changesStart = notes.indexOf('## Meeting Content And Notes');
  const changesEnd = notes.indexOf('## Validation And Limitations');
  if (changesStart < 0 || changesEnd <= changesStart) throw new Error('Audited release notes sections are missing.');
  const signing = authenticodePresent
    ? 'The setup contains an Authenticode certificate; its Windows trust-chain acceptance has not been tested here.'
    : 'The setup is not Authenticode-signed. Windows may display an unknown-publisher/SmartScreen warning.';
  const body = [
    '# ClawScribe 0.5.36 — Prerelease',
    '',
    '**For manual evaluation; not promoted to the stable updater.** Real microphone-plus-system-audio recording acceptance, graphical install/upgrade behavior, and sustained i5-1235U/8GB performance have not been confirmed for this build.',
    '',
    `Built from commit \`${expectedCommit}\` using the Windows x64 \`windows-gpu\` configuration.`,
    '',
    '## Install',
    `Download \`${nsis}\` for normal installation, or \`${msi}\` for MSI deployment. Back up existing meeting data before evaluation.`,
    '', signing,
    'The NSIS Tauri updater signature was independently verified against the public key embedded in the application configuration. That is not Authenticode publisher trust.',
    '', notes.slice(changesStart, changesEnd).trim(),
    '', '## Verified release checks',
    '- Complete Windows application build and both installer formats: passed.',
    `- Actual Windows native library tests: ${nativeTestCount} passed across the four required summary, chunking, logging, and hardware groups.`,
    '- Frontend typechecking and the configured 32-case helper/control suite: passed in the pinned build.',
    '- Both uploaded installers were downloaded and matched SHA-256 checksums and GitHub asset digests.',
    '- Build metadata, runtime version, source commit, feature configuration, and updater URL/signature were verified.',
    `- Build run: https://github.com/${context.repo.owner}/${context.repo.repo}/actions/runs/${buildRun}`,
    '', '## Known validation limits',
    'Native unit tests do not exercise live Teams/Webex capture, model recognition quality, rendered Windows accessibility, or graphical installation/upgrade. Prompt changes do not establish hallucination-free notes. Review names, decisions, dates, owners, and deadlines against the transcript/audio before sharing.',
    'Provider-wide context budgeting, resumable summary reduction, and evidence-linked actions remain further work. The four-thread policy is specific to Windows Whisper, not a process-wide budget or a measured notebook speed guarantee.',
    '', '## Update channel',
    `This is a GitHub prerelease, not the stable release. The prior stable channel (${previousStable.tag_name}) is not advanced by this publication.`,
    'The installed runtime reports 0.5.36. A differently rebuilt 0.5.36 is not a newer in-app update; changed future binaries need a higher numeric version. No published installer is replaced under this version.',
    '', '## Installer SHA-256',
    '```text', text('SHA256SUMS.txt').trim(), '```',
    '', 'See RELEASE-VERIFICATION.json for the machine-readable verification record.'
  ].join('\n');
  fs.writeFileSync(path.join(output, 'RELEASE-NOTES.md'), body + '\n');
  const { data: updated } = await github.rest.repos.updateRelease({
    ...context.repo, release_id: release.id,
    name: 'ClawScribe v0.5.36 — Prerelease', body,
    target_commitish: expectedCommit, prerelease: true, draft: false, make_latest: 'false'
  });
  if (updated.draft || !updated.prerelease) throw new Error('Unexpected publication state.');
  const { data: currentStable } = await github.rest.repos.getLatestRelease(context.repo);
  if (currentStable.id !== previousStable.id) throw new Error('Stable release changed during verification; inspect before claiming channel continuity.');
  core.info(`Published verified prerelease: ${updated.html_url}`);
  await core.summary.addHeading('Verified ClawScribe prerelease').addLink(tag, updated.html_url)
    .addRaw(`\nBuild: ${expectedCommit}\nNative tests: ${nativeTestCount}\nStable channel unchanged: ${previousStable.tag_name}\n`)
    .write();
};
