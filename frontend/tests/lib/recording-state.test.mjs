import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView(delaySubscriptions = false, snapshotOverride) {
  const harness = createHookHarness();
  const callbacks = {};
  const subscriptions = [];
  const requests = [];
  const intervals = new Map();
  let nextTimer = 0;
  let unsubscribed = 0;
  let backendSessionId = 'session-current';
  const service = { getRecordingState() { const request = deferred(); requests.push(request); return request.promise; } };
  for (const name of ['Started', 'Stopped', 'Paused', 'Resumed']) {
    service[`onRecording${name}`] = callback => {
      callbacks[name] = name === 'Started'
        ? (mode, sessionId, generation = recordingGeneration(sessionId)) => callback(mode, sessionId, generation)
        : name === 'Stopped'
          ? payload => callback(payload ? {
            ...payload, recording_generation: payload.recording_generation ?? recordingGeneration(payload.session_id),
          } : payload)
          : callback;
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
      '@/services/knowledgeService': { knowledgeService: { liveSnapshot: () => snapshotOverride
        ? snapshotOverride() : Promise.resolve(liveSnapshot(backendSessionId)) } },
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
    setBackendSession(id) { backendSessionId = id; },
    render: () => harness.render(() => RecordingStateProvider({ children: null })).value,
    tick() { for (const callback of intervals.values()) callback(); },
  };
}

const recording = { session_id: 'session-current', recording_generation: '2', is_recording: true, is_paused: false, is_active: true, recording_duration: 10, active_duration: 10, recording_mode: 'live' };

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

test('first stopping status notifies lifecycle consumers synchronously before React rerenders', async () => {
  const view = createView(); const state = view.render(); const observed = [];
  try {
    assert.equal(typeof state.subscribeLifecycle, 'function', 'the recording owner exposes lifecycle invalidation');
    const cleanup = state.subscribeLifecycle(status => observed.push(status));
    state.setStatus('stopping'); assert.deepEqual(observed, ['stopping']);
    cleanup(); state.setStatus('saving'); assert.deepEqual(observed, ['stopping']);
  } finally { view.unmount(); await flush(); }
});

test('a stopped event from the prior backend session cannot invalidate the replacement recording', async () => {
  const view = createView(); const observed = []; view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-current'); await flush();
    const state = view.render(); state.subscribeLifecycle(status => observed.push(status));
    view.callbacks.Stopped({ session_id: 'session-prior', message: 'Old recording finished' });
    assert.deepEqual(observed, [], 'stale Stop must not reach the global Live owner');
    assert.equal(view.render().isRecording, true); assert.equal(view.render().status, 'recording');
    view.callbacks.Stopped({ session_id: 'session-current', message: 'Current recording finished' });
    assert.deepEqual(observed, ['stopping']); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('a delayed duplicate start cannot reverse synchronous Stop while the backend still reports the old session', async () => {
  const view = createView(); view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-current'); await flush();
    view.requests[0].resolve(recording); await flush();
    const state = view.render(); state.setStatus('stopping');
    view.callbacks.Started('audio_only', 'session-current'); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().sessionMode, 'live');
    view.callbacks.Stopped({ session_id: 'session-current' });
    view.render().setStatus('starting'); view.setBackendSession('session-next');
    view.callbacks.Started('live', 'session-next'); await flush();
    assert.equal(view.render().status, 'recording'); assert.equal(view.render().isRecording, true);
  } finally { view.unmount(); }
});

test('a prior started event and prior Stop cannot replace the authoritative current recording identity or phase', async () => {
  const view = createView(); const observed = []; view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-current'); await flush();
    view.render().subscribeLifecycle(status => observed.push(status));
    view.callbacks.Started('audio_only', 'session-prior'); await flush();
    view.callbacks.Stopped({ session_id: 'session-prior' });
    const current = view.render();
    assert.deepEqual(observed, []); assert.equal(current.status, 'recording');
    assert.equal(current.sessionMode, 'live'); assert.equal(current.isRecording, true);
    view.callbacks.Stopped({ session_id: 'session-current' });
    assert.deepEqual(observed, ['stopping']); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('a pending authoritative start check cannot apply after synchronous Stop', async () => {
  const check = deferred(); const view = createView(false, () => check.promise); view.render(); await flush();
  try {
    view.render().setStatus('starting');
    view.callbacks.Started('live', 'session-current'); view.render().setStatus('stopping');
    check.resolve(liveSnapshot('session-current')); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('a delayed prior Stop cannot invalidate a genuine replacement while its authoritative start check is pending', async () => {
  const replacement = deferred(); let backendId = 'session-a'; const checks = [];
  const view = createView(false, () => {
    checks.push(backendId);
    return backendId === 'session-b' ? replacement.promise : Promise.resolve(liveSnapshot(backendId));
  });
  const observed = []; view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a'); await flush();
    assert.equal(view.render().status, 'recording');
    view.render().setStatus('stopping'); view.callbacks.Stopped({ session_id: 'session-a' });
    view.render().setStatus('starting'); backendId = 'session-b';
    view.render().subscribeLifecycle(status => observed.push(status));
    view.callbacks.Started('live', 'session-b'); await flush();
    assert.equal(checks.at(-1), 'session-b', 'the production owner has started the authoritative identity check');
    view.callbacks.Stopped({ session_id: 'session-a' });
    assert.deepEqual(observed, [], 'the already completed recording must not clear the replacement Live owner');
    assert.equal(view.render().status, 'starting');
    replacement.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'recording'); assert.equal(view.render().isRecording, true);
    view.callbacks.Stopped({ session_id: 'session-a' }); assert.deepEqual(observed, []);
    view.callbacks.Stopped({ session_id: 'session-b' });
    assert.deepEqual(observed, ['stopping']); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('a genuine replacement Stop invalidates its pending start check without accepting the retired identity', async () => {
  const replacement = deferred(); let backendId = 'session-a';
  const view = createView(false, () => backendId === 'session-b'
    ? replacement.promise : Promise.resolve(liveSnapshot(backendId)));
  const observed = []; view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a'); await flush();
    view.render().setStatus('stopping'); view.callbacks.Stopped({ session_id: 'session-a' });
    view.render().setStatus('starting'); backendId = 'session-b';
    view.render().subscribeLifecycle(status => observed.push(status));
    view.callbacks.Started('live', 'session-b'); await flush();
    view.callbacks.Stopped({ session_id: 'session-b' });
    assert.deepEqual(observed, ['stopping'], 'current native Stop must reach Live even before start validation completes');
    replacement.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

function recordingGeneration(sessionId) {
  return {
    'session-prior': '1', 'session-current': '2', 'session-next': '3',
    'session-c': '1', 'session-a': '2', 'session-b': '3',
  }[sessionId];
}

function liveSnapshot(sessionId, generation = recordingGeneration(sessionId)) {
  return {
    session_id: sessionId, recording_generation: generation, finalized_through_seconds: 0, segments: [],
    transcription_incomplete: false, transcription_available: true,
  };
}

test('a rejected older start cannot erase the genuine pending identity before its native Stop', async () => {
  const replacement = deferred(); const older = deferred();
  let snapshot = Promise.resolve(liveSnapshot('session-a'));
  const view = createView(false, () => snapshot); const observed = [];
  view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a'); await flush();
    assert.equal(view.render().status, 'recording');
    view.render().setStatus('stopping'); view.callbacks.Stopped({ session_id: 'session-a' });
    view.render().setStatus('starting');
    view.render().subscribeLifecycle(status => observed.push(status));
    snapshot = replacement.promise; view.callbacks.Started('live', 'session-b');
    snapshot = older.promise; view.callbacks.Started('audio_only', 'session-c');
    older.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'starting', 'the mismatched older event cannot promote a recording');
    view.callbacks.Stopped({ session_id: 'session-b' });
    assert.deepEqual(observed, ['stopping'], 'rejecting C must not make the current native Stop(B) disappear');
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
    replacement.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'stopping', 'the pre-Stop snapshot must not revive B');
    assert.equal(view.render().isRecording, false); assert.equal(view.intervals.size, 0);
  } finally { view.unmount(); }
});

test('a stale Stop for a second unverified start cannot invalidate the genuine pending recording', async () => {
  const replacement = deferred(); const older = deferred();
  let snapshot = Promise.resolve(liveSnapshot('session-a'));
  const view = createView(false, () => snapshot); const observed = [];
  view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a'); await flush();
    view.render().setStatus('stopping'); view.callbacks.Stopped({ session_id: 'session-a' });
    view.render().setStatus('starting');
    view.render().subscribeLifecycle(status => observed.push(status));
    snapshot = replacement.promise; view.callbacks.Started('live', 'session-b');
    snapshot = older.promise; view.callbacks.Started('audio_only', 'session-c');
    view.callbacks.Stopped({ session_id: 'session-c' });
    assert.deepEqual(observed, [], 'unverified C must not gain lifecycle ownership from event arrival order');
    assert.equal(view.render().status, 'starting');
    older.resolve(liveSnapshot('session-b')); await flush();
    replacement.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'recording'); assert.equal(view.render().isRecording, true);
    assert.equal(view.render().sessionMode, 'live', 'the genuine event retains its recording mode');
    view.callbacks.Stopped({ session_id: 'session-b' });
    assert.deepEqual(observed, ['stopping']); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('an older start arriving first cannot own Stop while the genuine replacement identity is unverified', async () => {
  const replacement = deferred(); const older = deferred();
  let snapshot = Promise.resolve(liveSnapshot('session-a'));
  const view = createView(false, () => snapshot); const observed = [];
  view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a'); await flush();
    view.render().setStatus('stopping'); view.callbacks.Stopped({ session_id: 'session-a' });
    view.render().setStatus('starting');
    view.render().subscribeLifecycle(status => observed.push(status));
    snapshot = older.promise; view.callbacks.Started('audio_only', 'session-c');
    snapshot = replacement.promise; view.callbacks.Started('live', 'session-b');
    view.callbacks.Stopped({ session_id: 'session-c' });
    assert.deepEqual(observed, [], 'pinning the first unverified event would let stale C stop B');
    assert.equal(view.render().status, 'starting');
    view.callbacks.Stopped({ session_id: 'session-b' });
    assert.deepEqual(observed, ['stopping'], 'the current native Stop must invalidate either arrival order');
    replacement.resolve(liveSnapshot('session-b')); older.resolve(liveSnapshot('session-b')); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
    assert.equal(view.intervals.size, 0);
  } finally { view.unmount(); }
});

test('adjacent native generations beyond JavaScript integer precision still distinguish replacement Stop ownership', async () => {
  const replacement = deferred();
  let snapshot = Promise.resolve(liveSnapshot('session-a', '9007199254740992'));
  const view = createView(false, () => snapshot); const observed = [];
  view.render(); await flush();
  try {
    view.callbacks.Started('live', 'session-a', '9007199254740992'); await flush();
    view.render().setStatus('stopping');
    view.callbacks.Stopped({ session_id: 'session-a', recording_generation: '9007199254740992' });
    view.render().setStatus('starting');
    view.render().subscribeLifecycle(status => observed.push(status));
    snapshot = replacement.promise;
    view.callbacks.Started('live', 'session-b', '9007199254740993');
    view.callbacks.Stopped({ session_id: 'session-a', recording_generation: '9007199254740992' });
    assert.deepEqual(observed, [], 'the lower generation remains retired without numeric rounding');
    view.callbacks.Stopped({ session_id: 'session-b', recording_generation: '9007199254740993' });
    assert.deepEqual(observed, ['stopping'], 'the adjacent greater generation owns the genuine Stop');
    replacement.resolve(liveSnapshot('session-b', '9007199254740993')); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('matching UUID alone cannot promote a start with a different canonical native generation', async () => {
  const view = createView(false, () => Promise.resolve(liveSnapshot('session-b', '4')));
  view.render(); await flush();
  try {
    view.render().setStatus('starting');
    view.callbacks.Started('live', 'session-b', '3'); await flush();
    assert.equal(view.render().status, 'starting'); assert.equal(view.render().isRecording, false);
    assert.equal(view.intervals.size, 0, 'a mismatched producer pair cannot start active polling');
  } finally { view.unmount(); }
});

test('started-event bursts coalesce canonical checks and current Stop clears queued promotion', async () => {
  const check = deferred(); let checks = 0;
  const view = createView(false, () => { checks++; return check.promise; });
  const observed = []; view.render(); await flush();
  try {
    view.render().setStatus('starting');
    view.render().subscribeLifecycle(status => observed.push(status));
    for (let generation = 1; generation <= 32; generation++) {
      view.callbacks.Started('live', `session-${generation}`, String(generation));
      view.callbacks.Started('audio_only', `session-${generation}`, String(generation));
    }
    assert.equal(checks, 1, 'only one canonical invocation may remain outstanding during the burst');
    view.callbacks.Stopped({ session_id: 'session-31', recording_generation: '31' });
    assert.deepEqual(observed, [], 'the newer native producer owns lifecycle invalidation');
    view.callbacks.Stopped({ session_id: 'session-32', recording_generation: '32' });
    assert.deepEqual(observed, ['stopping']);
    check.resolve(liveSnapshot('session-32', '32')); await flush();
    assert.equal(checks, 1, 'Stop must abandon the coalesced candidate without another lookup');
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

async function rejectedStartingView() {
  const checks = [];
  const view = createView(false, () => {
    const check = deferred(); checks.push(check); return check.promise;
  });
  view.render(); await flush();
  view.render().setStatus('starting');
  view.callbacks.Started('live', 'session-b', '3');
  assert.equal(checks.length, 1);
  checks[0].reject(new Error('Public fixture metadata failure')); await flush();
  return { view, checks };
}

function retryVerification(view) {
  const state = view.render();
  assert.equal(typeof state.retryVerification, 'function', 'the global recording owner exposes manual verification retry');
  state.retryVerification();
}

test('rejected initial Started verification is visible and manual retry restores global recording and polling', async () => {
  const { view, checks } = await rejectedStartingView();
  try {
    assert.equal(view.render().status, 'starting'); assert.equal(view.intervals.size, 0);
    assert.match(view.render().verificationError ?? '', /verify|recording|retry/i, 'the current verification failure is visible to consumers');
    retryVerification(view);
    assert.equal(checks.length, 2, 'retry performs a fresh canonical lookup without a second Started event');
    checks[1].resolve(liveSnapshot('session-b', '3')); await flush();
    const recovered = view.render();
    assert.equal(recovered.status, 'recording'); assert.equal(recovered.isRecording, true);
    assert.equal(recovered.sessionMode, 'live'); assert.equal(recovered.verificationError, null);
    assert.equal(view.intervals.size, 1, 'successful verification restores global recording-state polling');
    view.tick(); assert.equal(view.requests.length, 2);
    assert.equal(checks.length, 2, 'polling does not repeat successful Started verification');
  } finally { view.unmount(); }
});

test('current Stop synchronously retires a deferred recording-verification retry', async () => {
  const { view, checks } = await rejectedStartingView(); const observed = [];
  try {
    view.render().subscribeLifecycle(status => observed.push(status));
    retryVerification(view); assert.equal(checks.length, 2);
    view.callbacks.Stopped({ session_id: 'session-b', recording_generation: '3' });
    assert.deepEqual(observed, ['stopping'], 'Stop reaches consumers before retry completion or a React render');
    checks[1].resolve(liveSnapshot('session-b', '3')); await flush();
    assert.equal(view.render().status, 'stopping'); assert.equal(view.render().isRecording, false);
    assert.equal(view.render().verificationError, null); assert.equal(view.intervals.size, 0);
    retryVerification(view); await flush();
    assert.equal(checks.length, 2, 'a stopped producer cannot be retried');
  } finally { view.unmount(); }
});

test('a newer producer supersedes a deferred recording retry and obtains its own fresh canonical result', async () => {
  const { view, checks } = await rejectedStartingView(); const observed = [];
  try {
    retryVerification(view); assert.equal(checks.length, 2);
    view.callbacks.Started('audio_only', 'session-newest', '4');
    assert.equal(checks.length, 2, 'the Started lane coalesces the newer candidate behind the retry');
    checks[1].resolve(liveSnapshot('session-b', '3')); await flush();
    assert.equal(view.render().status, 'starting'); assert.equal(view.render().isRecording, false);
    assert.equal(checks.length, 3, 'the old retry result cannot stand in for the newer producer');
    checks[2].resolve({ ...liveSnapshot('session-newest', '4'), transcription_available: false }); await flush();
    assert.equal(view.render().status, 'recording'); assert.equal(view.render().sessionMode, 'audio_only');
    assert.equal(view.intervals.size, 1);
    view.render().subscribeLifecycle(status => observed.push(status));
    view.callbacks.Stopped({ session_id: 'session-b', recording_generation: '3' });
    assert.deepEqual(observed, []); assert.equal(view.render().isRecording, true);
    view.callbacks.Stopped({ session_id: 'session-newest', recording_generation: '4' });
    assert.deepEqual(observed, ['stopping']); assert.equal(view.render().isRecording, false);
  } finally { view.unmount(); }
});

test('recording retry failures remain bounded and matching UUID alone never recovers a producer', async () => {
  const { view, checks } = await rejectedStartingView();
  try {
    for (let i = 0; i < 32; i++) retryVerification(view);
    assert.equal(checks.length, 2, 'repeated manual clicks share one outstanding Started verification');
    checks[1].reject(new Error('Public fixture second metadata failure')); await flush();
    assert.equal(checks.length, 2, 'a rejected retry does not create an automatic retry storm');
    assert.equal(view.render().status, 'starting'); assert.equal(view.intervals.size, 0);
    assert.match(view.render().verificationError ?? '', /verify|recording|retry/i);
    retryVerification(view); assert.equal(checks.length, 3);
    checks[2].resolve(liveSnapshot('session-b', '4')); await flush();
    assert.equal(view.render().status, 'starting'); assert.equal(view.render().isRecording, false);
    assert.equal(view.intervals.size, 0, 'a different native generation cannot bypass exact-pair verification');
    retryVerification(view); assert.equal(checks.length, 4);
    checks[3].resolve(liveSnapshot('session-b', '3')); await flush();
    assert.equal(view.render().status, 'recording'); assert.equal(view.render().verificationError, null);
    assert.equal(view.intervals.size, 1);
  } finally { view.unmount(); }
});
