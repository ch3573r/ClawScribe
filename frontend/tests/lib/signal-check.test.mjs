import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('a signal check starts only on request and reports each source separately', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const pending = deferred();
  const calls = [];
  const { useSignalCheck } = loadTsModule('src/components/settings/useSignalCheck', {
    react: hooks.react,
    '@/components/AudioLevelMeter': { levelToDb: value => 20 * Math.log10(value) },
    './useAudioLevelFeed': { holdPeak: (_, peak) => peak },
    '@tauri-apps/api/core': {
      Channel: class {},
      invoke: (command, args) => { calls.push([command, args]); return pending.promise; },
    },
  });
  const render = (enabled = true, mic = 'Selected microphone (input)') =>
    hooks.render(() => useSignalCheck(enabled, mic, 'Selected speakers (output)'));
  await render(false).start();
  assert.deepEqual(calls, []);
  assert.equal(render().running, false);
  void render().start();
  void render().start();
  assert.equal(calls.length, 1);
  const [command, args] = calls[0];
  assert.equal(command, 'check_audio_signal');
  assert.equal(args.microphone, 'Selected microphone (input)');
  assert.equal(args.systemAudio, 'Selected speakers (output)');
  args.updates.onmessage({ source: 'microphone', rms: 0.1, peak: 0.2 });
  args.updates.onmessage({ source: 'systemAudio', rms: 0.01, peak: 0.02 });
  assert.equal(render().levels.microphone.db, -20);
  assert.equal(render().levels.systemAudio.db, -40);
  pending.resolve({ microphone: 'detected', systemAudio: 'noSignal' });
  await flush();
  assert.equal(render().result.microphone, 'detected');
  assert.equal(render().result.systemAudio, 'noSignal');
  assert.equal(render().running, false);
});

test('selecting a different endpoint invalidates the running check result', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const pending = deferred();
  let updates;
  const { useSignalCheck } = loadTsModule('src/components/settings/useSignalCheck', {
    react: hooks.react,
    '@/components/AudioLevelMeter': { levelToDb: value => 20 * Math.log10(value) },
    './useAudioLevelFeed': { holdPeak: (_, peak) => peak },
    '@tauri-apps/api/core': {
      Channel: class {},
      invoke: (_, args) => { updates = args.updates; return pending.promise; },
    },
  });
  const render = mic => hooks.render(() => useSignalCheck(true, mic, null));
  void render('First microphone (input)').start();
  render('Second microphone (input)');
  updates.onmessage({ source: 'microphone', rms: 1, peak: 1 });
  pending.resolve({ microphone: 'detected', systemAudio: 'detected' });
  await flush();
  const feed = render('Second microphone (input)');
  assert.equal(feed.levels.microphone, undefined);
  assert.equal(feed.result, null);
  assert.equal(feed.running, false);
});
