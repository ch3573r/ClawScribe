import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

// The decisions and timing of call detection (when a call starts a recording,
// when its end stops it, how long a prompt waits) are made by the core and
// tested there. These tests cover what stays in the window: carrying out the
// starts the core asks for without navigating, showing prompts and notices,
// and keeping the core's settings in line.

const path = name => fileURLToPath(new URL('../../src/' + name, import.meta.url));

function setupWindow() {
  const entries = new Map();
  const target = new EventTarget();
  target.localStorage = { getItem: key => entries.get(key) ?? null, setItem: (key, value) => entries.set(key, value) };
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

function app(options = {}) {
  const prefs = setupWindow();
  if (options.local) prefs.setCallSettings(options.local);
  const hooks = createHookHarness();
  const handlers = new Map();
  const unlistened = [];
  const state = { isRecording: false, isStarting: false, isStopping: false, isProcessing: false, isSaving: false, setStatus: (...args) => statuses.push(args), ...options.state };
  const statuses = [], starts = [], answers = [], toasts = [], dismissed = [], confirmed = [], declined = [], failures = [], corePushed = [];
  const navigation = [];
  // Navigation or a reload would go through these; the component must not touch them.
  window.sessionStorage = { setItem: (...args) => navigation.push(['sessionStorage', ...args]) };
  window.location = { assign: (...args) => navigation.push(['location', ...args]) };
  const toast = Object.assign((title, details) => { toasts.push({ title, ...details }); return toasts.length; }, {
    dismiss: id => dismissed.push(id), success: title => toasts.push({ title, tone: 'success' }), error: title => toasts.push({ title, tone: 'error' }),
  });
  let coreSettings = options.core ?? null;
  const { RecordingRequests } = loadTsModule(path('components/RecordingRequests.tsx'), {
    react: hooks.react,
    sonner: { toast },
    '@tauri-apps/api/core': { invoke: async (command, { key }) => {
      assert.equal(command, 'dismiss_call_detection_prompt');
      declined.push(key);
    } },
    '@tauri-apps/api/event': { listen: async (name, handler) => { handlers.set(name, handler); return () => unlistened.push(name); } },
    '@/lib/autoRecord': prefs,
    '@/contexts/RecordingStateContext': { useRecordingState: () => state, RecordingStatus: { ERROR: 'error' } },
    '@/hooks/useRecordingStart': {
      START_BUSY: 'recording_start_busy',
      reportStartFailure: error => failures.push(error),
      useStartRecordingSession: () => async (source, requestId) => {
        starts.push([source, requestId]);
        if (options.start) return options.start(source, requestId);
        state.isRecording = true;
        return 'synthetic-session';
      },
    },
    '@/services/callDetectionService': {
      getCoreCallSettings: async () => coreSettings,
      setCoreCallSettings: async value => { corePushed.push(value); coreSettings = value; },
      getPendingStartRequest: async () => options.pendingRequest ?? null,
      getPendingCallPrompt: async () => options.pendingPrompt ?? null,
      finishStartRequest: async (requestId, status) => { answers.push({ requestId, status }); return true; },
      confirmCallPrompt: async key => { confirmed.push(key); },
    },
  });
  const render = () => hooks.render(() => RecordingRequests({ ready: options.ready ?? true }));
  render();
  return {
    render, hooks, prefs, state, statuses, starts, answers, toasts, dismissed, confirmed, declined, failures, corePushed, navigation,
    emit: async (name, payload, fallback = payload) => {
      if (name === 'call-detection-prompt') options.pendingPrompt = fallback ?? null;
      if (name === 'call-detection-prompt-cleared') options.pendingPrompt = null;
      await handlers.get(name)({ payload });
      await flush();
    },
    has: name => handlers.has(name), unlistened,
  };
}

const request = (source, requestId = 'request-1', provider = null) => ({ requestId, source, provider });

test('a queued start arriving before the Stop render waits and rechecks native ownership', async () => {
  for (const expired of [true, false]) {
    const pending = request('after_save', 'after-commit');
    const options = { pendingRequest: pending, state: { isStopping: true, isRecording: true } };
    const view = app(options);
    try {
      await flush();
      assert.deepEqual(view.starts, []);
      if (expired) options.pendingRequest = null;
      view.state.isRecording = false;
      view.state.isStopping = false;
      view.render();
      await flush();
      assert.deepEqual(view.starts, expired ? [] : [['remote_request', pending.requestId]]);
    } finally { view.hooks.unmount(); }
  }
});

test('a start from the tray, a shortcut or the command line forwards each native request once', async () => {
  const window_ = app();
  await flush();
  for (const source of ['tray', 'shortcut', 'cli']) {
    window_.state.isRecording = false;
    await window_.emit('recording-start-requested', request(source, `request-${source}`));
  }
  assert.deepEqual(window_.starts, ['tray', 'shortcut', 'cli'].map(source => ['remote_request', `request-${source}`]));
  assert.deepEqual(window_.answers, [], 'native owns success once the request is claimed');
  assert.deepEqual(window_.navigation, [], 'the page and its drafts stay where they are');
  // A request the core repeats is not acted on twice.
  window_.state.isRecording = false;
  await window_.emit('recording-start-requested', request('tray', 'request-tray'));
  assert.equal(window_.starts.length, 3);
  window_.hooks.unmount();
});

test('a start while another recording step is under way starts nothing and says so', async () => {
  for (const busy of ['isRecording', 'isStarting', 'isStopping', 'isProcessing', 'isSaving']) {
    const window_ = app({ state: { [busy]: true } });
    await flush();
    await window_.emit('recording-start-requested', request('tray'));
    assert.deepEqual(window_.starts, [], busy);
    assert.deepEqual(window_.answers.map(answer => answer.status), ['busy'], busy);
    window_.hooks.unmount();
  }
});

test('a detected call passes its request ID for native validation', async () => {
  const window_ = app();
  await flush();
  await window_.emit('recording-start-requested', request('call', 'request-call', 'teams'));
  assert.deepEqual(window_.starts, [['detected_call', 'request-call']]);
  assert.deepEqual(window_.answers, []);
  window_.hooks.unmount();
});

test('a calendar call uses the invitation bound to the native claim and names the event for confirmation', async () => {
  const view = app();
  await flush();
  await view.emit('call-detection-prompt', { key: 'join-1', provider: 'teams', eventTitle: 'Planning', retry: false });
  assert.match(view.toasts.at(-1).description, /Planning/);
  await view.emit('recording-start-requested', request('calendar_call', 'selected-start', 'teams'));
  assert.deepEqual(view.starts, [['calendar_call', 'selected-start']]);
  view.hooks.unmount();
});

test('an early failure is answered as retryable except a full disk', async () => {
  const failing = app({ start: async () => { throw new Error('device missing'); } });
  await flush();
  await failing.emit('recording-start-requested', request('tray'));
  assert.equal(failing.failures.length, 1);
  assert.deepEqual(failing.answers.map(answer => answer.status), ['failed']);
  failing.hooks.unmount();

  const full = app({ start: async () => { throw 'Not enough free disk space to record. Free some space and try again.'; } });
  await flush();
  await full.emit('recording-start-requested', request('call', 'request-full', 'zoom'));
  assert.deepEqual(full.answers.map(answer => answer.status), ['refused']);
  full.hooks.unmount();
});

test('a desktop request during setup is refused and starts no capture', async () => {
  const window_ = app({ ready: false, pendingRequest: request('call', 'setup-call', 'teams') });
  await flush();
  assert.deepEqual(window_.starts, []);
  assert.deepEqual(window_.answers, [{ requestId: 'setup-call', status: 'refused' }]);
  window_.hooks.unmount();
});

test('a call prompt asks first, goes through the core and goes away with the call', async () => {
  const window_ = app();
  await flush();
  await window_.emit('call-detection-prompt', { key: 'room-a', provider: 'googleMeet', retry: false });
  const prompt = window_.toasts.at(-1);
  assert.equal(prompt.title, 'Google Meet call detected');
  assert.equal(prompt.action.label, 'Record');
  prompt.action.onClick();
  await flush();
  assert.deepEqual(window_.confirmed, ['room-a']);
  await window_.emit('call-detection-prompt-cleared', 'room-a');
  assert.equal(window_.dismissed.length >= 1, true);
  // An answer to a prompt that is no longer showing does nothing.
  prompt.action.onClick();
  await flush();
  assert.deepEqual(window_.confirmed, ['room-a']);
  window_.hooks.unmount();
});

test('a failed automatic start offers to try again', async () => {
  const window_ = app();
  await flush();
  await window_.emit('call-detection-prompt', { key: 'room-a', provider: 'teams', retry: true });
  const prompt = window_.toasts.at(-1);
  assert.equal(prompt.title, 'Could not record the Microsoft Teams call');
  assert.equal(prompt.action.label, 'Try again');
  window_.hooks.unmount();
});

test('a native prompt produces no toast and a fallback prompt is only shown once', async t => {
  const view = app();
  t.after(view.hooks.unmount);
  await flush();
  // The native event is a change signal. The main window can only fetch toast fallbacks.
  await view.emit('call-detection-prompt', { key: 'native', provider: 'teams', retry: false }, null);
  assert.deepEqual(view.toasts, []);

  const fallback = { key: 'fallback', provider: 'teams', retry: false };
  await view.emit('call-detection-prompt', fallback);
  await view.emit('call-detection-prompt', fallback);
  assert.equal(view.toasts.length, 1);
  assert.equal(view.toasts[0].duration, 20_000);
});

test('a delayed prompt snapshot cannot restore a dismissed toast', async t => {
  const stale = deferred();
  const view = app({ pendingPrompt: stale.promise });
  t.after(view.hooks.unmount);
  await flush();
  await view.emit('call-detection-prompt-cleared', 'dismissed');
  stale.resolve({ key: 'dismissed', provider: 'teams', retry: false });
  await flush();
  assert.deepEqual(view.toasts, []);
});

test('notices from the core say what happened to the call recording', async () => {
  const window_ = app();
  await flush();
  await window_.emit('call-detection-notice', { kind: 'started', provider: 'webex', label: 'Webex' });
  await window_.emit('call-detection-notice', { kind: 'stopped', provider: 'webex', label: 'Webex' });
  await window_.emit('call-detection-notice', { kind: 'stop-failed', provider: 'webex', label: 'Webex' });
  assert.deepEqual(window_.toasts.map(toast => toast.title), [
    'Recording started for Webex',
    'Call ended. Recording stopped.',
    'Could not finish the call recording. ClawScribe will retry. You can also stop it manually.',
  ]);
  window_.hooks.unmount();
});

test('a request or prompt that arrived before the window listened is picked up', async () => {
  const window_ = app({
    pendingRequest: request('cli', 'early-request'),
    pendingPrompt: { key: 'room-b', provider: 'zoom', retry: false },
  });
  await flush();
  await flush();
  assert.deepEqual(window_.starts, [['remote_request', 'early-request']]);
  assert.equal(window_.toasts.at(-1).title, 'Zoom call detected');
  window_.hooks.unmount();
});

test('the core keeps its own call settings and receives the saved ones the first time', async () => {
  const saved = { mode: 'prompt', providers: ['teams', 'zoom'], autoStop: true };
  const first = app({ local: saved });
  await flush();
  assert.equal(JSON.stringify(first.corePushed), JSON.stringify([saved]), 'the first time the saved settings are handed over');
  first.hooks.unmount();

  const core = { mode: 'auto', providers: ['googleMeet'], autoStop: false };
  const second = app({ local: saved, core });
  await flush();
  assert.deepEqual(second.corePushed, [], 'the core keeps its own');
  assert.deepEqual(JSON.parse(JSON.stringify(second.prefs.getCallSettings())), core);
  second.hooks.unmount();

  const same = app({ local: saved, core: saved });
  await flush();
  assert.deepEqual(same.corePushed, []);
  same.hooks.unmount();
});

test('unmount removes the listeners while the claimed start remains native-owned', async () => {
  const pending = deferred();
  const window_ = app({ start: () => pending.promise });
  await flush();
  void window_.emit('recording-start-requested', request('tray', 'slow'));
  await flush();
  window_.hooks.unmount();
  assert.ok(window_.unlistened.includes('recording-start-requested'));
  assert.ok(window_.unlistened.includes('call-detection-prompt'));
  assert.ok(window_.unlistened.includes('call-detection-notice'));
  pending.resolve('synthetic-session');
  await flush();
  assert.deepEqual(window_.answers, []);
});

test('a toast prompt can be declined explicitly and timing out leaves it pending', async t => {
  const view = app();
  t.after(() => view.hooks.unmount());
  await flush();
  const call = { key: 'decline-fixture', provider: 'teams' };
  await view.emit('call-detection-prompt', call);
  const prompt = view.toasts.at(-1);
  prompt.onAutoClose?.();
  assert.deepEqual(view.declined, []);
  assert.equal(prompt.cancel.label, 'Not now');
  prompt.cancel.onClick();
  await flush();
  assert.deepEqual(view.declined, [call.key]);
  assert.deepEqual(view.confirmed, []);
});
