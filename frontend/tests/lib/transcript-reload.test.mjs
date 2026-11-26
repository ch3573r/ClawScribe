import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('a late live row is inserted with logarithmic comparisons', () => {
  const { insertLiveTranscripts } = loadTsModule('src/contexts/live-transcript-order.ts');
  let comparisons = 0;
  const current = Array.from({ length: 10_000 }, (_, index) => ({
    sequence_id: index * 2,
    get chunk_start_time() { comparisons++; return index * 2; },
  }));
  const late = { sequence_id: 5_001, chunk_start_time: 5_001 };
  const ordered = insertLiveTranscripts(current, [late]);
  assert.equal(ordered.length, current.length + 1);
  assert.equal(ordered[2_500], current[2_500]);
  assert.equal(ordered[2_501], late);
  assert.equal(ordered[2_502], current[2_501]);
  assert.ok(comparisons <= Math.ceil(Math.log2(current.length)) + 1);
});

function transcriptView(titleRegistration) {
  const hooks = createHookHarness();
  const histories = [];
  const cacheWrites = [];
  const titleWrites = [];
  let started, updated, titleChanged;
  let titleUnsubscribes = 0;
  let subscriptions = 0;
  const state = { isRecording: true, sessionId: 'first' };
  const { TranscriptProvider } = loadTsModule('src/contexts/TranscriptContext.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: (_type, props) => props },
    './RecordingStateContext': { useRecordingState: () => state },
    '@tauri-apps/api/event': { listen: async (event, callback) => {
      assert.equal(event, 'recording-title-changed');
      titleChanged = callback;
      await titleRegistration;
      return () => { titleUnsubscribes++; };
    } },
    '@/services/recordingService': { recordingService: {
      onRecordingStarted: async callback => { started = callback; return () => {}; },
      getRecordingMeetingName: async () => state.sessionId,
    } },
    '@/services/transcriptService': { transcriptService: {
      onTranscriptUpdate: async callback => {
        subscriptions++;
        updated = callback;
        return () => { updated = undefined; };
      },
      getTranscriptHistory: () => {
        const request = deferred();
        histories.push(request);
        return request.promise;
      },
    } },
    '@/services/indexedDBService': { indexedDBService: {
      saveMeetingMetadata: async () => {},
      updateMeetingTitle: async (...args) => { titleWrites.push(args); },
      saveTranscript: async (id, update) => { cacheWrites.push([id, update.sequence_id]); },
    } },
  });
  return {
    hooks, histories, state, cacheWrites, titleWrites, subscriptions: () => subscriptions,
    titleChanged: (session_id, title) => titleChanged?.({ payload: { session_id, title } }),
    titleUnsubscribes: () => titleUnsubscribes,
    start: (...args) => started(...args), update: value => updated?.(value),
    render: () => hooks.render(() => TranscriptProvider({ children: null })).value,
  };
}

test('title events and the editor update the current live title and its recovery metadata', async t => {
  const previousStorage = globalThis.sessionStorage;
  globalThis.sessionStorage = { setItem() {} };
  const view = transcriptView();
  t.after(() => { view.hooks.unmount(); globalThis.sessionStorage = previousStorage; });
  view.render();
  await flush();
  view.start('live', 'first', '1', { recording_id: 'cache-first', meeting_name: 'Calendar subject' });
  view.titleChanged('other', 'Unrelated');
  assert.equal(view.render().meetingTitle, 'Calendar subject');
  view.titleChanged('first', 'Recording');
  assert.equal(view.render().meetingTitle, 'Recording');
  view.render().setActiveRecordingTitle('first', 'Edited title');
  view.render().clearTranscripts();
  view.titleChanged('first', 'Retired event');
  await flush();
  assert.equal(view.render().meetingTitle, 'Edited title');
  assert.deepEqual(view.titleWrites, [
    ['cache-first', 'first', 'Recording'],
    ['cache-first', 'first', 'Edited title'],
  ]);
});

test('a title event after reload still targets the cache entry for the matching live session', async t => {
  const previousStorage = globalThis.sessionStorage;
  globalThis.sessionStorage = { getItem: () => 'cache-first' };
  const view = transcriptView();
  t.after(() => { view.hooks.unmount(); globalThis.sessionStorage = previousStorage; });
  view.render();
  await flush();
  view.titleChanged('first', 'Recording');
  view.histories[0].resolve([]);
  await flush();
  assert.equal(view.render().meetingTitle, 'Recording');
  assert.deepEqual(view.titleWrites, [['cache-first', 'first', 'Recording']]);
});

