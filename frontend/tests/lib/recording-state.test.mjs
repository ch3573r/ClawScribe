import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView(delaySubscriptions = false) {
  const harness = createHookHarness();
  const callbacks = {};
  const subscriptions = [];
  const requests = [];
  const intervals = new Map();
  let nextTimer = 0;
  let unsubscribed = 0;
  const service = { getRecordingState() { const request = deferred(); requests.push(request); return request.promise; } };
  for (const name of ['Started', 'Stopped', 'Paused', 'Resumed']) {
    service[`onRecording${name}`] = callback => {
      callbacks[name] = callback;
      const subscription = deferred();
      subscriptions.push(() => subscription.resolve(() => { unsubscribed++; }));
      if (!delaySubscriptions) subscriptions.at(-1)();
      return subscription.promise;
    };
  }
  const originalSet = globalThis.setInterval;
  const originalClear = globalThis.clearInterval;
  let RecordingStateProvider;
  try {
    globalThis.setInterval = callback => { const id = ++nextTimer; intervals.set(id, callback); return id; };
    globalThis.clearInterval = id => { intervals.delete(id); };
    ({ RecordingStateProvider } = loadTsModule(fileURLToPath(new URL('../../src/contexts/RecordingStateContext.tsx', import.meta.url)), {
      react: harness.react,
      'react/jsx-runtime': { jsx: (_type, props) => props },
      '@/services/recordingService': { recordingService: service },
      '@tauri-apps/api/core': { invoke: async () => 'live' },
      '@tauri-apps/api/event': { listen: async () => () => {} },
    }));
  } finally {
    globalThis.setInterval = originalSet;
    globalThis.clearInterval = originalClear;
  }
  return {
    ...harness, callbacks, requests, intervals, subscriptions,
    get unsubscribed() { return unsubscribed; },
    render: () => harness.render(() => RecordingStateProvider({ children: null })).value,
    tick() { for (const callback of intervals.values()) callback(); },
  };
}

const recording = { is_recording: true, is_paused: false, is_active: true, recording_duration: 10, active_duration: 10, recording_mode: 'live' };

test('listeners resolving after unmount are immediately removed and cannot start polling', async () => {
  const view = createView(true);
  view.render();
  view.unmount();
  view.subscriptions.forEach(resolve => resolve());
  await flush();
  view.callbacks.Started('live');
  assert.equal(view.unsubscribed, 4);
  assert.equal(view.requests.length, 0);
  assert.equal(view.intervals.size, 0);
});

test('an old recording snapshot cannot reverse a newer stopped event', async () => {
  const view = createView();
  view.render();
  await flush();
  view.callbacks.Stopped();
  view.requests[0].resolve(recording);
  await flush();
  assert.equal(view.render().isRecording, false);
  assert.equal(view.render().status, 'stopping');
  assert.equal(view.intervals.size, 0);
  view.unmount();
});

test('reload during recording restores the lifecycle and polls without overlapping requests', async () => {
  const view = createView();
  view.render();
  await flush();
  view.requests[0].resolve(recording);
  await flush();
  assert.equal(view.render().status, 'recording');
  assert.equal(view.intervals.size, 1);
  view.tick();
  view.tick();
  assert.equal(view.requests.length, 2);
  view.callbacks.Paused();
  view.requests[1].resolve(recording);
  await flush();
  assert.equal(view.render().isPaused, true);
  assert.equal(view.render().isActive, false);
  view.unmount();
  assert.equal(view.intervals.size, 0);
  assert.equal(view.unsubscribed, 4);
});
