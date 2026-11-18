import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const path = name => fileURLToPath(new URL('../../src/' + name, import.meta.url));
const call = { provider: 'googleMeet', window: '9-a', key: 'room-a' };
const snapshot = (active = [], openWindows = [], ended = []) => ({ supported: true, complete: true, active, openWindows, ended });

function setupWindow() {
  const entries = new Map();
  const target = new EventTarget();
  target.localStorage = { getItem: key => entries.get(key) ?? null, setItem: (key,value) => entries.set(key,value) };
  globalThis.window = target;
  return loadTsModule(path('lib/autoRecord.ts'));
}

test('upgrades retain Teams-only consent and begin with automatic stopping off', () => {
  const prefs = setupWindow();
  window.localStorage.setItem(prefs.AUTO_RECORD_STORAGE_KEY, 'true');
  assert.equal(JSON.stringify(prefs.getCallSettings()), JSON.stringify({ mode: 'auto', providers: ['teams'], autoStop: false }));
  window.localStorage.setItem('clawscribe.teamsDetectionMode', 'prompt');
  assert.equal(prefs.getCallSettings().mode, 'prompt');
});

test('a hidden tab, incomplete snapshot or interrupted poll resets consecutive closure evidence', () => {
  const { ObservedCalls } = setupWindow();
  const seen = new ObservedCalls();
  seen.remember(call);
  for (const unknown of [snapshot([], [call.window]), { ...snapshot(), complete: false }, snapshot([call],[call.window])]) {
    assert.equal(seen.update(snapshot()).length, 0);
    assert.equal(seen.update(snapshot()).length, 0);
    assert.equal(seen.update(unknown).length, 0);
  }
  seen.interrupted();
  assert.equal(seen.update(snapshot()).length, 0);
  assert.equal(seen.update(snapshot()).length, 0);
  assert.equal(seen.update(snapshot([], [call.window], [call]))[0], call.key);
});

function poller(mode, autoStop, recording = false) {
  const prefs = setupWindow();
  const settings = { mode, providers: ['googleMeet'], autoStop };
  prefs.setCallSettings(settings);
  const hooks = createHookHarness();
  const state = { isRecording: recording, setStatus() {} };
  const starts = [], stops = [], prompts = [];
  let scan = snapshot([call], [call.window]), timer;
  window.setTimeout = fn => { timer = fn; return 1; };
  window.clearTimeout = () => {};
  const toast = Object.assign((title, options) => { prompts.push(options); return prompts.length; }, {
    dismiss() {}, success() {}, error() {},
  });
  const { TeamsAutoRecord } = loadTsModule(path('components/TeamsAutoRecord.tsx'), {
    react: hooks.react, sonner: { toast }, '@/lib/autoRecord': prefs,
    '@/contexts/RecordingStateContext': { useRecordingState: () => state, RecordingStatus: {STARTING:'starting', ERROR:'error'} },
    '@/hooks/useRecordingStart': { useStartRecordingSession: () => async source => {
      starts.push(source); state.isRecording = true; return {sessionId:'synthetic-recorder-id'};
    } },
    '@/services/teamsDetectionService': { getCallDetectionStatus: async () => {
      if (!scan) throw Error('poll failed'); return scan;
    } },
    '@/services/recordingService': { recordingService: {
      getRecordingState: async () => ({ is_recording: state.isRecording }),
      stopDetectedCall: async id => { stops.push(id); return true; },
    } },
  });
  hooks.render(TeamsAutoRecord);
  return { hooks, starts, stops, prompts, prefs, settings, state,
    scan: value => { scan = value; }, tick: async value => { scan = value; await timer(); await flush(); } };
}

test('only the detected recording is stopped after sustained confirmed absence', async () => {
  const app = poller('auto', true);
  await flush();
  assert.equal(app.starts.length, 1);
  await app.tick(snapshot([], [call.window]));
  await app.tick(snapshot());
  await app.tick(null);
  await app.tick(snapshot());
  await app.tick(snapshot());
  assert.deepEqual(app.stops, []);
  await app.tick(snapshot());
  assert.deepEqual(app.stops, ['synthetic-recorder-id']);
  await app.tick(snapshot());
  assert.equal(app.stops.length, 1);
  app.hooks.unmount();
});

test('a manual session or revoked auto-stop grant leaves the recorder running', async () => {
  for (const manual of [true,false]) {
    const app = poller('auto', true, manual);
    await flush();
    if (!manual) app.prefs.setCallSettings({ ...app.settings, autoStop:false });
    for (let i=0;i<4;i++) await app.tick(snapshot());
    assert.deepEqual(app.stops, []);
    if (manual) assert.equal(app.starts.length, 0);
    app.hooks.unmount();
  }
});

test('stale prompts recheck call evidence and respect switching detection off', async () => {
  const app = poller('prompt', false);
  await flush();
  const prompt = app.prompts[0];
  app.scan(snapshot());
  prompt.action.onClick();
  await flush();
  assert.equal(app.starts.length, 0);
  app.scan(snapshot([call], [call.window]));
  app.prefs.setCallSettings({ ...app.settings, mode:'off' });
  prompt.action.onClick();
  await flush();
  assert.equal(app.starts.length, 0);
  app.hooks.unmount();
});
