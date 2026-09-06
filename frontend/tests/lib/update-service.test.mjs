import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture(channel = 'stable') {
  const calls = { checks: 0, downloads: [], installs: [], closed: [], relaunches: 0 };
  const control = { channel, check: null, saveError: null, download: null, recording: false };
  const result = (rid = 1, selected = control.channel) => ({
    channel: selected,
    update: { rid, currentVersion: '0.5.36', version: '0.5.37', prerelease: selected === 'preview', rawJson: {} },
  });
  class FakeUpdate {
    constructor(metadata) { Object.assign(this, metadata); }
    async close() { calls.closed.push(this.rid); }
    async download(progress) {
      calls.downloads.push(this.rid);
      progress({ event: 'Started', data: { contentLength: 200 } });
      progress({ event: 'Progress', data: { chunkLength: 200 } });
      if (control.download) await control.download();
    }
    async install() { calls.installs.push(this.rid); }
  }
  const { UpdateService } = loadTsModule('src/services/updateService.ts', {
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'get_update_channel') return control.channel;
      if (command === 'set_update_channel') {
        if (control.saveError) throw control.saveError;
        control.channel = args.channel;
        return;
      }
      if (command === 'is_recording') return control.recording;
      if (command === 'check_app_update') {
        ++calls.checks;
        return control.check ? control.check() : result();
      }
      throw new Error(`Unexpected command: ${command}`);
    } },
    '@tauri-apps/api/app': { getVersion: async () => '0.5.36' },
    '@tauri-apps/plugin-updater': { Update: FakeUpdate },
    '@tauri-apps/plugin-process': { relaunch: async () => { ++calls.relaunches; } },
  });
  return { service: new UpdateService(), control, calls, result };
}

test('loads persisted channel and actual installed version before checking', async () => {
  const f = fixture('preview');
  assert.equal(f.service.getSnapshot().channel, 'stable');
  const info = await f.service.checkForUpdates();
  assert.equal(f.service.getSnapshot().channel, 'preview');
  assert.equal(info.currentVersion, '0.5.36');
  assert.equal(info.prerelease, true);
});

test('concurrent entry points share one check and successful cache retains update', async () => {
  const f = fixture();
  const pending = deferred();
  f.control.check = () => pending.promise;
  const checks = [f.service.checkForUpdates(true), f.service.checkForUpdates(true)];
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.calls.checks, 1);
  pending.resolve(f.result());
  const [first, second] = await Promise.all(checks);
  assert.equal(first, second);
  assert.equal(await f.service.checkForUpdates(), first);
  assert.equal(f.calls.checks, 1);
});

test('network failure is visible and does not suppress a retry for 24 hours', async () => {
  const f = fixture();
  f.control.check = () => { throw new Error('Offline'); };
  await assert.rejects(f.service.checkForUpdates(), /Offline/);
  assert.match(f.service.getSnapshot().error, /Offline/);
  f.control.check = null;
  assert.equal((await f.service.checkForUpdates()).available, true);
  assert.equal(f.calls.checks, 2);
  assert.equal(f.service.getSnapshot().error, null);
});

test('channel switch invalidates cached release and checks the new channel immediately', async () => {
  const f = fixture('preview');
  const previous = await f.service.checkForUpdates();
  await f.service.setChannel('stable');
  assert.equal(f.service.getSnapshot().updateInfo, null);
  assert.deepEqual(f.calls.closed, [1]);
  const next = await f.service.checkForUpdates();
  assert.equal(next.channel, 'stable');
  assert.equal(next.prerelease, false);
  assert.equal(f.calls.checks, 2);
  await assert.rejects(f.service.downloadAndInstall(previous, () => {}, () => true), /selected update changed/);
});

