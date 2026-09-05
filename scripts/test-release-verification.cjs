// All GitHub calls are mocked. These tests never build, install, upload, or
// publish an application. Real minisign verifies ephemeral fixture signatures.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const verify = require('./verify-0.5.36-preview.cjs');

const commit = '2ab2a6574e56062712fcbd85bc2e8a5cc52d3467';
const nsis = 'ClawScribe_0.5.36_x64-setup.exe';
const msi = 'ClawScribe_0.5.36_x64_en-US.msi';
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'clawscribe-verifier-fixture-'));
  const setup = Buffer.alloc(512);
  setup.write('MZ'); setup.writeUInt32LE(128, 0x3c);
  setup.write('PE\0\0', 128, 'binary'); setup.writeUInt16LE(0x20b, 152);
  fs.writeFileSync(path.join(root, 'setup.fixture'), setup);
  execFileSync('minisign', ['-G','-W','-p',path.join(root,'public.key'),'-s',path.join(root,'secret.key')], {stdio:'pipe'});
  execFileSync('minisign', ['-S','-s',path.join(root,'secret.key'),'-m',path.join(root,'setup.fixture'),'-x',path.join(root,'signature')], {stdio:'pipe'});
  fs.mkdirSync(path.join(root,'frontend/src-tauri'), {recursive:true});
  fs.mkdirSync(path.join(root,'docs/releases'), {recursive:true});
  fs.writeFileSync(path.join(root,'frontend/src-tauri/tauri.conf.json'), JSON.stringify({plugins:{updater:{pubkey:fs.readFileSync(path.join(root,'public.key')).toString('base64')}}}));
  fs.writeFileSync(path.join(root,'docs/releases/0.5.36.md'), '## Meeting Content And Notes\n\nFixture-only change description.\n\n## Validation And Limitations\n');
  const files = new Map([
    [nsis, setup], [msi, Buffer.from('Fixture bytes, not a real MSI')],
    ['BUILD-METADATA.txt', Buffer.from('version=0.5.36\ninstaller_product_version=0.5.36\nbuild_commit='+commit+'\n')],
    ['BUILD-METRICS.json', Buffer.from(JSON.stringify({version:'0.5.36',build_commit:commit,requested_feature:'windows-gpu'}))],
    ['latest.json', Buffer.from(JSON.stringify({version:'0.5.36',platforms:{'windows-x86_64':{url:'https://github.com/ch3573r/ClawScribe/releases/download/v0.5.36/'+nsis,signature:fs.readFileSync(path.join(root,'signature')).toString('base64')}}}))]
  ]);
  files.set('SHA256SUMS.txt', Buffer.from(sha(files.get(nsis))+'  nsis/'+nsis+'\n'+sha(files.get(msi))+'  msi/'+msi+'\n'));
  const groups = [['summary::processor::tests',26],['summary::chunking::tests',5],['audio::async_logger::tests',2],['audio::hardware_detector::tests',7]];
  const steps = ['Build release installers','Run native summary and resource regression tests','Write build metadata','Generate updater manifest (latest.json)','Publish GitHub Release'].map(name => ({name,conclusion:'success'}));
  const model = {
    root, files, updates:[], uploads:[],
    run:{head_sha:commit,status:'completed',conclusion:'success'},
    job:{id:123,name:'stage / Build ClawScribe Windows installers',conclusion:'success',steps},
    log:groups.map(([name,count]) => `Running ${count} required native tests: ${name}\ntest result: ok. ${count} passed; 0 failed;`).join('\n'),
    release:{id:1234,draft:true,target_commitish:commit,assets:[]}
  };
  model.refreshAssets = () => {
    model.release.assets = [...files].map(([name,bytes],index) => ({id:index+1,name,size:bytes.length,state:'uploaded',digest:'sha256:'+sha(bytes)}));
  };
  model.refreshAssets();
  model.github = {
    rest:{
      actions:{getWorkflowRun:async() => ({data:model.run}), listJobsForWorkflowRun:async() => ({data:{jobs:[model.job]}})},
      repos:{
        compareCommits:async() => ({data:{status:'ahead'}}),
        getReleaseByTag:async() => ({data:model.release}),
        getLatestRelease:async() => ({data:{id:352410193,tag_name:'v0.5.35'}}),
        uploadReleaseAsset:async args => { model.uploads.push(args); return {data:{}}; },
        updateRelease:async args => { model.updates.push(args); return {data:{...args,html_url:'https://example.test/fixture-release'}}; }
      }
    },
    request:async(route,args) => {
      if (route.includes('/actions/jobs/')) return {headers:{},data:Buffer.from(model.log)};
      if (route.includes('/releases/assets/')) {
        const asset = model.release.assets.find(asset => asset.id === args.asset_id);
        if (!asset) throw new Error('Unexpected asset request');
        return {headers:{},data:files.get(asset.name)};
      }
      throw new Error('Unexpected network operation: '+route);
    }
  };
  return model;
}

