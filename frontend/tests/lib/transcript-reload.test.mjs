import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function transcriptView() {
  const hooks = createHookHarness();
  const histories = [];
  const state = { isRecording: true, sessionId: 'first' };
  const { TranscriptProvider } = loadTsModule('src/contexts/TranscriptContext.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: (_type, props) => props },
    './RecordingStateContext': { useRecordingState: () => state },
    '@/services/recordingService': { recordingService: {
      onRecordingStarted: async () => () => {},
      getRecordingMeetingName: async () => state.sessionId,
    } },
    '@/services/transcriptService': { transcriptService: {
      onTranscriptUpdate: async () => () => {},
      getTranscriptHistory: () => {
        const request = deferred();
        histories.push(request);
        return request.promise;
      },
    } },
  });
  return { hooks, histories, state, render: () => hooks.render(() => TranscriptProvider({ children: null })).value };
}

test('a delayed reload merges history with live updates and retains word timings', async () => {
  const view = transcriptView();
  view.render().addTranscript({ text: 'Live', sequence_id: 2, timestamp: '12:01', source: 'Me' });
  view.render();
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

test('switching recording sessions invalidates an outstanding history read', async () => {
  const view = transcriptView();
  view.render();
  view.state.sessionId = 'second';
  view.render();
  view.histories[1].resolve([{ id: 'current', text: 'Current', sequence_id: 1, display_time: '12:00' }]);
  await flush();
  view.histories[0].resolve([{ id: 'stale', text: 'Stale', sequence_id: 1, display_time: '08:00' }]);
  await flush();
  assert.equal(view.render().transcripts[0].id, 'current');
  view.hooks.unmount();
});
