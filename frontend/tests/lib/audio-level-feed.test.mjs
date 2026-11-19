import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(t, invokeImpl) {
  const hooks = createHookHarness();
  const calls = [];
  const listeners = new Map();
  const unlistened = [];
  const module = loadTsModule('src/components/settings/useAudioLevelFeed.ts', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: () => null, jsxs: () => null },
    '@tauri-apps/api/core': { invoke: (command, args) => { calls.push([command, args && JSON.parse(JSON.stringify(args))]); return invokeImpl(command, args); } },
    '@tauri-apps/api/event': { listen: async (name, callback) => { listeners.set(name, callback); return () => unlistened.push(name); } },
  });
  t.after(() => hooks.unmount());
  const render = options => hooks.render(() => module.useAudioLevelFeed(options));
  const commands = () => calls.map(([command, args]) => command + (args?.deviceNames ? ':' + args.deviceNames.join('|') : ''));
  return { ...module, render, calls, commands, listeners, unlistened, hooks };
}

const device = name => [{ requested_name: name, device_name: name, used_default: false }];

test('level maths: decibels, peak fall and text', t => {
  const m = setup(t, async () => []);
  assert.equal(Math.round(m.holdPeak(null, -30)), -30);
  assert.equal(m.holdPeak(-10, -50), -13, 'the peak marker falls 3 dB per reading');
  assert.equal(m.holdPeak(-10, -5), -5, 'a louder reading replaces it at once');
  assert.equal(m.holdPeak(-59, -60), -60, 'never below the floor');
  assert.equal(m.formatDb(-24.6), '-25 dB');
  assert.equal(m.formatDb(-60), '< -60 dB');
  assert.equal(m.monitorRequestFor(null), 'default');
  assert.equal(m.monitorRequestFor('USB Headset (input)'), 'USB Headset');
});

test('the monitor starts when the page opens, reports real readings and stops when it is left', async t => {
  const m = setup(t, async command => command === 'start_audio_level_monitoring' ? device('USB Headset') : undefined);
  const options = { enabled: true, micDevice: 'USB Headset (input)' };
  m.render(options);
  assert.equal(m.render(options).status, 'starting', 'shows progress until the native monitor answers');
  await flush();
  assert.deepEqual(m.commands(), ['start_audio_level_monitoring:USB Headset']);
  assert.equal(m.render(options).status, 'live');
  assert.equal(m.render(options).monitored.usedDefault, false);

  // A reading for another device is ignored; one for the measured device becomes the level.
  m.listeners.get('audio-levels')({ payload: { levels: [{ device_name: 'Other', rms_level: 1, peak_level: 1, is_active: true }] } });
  assert.equal(m.render(options).microphone, null);
  m.listeners.get('audio-levels')({ payload: { levels: [{ device_name: 'USB Headset', rms_level: 0.1, peak_level: 0.2, is_active: true }] } });
  const feed = m.render(options);
  assert.equal(Math.round(feed.microphone.db), -20);
  assert.equal(Math.round(feed.microphone.peakDb), -14);
  assert.equal(feed.microphone.active, true);

  m.hooks.unmount();
  await flush();
  assert.deepEqual(m.commands(), ['start_audio_level_monitoring:USB Headset', 'stop_audio_level_monitoring']);
  assert.deepEqual([...m.unlistened], ['audio-levels']);
});

test('changing the microphone restarts the monitor in order, and a page left mid-start still releases the microphone', async t => {
  const start = deferred();
  let starts = 0;
  const m = setup(t, command => command === 'start_audio_level_monitoring' ? (++starts === 1 ? start.promise : Promise.resolve(device('B'))) : Promise.resolve());
  m.render({ enabled: true, micDevice: 'A (input)' });
  m.render({ enabled: true, micDevice: 'B (input)' });
  await flush();
  assert.deepEqual(m.commands(), ['start_audio_level_monitoring:A'], 'the second start waits for the first');
  start.resolve(device('A'));
  await flush();
  assert.deepEqual(m.commands(), [
    'start_audio_level_monitoring:A', 'stop_audio_level_monitoring', 'start_audio_level_monitoring:B',
  ]);
  const feed = m.render({ enabled: true, micDevice: 'B (input)' });
  assert.equal(feed.monitored.deviceName, 'B', 'the late first result does not replace the new device');
});

test('nothing is started while disabled, such as during a recording, and errors can be retried', async t => {
  let fail = true;
  const m = setup(t, async command => {
    if (command === 'start_audio_level_monitoring' && fail) throw 'No microphone level streams could be started';
    return command === 'start_audio_level_monitoring' ? device('Mic') : undefined;
  });
  assert.equal(m.render({ enabled: false, micDevice: null }).status, 'idle');
  await flush();
  assert.deepEqual(m.commands(), [], 'the microphone stays closed');

  m.render({ enabled: true, micDevice: null });
  await flush();
  const failed = m.render({ enabled: true, micDevice: null });
  assert.equal(failed.status, 'error');
  assert.match(failed.error, /No microphone level streams/);

  fail = false;
  failed.retry();
  m.render({ enabled: true, micDevice: null });
  await flush();
  assert.equal(m.render({ enabled: true, micDevice: null }).status, 'live');
});