const core = {info(){},summary:{addHeading(){return this;},addLink(){return this;},addRaw(){return this;},async write(){}}};
const cases = [
  ['valid bundle paths and genuine updater signature', null],
  ['wrong build commit', f => {f.run.head_sha='unexpected';}],
  ['failed native gate', f => {f.job.steps[1].conclusion='failure';}],
  ['missing native test group', f => {f.log=f.log.replace('Running 7 required native tests: audio::hardware_detector::tests','Missing group');}],
  ['already-public release', f => {f.release.draft=false;}],
  ['asset size mismatch', f => {f.release.assets[0].size++;}],
  ['GitHub digest mismatch', f => {f.release.assets[0].digest='sha256:'+'0'.repeat(64);}],
  ['tampered installer checksum', f => {f.files.set('SHA256SUMS.txt',Buffer.from('0'.repeat(64)+'  nsis/'+nsis+'\n'+sha(f.files.get(msi))+'  msi/'+msi+'\n'));f.refreshAssets();}],
  ['incorrect source metadata', f => {f.files.set('BUILD-METADATA.txt',Buffer.from('version=0.5.36\ninstaller_product_version=0.5.36\nbuild_commit=wrong\n'));f.refreshAssets();}],
  ['incorrect updater URL', f => {const manifest=JSON.parse(f.files.get('latest.json'));manifest.platforms['windows-x86_64'].url='https://example.test/unexpected.exe';f.files.set('latest.json',Buffer.from(JSON.stringify(manifest)));f.refreshAssets();}],
  ['invalid updater signature', f => {const manifest=JSON.parse(f.files.get('latest.json'));manifest.platforms['windows-x86_64'].signature=Buffer.from('invalid fixture signature').toString('base64');f.files.set('latest.json',Buffer.from(JSON.stringify(manifest)));f.refreshAssets();}]
];

(async () => {
  for (const [name, mutate] of cases) {
    const f = fixture();
    const previous = process.cwd();
    try {
      mutate?.(f);
      process.chdir(f.root);
      const run = () => verify({github:f.github,context:{repo:{owner:'ch3573r',repo:'ClawScribe'}},core});
      if (mutate) {
        await assert.rejects(run);
        assert.equal(f.updates.length,0,'A rejected fixture must not publish');
      } else {
        await run();
        assert.equal(f.updates.length,1);
        assert.equal(f.updates[0].prerelease,true);
        assert.equal(f.updates[0].draft,false);
        assert.equal(f.updates[0].make_latest,'false');
        assert.equal(f.updates[0].target_commitish,commit);
        const report=JSON.parse(f.uploads[0].data);
        assert.equal(report.native_test_count,40);
        assert.equal(report.nsis_embedded_authenticode_certificate,false);
        assert.equal(report.live_capture_acceptance,'not confirmed for this build');
      }
      console.log('PASS (isolated verifier fixture): '+name);
    } finally {
      process.chdir(previous);
      fs.rmSync(f.root,{recursive:true,force:true});
    }
  }
  console.log(`${cases.length} isolated verifier cases passed. No actual release or installer was produced by these tests.`);
})().catch(error => {console.error(error);process.exitCode=1;});
