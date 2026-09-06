// Every GitHub/network operation is mocked. Only disposable fixture files and
// ephemeral minisign keys are created; no real release can be published here.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const verify = require('./verify-0.5.36-preview.cjs');
const commit = '1d92be1122bdcf0a6f93ed47e4981a3836e215a1';
const nsis = 'ClawScribe_0.5.36_x64-setup.exe';
const msi = 'ClawScribe_0.5.36_x64_en-US.msi';
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'clawscribe-verifier-fixture-'));
  const setup = Buffer.alloc(512);
  setup.write('MZ'); setup.writeUInt32LE(128, 0x3c);
  setup.write('PE\0\0', 128, 'binary'); setup.writeUInt16LE(0x20b, 152);
  fs.writeFileSync(path.join(root, 'setup.fixture'), setup);
  execFileSync('minisign', ['-G', '-W', '-p', path.join(root, 'public.key'), '-s', path.join(root, 'secret.key')], { stdio: 'pipe' });
  execFileSync('minisign', ['-S', '-s', path.join(root, 'secret.key'), '-m', path.join(root, 'setup.fixture'), '-x', path.join(root, 'signature')], { stdio: 'pipe' });
  const config = JSON.stringify({ plugins: { updater: { pubkey: fs.readFileSync(path.join(root, 'public.key')).toString('base64') } } });
  fs.mkdirSync(path.join(root, 'docs/releases'), { recursive: true });
  fs.writeFileSync(path.join(root, 'docs/releases/0.5.36.md'), '## Meeting Content And Notes\nFixture-only description.\n## Validation And Limitations\n');
  const files = new Map([
    [nsis, setup], [msi, Buffer.from('Fixture only, not a real MSI')],
    ['BUILD-METADATA.txt', Buffer.from(`version=0.5.36\ninstaller_product_version=0.5.36\nbuild_commit=${commit}\n`)],
    ['BUILD-METRICS.json', Buffer.from(JSON.stringify({ version: '0.5.36', installer_product_version: '0.5.36', build_commit: commit, requested_feature: 'windows-gpu' }))],
    ['latest.json', Buffer.from(JSON.stringify({ version: '0.5.36', platforms: { 'windows-x86_64': { url: 'https://github.com/ch3573r/ClawScribe/releases/download/v0.5.36/' + nsis, signature: fs.readFileSync(path.join(root, 'signature')).toString('base64') } } }))]
  ]);
  files.set('SHA256SUMS.txt', Buffer.from(`${sha(setup)}  nsis/${nsis}\n${sha(files.get(msi))}  msi/${msi}\n`));
  const groups = [['summary::processor::tests', 26], ['summary::chunking::tests', 5], ['audio::async_logger::tests', 2], ['audio::hardware_detector::tests', 7]];
  const model = {
    root, files, config, updates: [], uploads: [], tag: null, assetLists: 0,
    run: { head_sha: commit, status: 'completed', conclusion: 'success' },
    validation: { name: 'validate', conclusion: 'success' },
    job: { id: 123, name: 'stage / Build ClawScribe Windows installers', conclusion: 'success', steps: ['Build release installers', 'Run native summary and resource regression tests', 'Write build metadata', 'Generate updater manifest (latest.json)', 'Publish GitHub Release'].map(name => ({ name, conclusion: 'success' })) },
    log: groups.map(([name, n]) => `Running ${n} required native tests: ${name}\ntest result: ok. ${n} passed; 0 failed; 0 ignored;\n`).join(''),
    release: { id: 1234, draft: true, prerelease: false, tag_name: 'v0.5.36', target_commitish: commit, assets: [], html_url: 'https://example.test/fixture-release' }
  };
  model.refreshAssets = () => { model.release.assets = [...files].map(([name, bytes], i) => ({ id: i + 1, name, size: bytes.length, state: 'uploaded', digest: 'sha256:' + sha(bytes) })); };
  model.refreshAssets();
  model.github = {
    rest: {
      actions: { getWorkflowRun: async () => ({ data: model.run }), listJobsForWorkflowRun: async () => ({ data: { jobs: [model.validation, model.job] } }) },
      git: {
        getRef: async () => { if (!model.tag) throw Object.assign(new Error('No draft tag yet'), { status: 404 }); return { data: { object: model.tag } }; },
        getTag: async () => ({ data: { object: { type: 'commit', sha: model.annotatedTarget ?? commit } } })
      },
      repos: {
        compareCommits: async () => ({ data: { status: model.comparison ?? 'ahead' } }),
        getReleaseByTag: async () => ({ data: model.release }),
        getRelease: async () => ({ data: model.release }),
        getContent: async args => { assert.equal(args.ref, commit); return { data: { content: Buffer.from(model.config).toString('base64'), encoding: 'base64' } }; },
        listReleaseAssets: async () => {
          model.assetLists++;
          if (model.replaceAsset && model.assetLists === 2) model.release.assets[0].id += 100;
          return { data: model.release.assets };
        },
        getLatestRelease: async () => ({ data: { id: 352410193, tag_name: 'v0.5.35' } }),
        uploadReleaseAsset: async args => { model.uploads.push(args); return { data: {} }; },
        updateRelease: async args => { model.updates.push(args); Object.assign(model.release, args); model.tag ??= { type: 'commit', sha: commit }; return { data: model.release }; }
      }
    },
    paginate: async (method, args) => { const { data } = await method(args); return data.jobs ?? data; },
    request: async (route, args) => {
      if (route.includes('/actions/jobs/')) return { headers: {}, data: Buffer.from(model.log) };
      if (route.includes('/releases/assets/')) {
        const asset = model.release.assets.find(a => a.id === args.asset_id);
        if (!asset) throw new Error('Unexpected asset request');
        return { headers: {}, data: files.get(asset.name) };
      }
      throw new Error('Unexpected network operation: ' + route);
    }
  };
  return model;
}
const core = { info() {}, summary: { addHeading() { return this; }, addLink() { return this; }, addRaw() { return this; }, async write() {} } };
const updateJson = (f, name, edit) => { const x = JSON.parse(f.files.get(name)); edit(x); f.files.set(name, Buffer.from(JSON.stringify(x))); f.refreshAssets(); };
const cases = [
  ['valid draft without a tag publishes exact build as prerelease', true, () => {}],
  ['valid existing lightweight tag', true, f => { f.tag = { type: 'commit', sha: commit }; }],
  ['valid existing annotated tag', true, f => { f.tag = { type: 'tag', sha: 'annotated' }; }],
  ['valid flat checksum layout', true, f => { f.files.set('SHA256SUMS.txt', Buffer.from(`${sha(f.files.get(nsis))}  ${nsis}\n${sha(f.files.get(msi))}  ${msi}\n`)); f.refreshAssets(); }],
  ['wrong build commit', false, f => { f.run.head_sha = 'wrong'; }],
  ['unfinished build', false, f => { f.run.status = 'in_progress'; }],
  ['failed build', false, f => { f.run.conclusion = 'failure'; }],
  ['failed frontend validation', false, f => { f.validation.conclusion = 'failure'; }],
  ['failed native gate', false, f => { f.job.steps[1].conclusion = 'failure'; }],
  ['missing native group', false, f => { f.log = f.log.replace('Running 7 required native tests: audio::hardware_detector::tests', 'Missing group'); }],
  ['incorrect individual test count', false, f => { f.log = f.log.replace('26 passed', '25 passed'); }],
  ['ignored tests', false, f => { f.log = f.log.replace('0 ignored', '1 ignored'); }],
  ['already public release', false, f => { f.release.draft = false; }],
  ['wrong draft target', false, f => { f.release.target_commitish = 'wrong'; }],
  ['conflicting tag', false, f => { f.tag = { type: 'commit', sha: 'wrong' }; }],
  ['conflicting annotated tag', false, f => { f.tag = { type: 'tag', sha: 'annotated' }; f.annotatedTarget = 'wrong'; }],
  ['source not merged into main', false, f => { f.comparison = 'diverged'; }],
  ['size mismatch', false, f => { f.release.assets[0].size++; }],
  ['GitHub digest mismatch', false, f => { f.release.assets[0].digest = 'sha256:' + '0'.repeat(64); }],
  ['missing installer', false, f => { f.release.assets.pop(); }],
  ['path traversal checksum', false, f => { f.files.set('SHA256SUMS.txt', Buffer.from(`${sha(f.files.get(nsis))}  ../${nsis}\n${sha(f.files.get(msi))}  msi/${msi}\n`)); f.refreshAssets(); }],
  ['tampered checksum', false, f => { f.files.set('SHA256SUMS.txt', Buffer.from(`${'0'.repeat(64)}  nsis/${nsis}\n${sha(f.files.get(msi))}  msi/${msi}\n`)); f.refreshAssets(); }],
  ['wrong source metadata', false, f => { f.files.set('BUILD-METADATA.txt', Buffer.from('version=0.5.36\ninstaller_product_version=0.5.36\nbuild_commit=wrong\n')); f.refreshAssets(); }],
  ['duplicate metadata', false, f => { f.files.set('BUILD-METADATA.txt', Buffer.concat([f.files.get('BUILD-METADATA.txt'), Buffer.from('version=0.5.36\n')])); f.refreshAssets(); }],
  ['wrong metrics version', false, f => updateJson(f, 'BUILD-METRICS.json', x => { x.installer_product_version = '0.5.35'; })],
  ['wrong updater URL', false, f => updateJson(f, 'latest.json', x => { x.platforms['windows-x86_64'].url = 'https://example.test/wrong.exe'; })],
  ['invalid signature', false, f => updateJson(f, 'latest.json', x => { x.platforms['windows-x86_64'].signature = Buffer.from('invalid').toString('base64'); })],
  ['missing audited notes', false, f => fs.writeFileSync(path.join(f.root, 'docs/releases/0.5.36.md'), 'Missing sections')],
  ['asset replaced during verification', false, f => { f.replaceAsset = true; }],
  ['existing report not overwritten', false, f => { f.release.assets.push({ name: 'RELEASE-VERIFICATION.json' }); }]
];
(async () => {
  for (const [name, success, mutate] of cases) {
    const f = fixture();
    const previous = process.cwd();
    try {
      mutate(f); process.chdir(f.root);
      const call = () => verify({ github: f.github, context: { repo: { owner: 'ch3573r', repo: 'ClawScribe' } }, core });
      if (success) {
        await call(); assert.equal(f.updates.length, 1); assert.equal(f.uploads.length, 1);
        assert.equal(f.updates[0].prerelease, true); assert.equal(f.updates[0].draft, false); assert.equal(f.updates[0].make_latest, 'false'); assert.equal(f.updates[0].target_commitish, commit);
        const report = JSON.parse(f.uploads[0].data); assert.equal(report.native_test_count, 40); assert.equal(report.nsis_embedded_authenticode_certificate, false); assert.equal(report.live_capture_acceptance, 'not confirmed for this build');
      } else {
        await assert.rejects(call); assert.equal(f.updates.length, 0, 'Rejected fixture must not publish'); assert.equal(f.uploads.length, 0, 'Validation must finish before uploading');
      }
      console.log('PASS (isolated fixture): ' + name);
    } finally { process.chdir(previous); fs.rmSync(f.root, { recursive: true, force: true }); }
  }
  console.log(`${cases.length} isolated verifier cases passed. No actual release was modified by these tests.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
