import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(t, { recording = { is_recording: true, is_paused: false, is_active: true }, uiStatus = 'RECORDING', uiRecording = true, stopRecordingFails = null } = {}) {
  const previousWindow = globalThis.window;
  const hooks = createHookHarness();
  const events = new Map();
  const windowListeners = new Map();
  const toasts = [];
  const calls = [];
  const subscriptions = [];
  const dismissed = [];
  globalThis.window = {
    addEventListener: (name, callback) => windowListeners.set(name, callback),
    removeEventListener: name => windowListeners.delete(name),
  };
  const { RecordingPostProcessingProvider } = loadTsModule('src/contexts/RecordingPostProcessingProvider.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: () => null, jsxs: () => null, Fragment: 'Fragment' },
    '@tauri-apps/api/event': { listen: async (name, callback) => { subscriptions.push(name); events.set(name, callback); return () => events.delete(name); } },
    '@tauri-apps/api/core': { invoke: async command => { calls.push(['invoke', command]); } },
    '@tauri-apps/api/path': { appDataDir: async () => 'synthetic-data' },
    sonner: { toast: {
      error: (title, options) => toasts.push({ title, ...options }),
      info: (title, options) => toasts.push({ kind: 'info', title, ...options }),
      warning: (title, options) => toasts.push({ kind: 'warning', title, ...options }),
      dismiss: id => dismissed.push(id),
    } },
    '@/hooks/useRecordingStop': { useRecordingStop: () => {} },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ isRecording: uiRecording, isStopping: false, status: uiStatus, setStatus: value => calls.push(['setStatus', value]) }),
      RecordingStatus: Object.fromEntries(['IDLE', 'STARTING', 'RECORDING', 'STOPPING'].map(value => [value, value])),
    },
    '@/services/recordingService': { recordingService: {
      getRecordingState: async () => recording,
      stopRecording: async path => {
        calls.push(['stop_recording', path.startsWith('synthetic-data/recording-')]);
        if (stopRecordingFails !== null) throw stopRecordingFails;
      },
    } },
    '@/lib/recording-capture': loadTsModule('src/lib/recording-capture.ts'),
    '@/lib/storage-dialog-controller': { openStorageDialog: () => calls.push(['openStorageDialog']) },
  });
  t.after(() => { hooks.unmount(); globalThis.window = previousWindow; });
  const render = () => hooks.render(() => RecordingPostProcessingProvider({ children: null }));
  render();
  return { render, events, windowListeners, toasts, calls, subscriptions, dismissed };
}

test('Stop from the sidebar saves the recording without needing the Home page', async t => {
  const app = setup(t);
  await flush();
  app.windowListeners.get('stop-recording-from-sidebar')();
  await flush();
  assert.deepEqual(app.calls.filter(([name]) => name !== 'version'), [
    ['setStatus', 'STOPPING'], ['stop_recording', true],
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

test('listeners subscribe once across repeated renders', async t => {
  const app = setup(t);
  await flush();
  for (let index = 0; index < 5; index++) app.render();
  await flush();
  assert.deepEqual([...app.subscriptions].sort(), ['batch-job-preempted', 'recording-disk-space-low', 'recording-error', 'recording-stop-failed', 'recording-stopped']);
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
  assert.ok(app.calls.some(([name]) => name === 'stop_recording'));
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

test('low disk space shows a lasting warning with the free space, no path, and one action', async t => {
  const app = setup(t);
  await flush();
  app.events.get('recording-disk-space-low')({ payload: { freeBytes: 612_345_678, folder: 'D:\\Private\\Recordings' } });
  assert.equal(app.toasts.length, 1);
  const toast = app.toasts[0];
  assert.equal(toast.kind, 'warning');
  assert.equal(toast.title, 'Low disk space');
  assert.equal(toast.id, 'recording-disk-space-low');
  assert.equal(toast.duration, Infinity);
  assert.equal(toast.description, '612 MB is free where recordings are saved. The recording continues, but it may be cut short if the disk fills up.');
  assert.doesNotMatch(JSON.stringify(toast.description), /Private|Recordings|\\/, 'the folder path is not shown');
  assert.equal(toast.action.label, 'Review storage');
});

test('the warning action opens the storage dialog', async t => {
  const app = setup(t);
  await flush();
  app.events.get('recording-disk-space-low')({ payload: { freeBytes: 1_234_567_890, folder: 'x' } });
  app.toasts[0].action.onClick();
  await flush();
  assert.ok(app.calls.some(([name]) => name === 'openStorageDialog'));
  assert.match(app.toasts[0].description, /^1\.2 GB is free/);
});

test('the low disk space warning goes away when the recording ends', async t => {
  const app = setup(t);
  await flush();
  app.events.get('recording-stopped')({ payload: {} });
  assert.deepEqual(app.dismissed, ['recording-disk-space-low']);
});

const NOT_RESPONDING = 'The audio device is not responding, so the recording cannot be stopped yet. Everything recorded so far is saved on disk. Try Stop again in a moment.';

test('a Stop the core refuses shows its message, skips the save flow and returns to recording at once', async t => {
  for (const failure of [NOT_RESPONDING, new Error(NOT_RESPONDING)]) {
    const app = setup(t, { uiStatus: 'RECORDING', stopRecordingFails: failure });
    await flush();
    app.windowListeners.get('stop-recording-from-sidebar')();
    await flush();
    const names = app.calls.filter(([name]) => name !== 'version').map(([name]) => name);
    assert.ok(!names.includes('handleRecordingStop'), 'the save flow is not run for a recording that is still running');
    assert.deepEqual(app.calls.filter(([name]) => name === 'setStatus'), [['setStatus', 'STOPPING'], ['setStatus', 'RECORDING']]);
    const shown = app.toasts.filter(toast => toast.title === 'Could not stop the recording');
    assert.equal(shown.length, 1);
    assert.equal(shown[0].description, NOT_RESPONDING);
  }
});

test('an unusable or very long error shows a plain message instead', async t => {
  const app = setup(t, { stopRecordingFails: 'x'.repeat(500) });
  await flush();
  app.windowListeners.get('stop-recording-from-sidebar')();
  await flush();
  const shown = app.toasts.find(toast => toast.title === 'Could not stop the recording');
  assert.match(shown.description, /still running/);
  assert.ok(shown.description.length < 100);
});

test('a Stop from the tray that failed is reported and, while stopping, returns to recording', async t => {
  const stopping = setup(t, { uiStatus: 'STOPPING' });
  await flush();
  stopping.events.get('recording-stop-failed')({ payload: { message: NOT_RESPONDING } });
  assert.equal(stopping.toasts.at(-1).description, NOT_RESPONDING);
  assert.deepEqual(stopping.calls.filter(([name]) => name === 'setStatus'), [['setStatus', 'RECORDING']]);
  // In any other state the message is shown and the state is left alone.
  const idle = setup(t, { uiStatus: 'RECORDING' });
  await flush();
  idle.events.get('recording-stop-failed')({ payload: { message: NOT_RESPONDING } });
  assert.equal(idle.toasts.at(-1).description, NOT_RESPONDING);
  assert.deepEqual(idle.calls.filter(([name]) => name === 'setStatus'), []);
});