test('late old-channel result is closed without replacing newer state', async () => {
  const f = fixture('preview');
  const pending = deferred();
  f.control.check = () => pending.promise;
  const oldCheck = f.service.checkForUpdates(true);
  await new Promise(resolve => setImmediate(resolve));
  await f.service.setChannel('stable');
  f.control.check = () => f.result(2, 'stable');
  const current = await f.service.checkForUpdates(true);
  pending.resolve(f.result(1, 'preview'));
  assert.equal(await oldCheck, null);
  assert.equal(f.service.getSnapshot().updateInfo, current);
  assert.deepEqual(f.calls.closed, [1]);
});

test('failed preference write keeps the prior channel and checked resource', async () => {
  const f = fixture();
  const previous = await f.service.checkForUpdates();
  f.control.saveError = new Error('Could not save');
  await assert.rejects(f.service.setChannel('preview'), /Could not save/);
  assert.equal(f.service.getSnapshot().channel, 'stable');
  assert.equal(f.service.getSnapshot().updateInfo, previous);
  assert.equal(f.service.getSnapshot().isSavingChannel, false);
  assert.deepEqual(f.calls.closed, []);
});

test('late old-channel failure does not overwrite a successful new-channel check', async () => {
  const f = fixture('preview');
  const pending = deferred();
  f.control.check = () => pending.promise;
  const oldCheck = f.service.checkForUpdates(true);
  await new Promise(resolve => setImmediate(resolve));
  await f.service.setChannel('stable');
  f.control.check = () => f.result(2, 'stable');
  const current = await f.service.checkForUpdates(true);
  pending.reject(new Error('Old preview request failed'));
  assert.equal(await oldCheck, null);
  assert.equal(f.service.getSnapshot().updateInfo, current);
  assert.equal(f.service.getSnapshot().error, null);
  assert.equal(f.service.getSnapshot().isChecking, false);
});

test('install uses the exact checked preview resource without checking stable again', async () => {
  const f = fixture('preview');
  f.control.check = () => f.result(42);
  const info = await f.service.checkForUpdates();
  const progress = [];
  await f.service.downloadAndInstall(info, p => progress.push(p), () => true);
  assert.equal(f.calls.checks, 1);
  assert.deepEqual(f.calls.downloads, [42]);
  assert.deepEqual(f.calls.installs, [42]);
  assert.equal(f.calls.relaunches, 1);
  assert.equal(progress.at(-1).phase, 'installing');
  assert.equal(progress.at(-1).percentage, 100);
});

test('double install clicks are idempotent and channel changes are blocked during install', async () => {
  const f = fixture();
  const pending = deferred();
  f.control.download = () => pending.promise;
  const info = await f.service.checkForUpdates();
  const first = f.service.downloadAndInstall(info, () => {}, () => true);
  await new Promise(resolve => setImmediate(resolve));
  await f.service.downloadAndInstall(info, () => {}, () => true);
  await assert.rejects(f.service.setChannel('preview'), /Wait for/);
  pending.resolve();
  await first;
  assert.equal(f.calls.downloads.length, 1);
  assert.equal(f.calls.installs.length, 1);
});

test('failed signature/download releases memory and never installs or relaunches', async () => {
  const f = fixture();
  f.control.download = () => { throw new Error('Signature verification failed'); };
  const info = await f.service.checkForUpdates();
  await assert.rejects(f.service.downloadAndInstall(info, () => {}, () => true), /Signature verification/);
  assert.equal(f.calls.installs.length, 0);
  assert.equal(f.calls.relaunches, 0);
  assert.deepEqual(f.calls.closed, [1]);
  assert.equal(f.service.getSnapshot().isInstalling, false);
  assert.equal(f.service.getSnapshot().updateInfo, null);
});

test('active capture and recording started during download both prevent installation', async () => {
  for (const timing of ['before', 'during']) {
    const f = fixture();
    f.control.recording = timing === 'before';
    f.control.download = () => { f.control.recording = true; };
    const info = await f.service.checkForUpdates();
    await assert.rejects(f.service.downloadAndInstall(info, () => {}, () => true), /Finish recording/);
    assert.equal(f.calls.installs.length, 0);
    assert.equal(f.calls.relaunches, 0);
  }
});
