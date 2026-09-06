const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');

// Deliberately pinned to one completed build, not a mutable branch or latest run.
module.exports = async ({ github, context, core }) => {
  const expectedCommit = '1d92be1122bdcf0a6f93ed47e4981a3836e215a1';
  const buildRun = 34010383598;
  const version = '0.5.36';
  const tag = `v${version}`;
  if (context.repo.owner !== 'ch3573r' || context.repo.repo !== 'ClawScribe') throw new Error('Unexpected repository.');
  const { data: run } = await github.rest.actions.getWorkflowRun({ ...context.repo, run_id: buildRun });
  if (run.head_sha !== expectedCommit || run.status !== 'completed' || run.conclusion !== 'success') throw new Error('Exact Windows build has not passed.');
  const jobs = await github.paginate(github.rest.actions.listJobsForWorkflowRun, { ...context.repo, run_id: buildRun, per_page: 100 });
  const validate = jobs.find(job => job.name === 'validate');
  const nativeJob = jobs.find(job => job.name === 'stage / Build ClawScribe Windows installers');
  const requiredSteps = ['Build release installers', 'Run native summary and resource regression tests', 'Write build metadata', 'Generate updater manifest (latest.json)', 'Publish GitHub Release'];
  if (validate?.conclusion !== 'success' || nativeJob?.conclusion !== 'success' || requiredSteps.some(name => !nativeJob.steps.some(step => step.name === name && step.conclusion === 'success'))) throw new Error('Required build/staging gate did not pass.');
  const comparison = await github.rest.repos.compareCommits({ ...context.repo, base: expectedCommit, head: 'main' });
  if (!['identical', 'ahead'].includes(comparison.data.status)) throw new Error('The tested source is not merged into main.');

  async function checkedTag(allowMissing) {
    let object;
    try { object = (await github.rest.git.getRef({ ...context.repo, ref: `tags/${tag}` })).data.object; }
    catch (error) { if (allowMissing && error.status === 404) return; throw error; }
    for (let i = 0; object.type === 'tag' && i < 4; i++) object = (await github.rest.git.getTag({ ...context.repo, tag_sha: object.sha })).data.object;
    if (object.type !== 'commit' || object.sha !== expectedCommit) throw new Error('Tag conflicts with the verified build; never move it.');
  }
  await checkedTag(true); // Drafts can legitimately have no tag until publication.
  async function bytesFromResponse(response) {
    if (response.headers?.location) {
      const url = new URL(response.headers.location);
      if (url.protocol !== 'https:') throw new Error('Insecure download redirect.');
      // Never forward the GitHub credential to signed storage redirects.
      const download = await fetch(url, { signal: AbortSignal.timeout(120000) });
      if (!download.ok) throw new Error(`Download failed: ${download.status}`);
      return Buffer.from(await download.arrayBuffer());
    }
    if (typeof response.data === 'string') return Buffer.from(response.data, 'utf8');
    if (Buffer.isBuffer(response.data)) return response.data;
    if (response.data instanceof ArrayBuffer) return Buffer.from(response.data);
    if (ArrayBuffer.isView(response.data)) return Buffer.from(response.data.buffer, response.data.byteOffset, response.data.byteLength);
    throw new Error('Expected downloaded bytes, not metadata.');
  }
  const logResponse = await github.request('GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs', { ...context.repo, job_id: nativeJob.id });
  const nativeLog = (await bytesFromResponse(logResponse)).toString('utf8').replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '');
  const markers = [...nativeLog.matchAll(/Running (\d+) required native tests: ([^\r\n]+)/g)];
  const expectedGroups = ['summary::processor::tests', 'summary::chunking::tests', 'audio::async_logger::tests', 'audio::hardware_detector::tests'];
  if (markers.length !== expectedGroups.length) throw new Error('Unexpected native test group count.');
  const groups = {};
  for (let i = 0; i < expectedGroups.length; i++) {
    const marker = markers[i];
    const name = marker[2].trim();
    const count = Number(marker[1]);
    const section = nativeLog.slice(marker.index + marker[0].length, markers[i + 1]?.index ?? nativeLog.length);
    const results = [...section.matchAll(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/g)];
    if (name !== expectedGroups[i] || count <= 0 || results.length !== 1 || Number(results[0][1]) !== count || results[0][2] !== '0' || results[0][3] !== '0') throw new Error(`Incomplete native test evidence: ${name}`);
    groups[name] = count;
  }
  const nativeTestCount = Object.values(groups).reduce((a, b) => a + b, 0);
  const { data: release } = await github.rest.repos.getReleaseByTag({ ...context.repo, tag });
  if (!release.draft || release.target_commitish !== expectedCommit || release.tag_name !== tag) throw new Error('Draft identity mismatch or already public.');
  const { data: previousStable } = await github.rest.repos.getLatestRelease(context.repo);
  if (previousStable.tag_name === tag) throw new Error('Candidate already owns stable channel.');
  const assets = await github.paginate(github.rest.repos.listReleaseAssets, { ...context.repo, release_id: release.id, per_page: 100 });
  if (assets.some(a => a.name === 'RELEASE-VERIFICATION.json')) throw new Error('Existing verification report requires review before replacement.');
  const nsis = `ClawScribe_${version}_x64-setup.exe`;
  const msi = `ClawScribe_${version}_x64_en-US.msi`;
  const filenames = [nsis, msi, 'SHA256SUMS.txt', 'BUILD-METADATA.txt', 'BUILD-METRICS.json', 'latest.json'];
  const output = fs.mkdtempSync(path.join(os.tmpdir(), 'clawscribe-verified-'));
  try {
    const downloaded = new Map();
    const digests = {};
    const identities = new Map();
    for (const name of filenames) {
      const matches = assets.filter(asset => asset.name === name);
      if (matches.length !== 1 || matches[0].state !== 'uploaded' || matches[0].size <= 0 || matches[0].size > 1024 * 1024 * 1024) throw new Error(`Invalid asset: ${name}`);
      const asset = matches[0];
      const response = await github.request('GET /repos/{owner}/{repo}/releases/assets/{asset_id}', { ...context.repo, asset_id: asset.id, headers: { accept: 'application/octet-stream' } });
      const bytes = await bytesFromResponse(response);
      if (bytes.length !== asset.size) throw new Error(`Asset size mismatch: ${name}`);
      const digest = crypto.createHash('sha256').update(bytes).digest('hex');
      if (asset.digest !== `sha256:${digest}`) throw new Error(`GitHub digest mismatch: ${name}`);
      fs.writeFileSync(path.join(output, name), bytes);
      downloaded.set(name, bytes);
      identities.set(name, asset.id);
      digests[name] = digest;
      core.info(`Verified download ${name}: ${bytes.length} bytes; SHA-256 ${digest}`);
    }
    const text = name => downloaded.get(name).toString('utf8').replace(/^\uFEFF/, '');
    const checksums = new Map();
    const checksumAssets = new Map([[nsis, nsis], [msi, msi], [`nsis/${nsis}`, nsis], [`msi/${msi}`, msi]]);
    for (const line of text('SHA256SUMS.txt').trim().split(/\r?\n/)) {
      const match = line.match(/^([a-fA-F0-9]{64})\s+\*?([^\r\n]+)$/);
      const assetName = match && checksumAssets.get(match[2]);
      if (!match || !assetName || checksums.has(assetName)) throw new Error('Unexpected checksum manifest entry.');
      checksums.set(assetName, match[1].toLowerCase());
    }
    if (checksums.size !== 2 || [...checksums].some(([name, digest]) => digests[name] !== digest)) throw new Error('Installer SHA-256 verification failed.');
    const metadata = {};
    for (const line of text('BUILD-METADATA.txt').trim().split(/\r?\n/)) {
      const i = line.indexOf('=');
      const key = line.slice(0, i);
      if (i < 1 || Object.hasOwn(metadata, key)) throw new Error('Malformed or duplicate metadata.');
      metadata[key] = line.slice(i + 1).trim();
    }
    const metrics = JSON.parse(text('BUILD-METRICS.json'));
    if (metadata.build_commit !== expectedCommit || metadata.version !== version || metadata.installer_product_version !== version || metrics.build_commit !== expectedCommit || metrics.version !== version || metrics.installer_product_version !== version || metrics.requested_feature !== 'windows-gpu') throw new Error('Build source/version/feature mismatch.');
    const manifest = JSON.parse(text('latest.json'));
    const platform = manifest.platforms?.['windows-x86_64'];
    const expectedUrl = `https://github.com/${context.repo.owner}/${context.repo.repo}/releases/download/${tag}/${nsis}`;
    if (manifest.version !== version || platform?.url !== expectedUrl || !platform.signature) throw new Error('Invalid updater manifest.');
    const configFile = (await github.rest.repos.getContent({ ...context.repo, path: 'frontend/src-tauri/tauri.conf.json', ref: expectedCommit })).data;
    const config = JSON.parse(Buffer.from(configFile.content, configFile.encoding).toString('utf8'));
    if (!config.plugins?.updater?.pubkey) throw new Error('Missing committed updater public key.');
    fs.writeFileSync(path.join(output, 'updater.pub'), Buffer.from(config.plugins.updater.pubkey, 'base64'));
    fs.writeFileSync(path.join(output, 'updater.minisig'), Buffer.from(platform.signature, 'base64'));
    core.info(execFileSync('minisign', ['-V', '-m', path.join(output, nsis), '-p', path.join(output, 'updater.pub'), '-x', path.join(output, 'updater.minisig')], { encoding: 'utf8' }));
    const setup = downloaded.get(nsis);
    if (setup.length < 64 || setup.toString('ascii', 0, 2) !== 'MZ') throw new Error('Setup is not PE.');
    const pe = setup.readUInt32LE(0x3c);
    if (pe + 26 > setup.length || setup.toString('binary', pe, pe + 4) !== 'PE\0\0') throw new Error('Invalid PE header.');
    const optional = pe + 24;
    const magic = setup.readUInt16LE(optional);
    if (![0x10b, 0x20b].includes(magic)) throw new Error('Unknown PE format.');
    const security = optional + (magic === 0x20b ? 112 : 96) + 4 * 8;
    if (security + 8 > setup.length) throw new Error('Truncated PE security directory.');
    const authenticodePresent = setup.readUInt32LE(security) !== 0 && setup.readUInt32LE(security + 4) !== 0;
    const notes = fs.readFileSync('docs/releases/0.5.36.md', 'utf8');
    const start = notes.indexOf('## Meeting Content And Notes');
    const end = notes.indexOf('## Validation And Limitations');
    if (start < 0 || end <= start) throw new Error('Audited release notes are missing.');
    const report = {
      version, tag, channel: 'prerelease', source_commit: expectedCommit, build_run: buildRun,
      verification_commit: process.env.GITHUB_SHA, native_test_count: nativeTestCount, native_test_groups: groups,
      asset_sha256: digests, asset_ids: Object.fromEntries(identities),
      nsis_updater_signature: 'verified against the exact built commit public key',
      nsis_embedded_authenticode_certificate: authenticodePresent, previous_stable_tag: previousStable.tag_name,
      live_capture_acceptance: 'not confirmed for this build', graphical_install_upgrade_acceptance: 'not performed',
      rendered_ui_accessibility: 'not performed', target_notebook_benchmark: 'not performed', verified_at_utc: new Date().toISOString()
    };
    const signing = authenticodePresent ? 'The setup contains an Authenticode certificate; Windows trust-chain acceptance has not been tested here.' : 'The setup is not Authenticode-signed. Windows may display an unknown-publisher/SmartScreen warning.';
    const body = [
      '# ClawScribe 0.5.36 — Prerelease', '',
      '**Manual evaluation release; not a stable-channel update.** Real microphone/system-audio capture, graphical installation/upgrade, rendered accessibility/scaling, and sustained i5-1235U/8 GB performance have not been confirmed for this build.', '',
      `**Binary source:** \`${expectedCommit}\`. **Build:** https://github.com/${context.repo.owner}/${context.repo.repo}/actions/runs/${buildRun}`, '',
      `## Install\nDownload \`${nsis}\` for normal installation, or \`${msi}\` for MSI deployment. Back up existing meeting data before evaluation.`, '',
      'The Windows GPU build requires the Vulkan loader (vulkan-1.dll), normally supplied by a supported GPU driver or official Vulkan runtime. The build fix provisions that prerequisite on CI; it does not bundle a driver or install the runtime on the user’s machine. Do not obtain individual DLLs from third-party download sites.', '',
      signing, 'The Tauri updater signature was independently verified; this is separate from Windows publisher trust. Never bypass an unexpected signature or checksum mismatch.', '',
      notes.slice(start, end).trim(), '',
      '## Verified release checks',
      '- Complete Windows application build and both installer formats: passed.',
      `- Actual Windows release-profile native tests: **${nativeTestCount} passed** across four required summary, chunking, logging, and hardware groups.`,
      '- Frontend typecheck and configured helper/control suite: passed in the pinned candidate build.',
      '- Both uploaded installers downloaded and matched SHA-256 checksums and GitHub asset digests.',
      '- Exact binary identity, runtime version, feature configuration, and NSIS updater URL/signature verified.', '',
      '## Known limitations',
      'Native unit tests do not exercise live Teams/Webex capture, model accuracy, graphical installation, or rendered Windows UI. Prompt changes do not prove hallucination-free notes; review names, numbers, decisions, owners, and deadlines against transcript/audio before sharing.',
      'Provider-wide context budgeting, resumable summary reduction, and evidence-linked actions remain further work. The four-thread policy applies to Windows Whisper, not the entire app, and is not a measured notebook-speed guarantee.', '',
      '## Update channel',
      `The stable updater remains on ${previousStable.tag_name}. Install this prerelease manually. Its runtime version is ${version}; changed subsequent binaries need a higher numeric version, not replacement under the same tag.`, '',
      '## Installer SHA-256', '```text', `${digests[nsis]}  ${nsis}`, `${digests[msi]}  ${msi}`, '```', '',
      'RELEASE-VERIFICATION.json records source identity, actual native test counts, downloaded asset hashes, and unperformed acceptance checks.'
    ].join('\n');
    // Finish all validation before any write. Recheck identity/digests to reject replacements.
    const latestDraft = (await github.rest.repos.getRelease({ ...context.repo, release_id: release.id })).data;
    if (!latestDraft.draft || latestDraft.target_commitish !== expectedCommit || latestDraft.tag_name !== tag) throw new Error('Draft changed during verification.');
    const recheck = await github.paginate(github.rest.repos.listReleaseAssets, { ...context.repo, release_id: release.id, per_page: 100 });
    for (const name of filenames) {
      const matches = recheck.filter(a => a.name === name);
      if (matches.length !== 1 || matches[0].id !== identities.get(name) || matches[0].digest !== `sha256:${digests[name]}` || matches[0].state !== 'uploaded') throw new Error(`Asset changed during verification: ${name}`);
    }
    if (recheck.some(a => a.name === 'RELEASE-VERIFICATION.json')) throw new Error('Verification report appeared concurrently.');
    await checkedTag(true);
    if ((await github.rest.repos.getLatestRelease(context.repo)).data.id !== previousStable.id) throw new Error('Stable release changed during verification.');
    const reportBytes = Buffer.from(JSON.stringify(report, null, 2) + '\n');
    await github.rest.repos.uploadReleaseAsset({ ...context.repo, release_id: release.id, name: 'RELEASE-VERIFICATION.json', data: reportBytes, headers: { 'content-type': 'application/json', 'content-length': reportBytes.length } });
    await github.rest.repos.updateRelease({ ...context.repo, release_id: release.id, name: 'ClawScribe v0.5.36 — Prerelease', body, target_commitish: expectedCommit, prerelease: true, draft: false, make_latest: 'false' });
    const published = (await github.rest.repos.getRelease({ ...context.repo, release_id: release.id })).data;
    const stable = (await github.rest.repos.getLatestRelease(context.repo)).data;
    if (published.draft || !published.prerelease || published.tag_name !== tag || stable.id !== previousStable.id) throw new Error('Post-publication channel check failed.');
    await checkedTag(false);
    core.info(`Published verified prerelease: ${published.html_url}`);
    await core.summary.addHeading('Verified ClawScribe prerelease').addLink(tag, published.html_url).addRaw(`\nBuild: ${expectedCommit}\nNative tests: ${nativeTestCount}\nStable unchanged: ${stable.tag_name}\n`).write();
  } finally {
    fs.rmSync(output, { recursive: true, force: true });
  }
};
