import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(t, { recording = { is_recording: true, is_paused: false, is_active: true }, uiStatus = 'RECORDING', uiRecording = true, stop = async () => {}, exitSaveFailure = false } = {}) {
  const previousWindow = globalThis.window;
  const hooks = createHookHarness();
  const events = new Map();
  const windowListeners = new Map();
  const toasts = [];
  const calls = [];
  const subscriptions = [];
  globalThis.window = {
    addEventListener: (name, callback) => windowListeners.set(name, callback),
    removeEventListener: name => windowListeners.delete(name),
  };
  const stopHandler = async value => { calls.push(['handleRecordingStop', value]); return stop(value); };
  let stopHandlerVersion = 0;
  const { RecordingPostProcessingProvider } = loadTsModule('src/contexts/RecordingPostProcessingProvider.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: () => null, jsxs: () => null, Fragment: 'Fragment' },
    '@tauri-apps/api/event': { listen: async (name, callback) => { subscriptions.push(name); events.set(name, callback); return () => events.delete(name); } },
    '@tauri-apps/api/core': { invoke: async command => { calls.push(['invoke', command]); if (exitSaveFailure) throw new Error('Synthetic failure'); } },
    '@tauri-apps/api/path': { appDataDir: async () => 'synthetic-data' },
    sonner: { toast: {
      error: (title, options) => toasts.push({ title, ...options }),
      info: (title, options) => toasts.push({ kind: 'info', title, ...options }),
    } },
    '@/hooks/useRecordingStop': { useRecordingStop: () => {
      // A new function every render, as the real hook gives while meetings change.
      const version = ++stopHandlerVersion;
      return { handleRecordingStop: value => { calls.push(['version', version]); return stopHandler(value); }, setIsStopping: value => calls.push(['setIsStopping', value]) };
    } },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ isRecording: uiRecording, isStopping: false, status: uiStatus, setStatus: value => calls.push(['setStatus', value]) }),
      RecordingStatus: Object.fromEntries(['IDLE', 'STARTING', 'RECORDING', 'STOPPING'].map(value => [value, value])),
    },
    '@/services/recordingService': { recordingService: {
      getRecordingState: async () => recording,
      stopRecording: async path => { calls.push(['stop_recording', path.startsWith('synthetic-data/recording-')]); },
    } },
    '@/lib/recording-capture': loadTsModule('src/lib/recording-capture.ts'),
  });
  t.after(() => { hooks.unmount(); globalThis.window = previousWindow; });
  const render = () => hooks.render(() => RecordingPostProcessingProvider({ children: null }));
  render();
  return { render, events, windowListeners, toasts, calls, subscriptions };
}

test('Stop from the sidebar saves the recording without needing the Home page', async t => {
  const app = setup(t);
  await flush();
  app.windowListeners.get('stop-recording-from-sidebar')();
  await flush();
  assert.deepEqual(app.calls.filter(([name]) => name !== 'version'), [
    ['setIsStopping', true], ['stop_recording', true], ['handleRecordingStop', true],
  ]);
});

test('a second Stop while one is running is ignored', async t => {
  const app = setup(t);
  await flush();
  const stop = app.windowListeners.get('stop-recording-from-sidebar');
  stop(); stop();
  await flush();
  assert.equal(app.calls.filter(([name]) => name === 'stop_recording').length, 1);
});

test('nothing is stopped when no recording is running', async t => {
  const app = setup(t, { uiRecording: false, uiStatus: 'IDLE' });
  await flush();
  app.windowListeners.get('stop-recording-from-sidebar')();
  await flush();
  assert.deepEqual(app.calls.filter(([name]) => name !== 'version'), []);
});

test('listeners subscribe once, however often the stop handler changes', async t => {
  const app = setup(t);
  await flush();
  for (let index = 0; index < 5; index++) app.render();
  await flush();
  assert.deepEqual([...app.subscriptions].sort(), ['batch-job-preempted', 'recording-error', 'recording-stop-complete']);
  app.events.get('recording-stop-complete')({ payload: true });
  await flush();
  const versions = app.calls.filter(([name]) => name === 'version');
  assert.equal(versions.length, 1);
  assert.ok(versions[0][1] >= 6, 'the newest handler runs');
});

test('a capture failure that stopped the session shows a lasting error and saves what was captured', async t => {
  const app = setup(t, { recording: { is_recording: true, is_paused: false, is_active: false } });
  await flush();
  app.events.get('recording-error')({ payload: 'Audio device was disconnected' });
  await flush();
  assert.equal(app.toasts.length, 1);
  assert.equal(app.toasts[0].title, 'Recording stopped');
  assert.equal(app.toasts[0].duration, Infinity);
  assert.match(app.toasts[0].description, /Audio device was disconnected/);
  assert.ok(app.calls.some(([name, value]) => name === 'handleRecordingStop' && value === true));
});

test('a recoverable error is reported and the recording keeps running', async t => {
  const app = setup(t);
  await flush();
  app.events.get('recording-error')({ payload: 'Audio buffer overflow' });
  await flush();
  assert.equal(app.toasts[0].title, 'Recording problem');
  assert.match(app.toasts[0].description, /Recording continues/);
  assert.ok(!app.calls.some(([name]) => name === 'stop_recording'));
});

test('an error while the app is still starting clears the starting state', async t => {
  const app = setup(t, { recording: { is_recording: false, is_paused: false, is_active: false }, uiStatus: 'STARTING', uiRecording: false });
  await flush();
  app.events.get('recording-error')({ payload: 'Failed to initialize audio system' });
  await flush();
  assert.ok(app.calls.some(([name, value]) => name === 'setStatus' && value === 'IDLE'));
  assert.equal(app.toasts[0].title, 'Recording failed');
  assert.doesNotMatch(app.toasts[0].description, /continues/);
});

test('a batch job stopped for the recording is announced without claiming it is already stopped', async t => {
  const app = setup(t);
  await flush();
  app.events.get('batch-job-preempted')({ payload: { job: 'retranscription' } });
  app.events.get('batch-job-preempted')({ payload: { job: 'something-new' } });
  assert.equal(app.toasts[0].kind, 'info');
  assert.equal(app.toasts[0].title, 'Retranscription is being stopped for the recording');
  assert.equal(app.toasts[0].description, 'Start it again after the recording if it did not finish.');
  assert.equal(app.toasts[1].title, 'A running job is being stopped for the recording');
});

test('the listener runs the stop handler and leaves the exit confirmation to it', async t => {
  const app = setup(t, { stop: async () => { throw new Error('Synthetic save failure'); } });
  await flush();
  app.events.get('recording-stop-complete')({ payload: true });
  await flush();
  assert.ok(app.calls.some(([name, value]) => name === 'handleRecordingStop' && value === true));
  assert.ok(!app.calls.some(([name]) => name === 'invoke'), 'the provider does not confirm itself');
});