for (const pending of [false, true]) {
  test(`title listener cleanup handles ${pending ? 'pending' : 'completed'} registration`, async () => {
    const registration = deferred();
    const view = transcriptView(registration.promise);
    view.render();
    if (!pending) {
      registration.resolve();
      await flush();
    }
    view.hooks.unmount();
    view.titleChanged('first', 'After cleanup');
    registration.resolve();
    await flush();
    assert.equal(view.titleUnsubscribes(), 1);
    assert.equal(view.render().meetingTitle, '+ New Call');
  });
}

test('a saved title updates its recording and an older save cannot rename the next one', async t => {
  const view = transcriptView();
  t.after(() => view.hooks.unmount());
  view.render();
  await flush();
  const saveTitle = view.render().setActiveRecordingTitle;
  saveTitle('first', 'Edited meeting');
  view.histories[0].resolve([]);
  await flush();
  assert.equal(view.render().meetingTitle, 'Edited meeting');

  view.state.sessionId = 'second';
  view.render();
  await flush();
  view.histories[1].resolve([]);
  await flush();
  saveTitle('first', 'Late edit');
  assert.equal(view.render().meetingTitle, 'second');
});

test('a delayed reload merges history with live updates and retains word timings', async () => {
  const view = transcriptView();
  view.render().addTranscript({ text: 'Live', sequence_id: 2, timestamp: '12:01', source: 'Me' });
  view.render();
  await flush();
  const words = [{ text: 'Saved', start: 0, end: 1 }];
  view.histories[0].resolve([
    { id: '1', text: 'Saved', sequence_id: 1, display_time: '12:00', word_timestamps: words },
    { id: '2', text: 'Live', sequence_id: 2, display_time: '12:01' },
  ]);
  await flush();
  const rows = view.render().transcripts;
  assert.equal(rows.length, 2);
  assert.equal(rows[0].timestamp, '12:00');
  assert.deepEqual(rows[0].word_timestamps, words);
  assert.equal(rows[1].speaker, 'Me');
  assert.equal(view.histories.length, 1);
  view.hooks.unmount();
});

test('recording start retains its early segment and keeps the same listener through cache changes', async () => {
  const previousStorage = globalThis.sessionStorage;
  globalThis.sessionStorage = { setItem() {} };
  const view = transcriptView();
  view.state.isRecording = false;
  view.state.sessionId = null;
  try {
    view.render();
    await flush();
    const row = (session_id, sequence_id, chunk_start_time) => ({
      session_id, sequence_id, chunk_start_time, text: `Segment ${sequence_id}`,
    });
    view.update(row('new', 2, 20));
    view.start('live', 'new', '1', { recording_id: 'cache-new' });
    const context = view.render();
    view.update(row('new', 3, 20));
    view.update(row('new', 1, 10));
    view.update(row('new', 1, 10));
    context.flushBuffer();
    assert.deepEqual(Array.from(context.transcriptsRef.current, item => item.sequence_id), [1, 2, 3]);
    assert.deepEqual(view.cacheWrites, [['cache-new', 2], ['cache-new', 3], ['cache-new', 1]]);
    assert.equal(view.subscriptions(), 1);
    const updatedContext = view.render();
    assert.equal(view.render(), updatedContext);
    view.start('live', 'next', '2', { recording_id: 'cache-next' });
    view.update(row('new', 4, 25));
    view.update(row('next', 1, 1));
    context.flushBuffer();
    assert.deepEqual(Array.from(context.transcriptsRef.current, item => item.sequence_id), [1]);
    assert.deepEqual(view.cacheWrites.at(-1), ['cache-next', 1]);
    assert.equal(view.subscriptions(), 1);
  } finally {
    view.hooks.unmount();
    globalThis.sessionStorage = previousStorage;
  }
});

test('switching recording sessions invalidates an outstanding history read', async () => {
  const view = transcriptView();
  view.render();
  await flush();
  view.state.sessionId = 'second';
  view.render();
  await flush();
  view.histories[1].resolve([{ id: 'current', text: 'Current', sequence_id: 1, display_time: '12:00' }]);
  await flush();
  view.histories[0].resolve([{ id: 'stale', text: 'Stale', sequence_id: 1, display_time: '08:00' }]);
  await flush();
  assert.equal(view.render().transcripts[0].id, 'current');
  view.hooks.unmount();
});

test('recording history cannot replace the title supplied by Started', async t => {
  const previousStorage = globalThis.sessionStorage;
  globalThis.sessionStorage = { setItem() {} };
  const view = transcriptView();
  t.after(() => { view.hooks.unmount(); globalThis.sessionStorage = previousStorage; });
  view.render();
  await flush();
  view.start('live', 'first', '1', { recording_id: 'cache-first', meeting_name: 'Q4 planning' });
  view.histories[0].resolve([]);
  await flush();
  assert.equal(view.render().meetingTitle, 'Q4 planning');
});
