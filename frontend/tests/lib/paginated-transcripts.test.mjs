import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView() {
  const harness = createHookHarness();
  const requests = [];
  const errors = [];
  const { usePaginatedTranscripts } = loadTsModule(fileURLToPath(new URL('../../src/hooks/usePaginatedTranscripts.ts', import.meta.url)), {
    react: harness.react,
    sonner: { toast: { error: message => errors.push(message) } },
    '@tauri-apps/api/core': { invoke(command, args) {
      const result = deferred();
      requests.push({ command, args, ...result });
      return result.promise;
    } },
  });
  let meetingId = 'first';
  const render = () => harness.render(() => usePaginatedTranscripts({ meetingId }));
  return { ...harness, requests, errors, render, navigate(id) { meetingId = id; return render(); } };
}

const row = (id, text = id) => ({ id, text, audio_start_time: 1, audio_end_time: 2 });
const page = (transcripts, has_more = false) => ({ transcripts, has_more, total_count: 300 });
function resolveInitial(requests, title = 'Meeting', text = title) {
  requests.find(request => request.command === 'api_get_meeting_metadata').resolve({ id: requests[0].args.meetingId, title });
  requests.find(request => request.command === 'api_get_meeting_transcripts').resolve(page([row(title, text)], true));
  requests.find(request => request.args.limit === 1000).resolve(page([row(title, text)]));
}

test('late metadata and transcript pages cannot replace a newly opened meeting', async () => {
  const view = createView();
  view.render();
  const oldRequests = view.requests.slice();
  view.navigate('second');
  resolveInitial(view.requests.slice(3), 'Second');
  await flush();
  resolveInitial(oldRequests, 'First');
  await flush();
  const current = view.render();
  assert.equal(current.metadata.id, 'second');
  assert.equal(current.transcripts[0].text, 'Second');
  assert.equal(current.timelineSegments[0].text, 'Second');
  assert.equal(current.isLoading, false);
  view.unmount();
});

test('a refetch ignores an earlier source page containing text from before a correction', async () => {
  const view = createView();
  view.render();
  resolveInitial(view.requests, 'Original');
  await flush();
  const source = view.render().revealSource('far-away', 250);
  const oldSource = view.requests.at(-1);
  const refresh = view.render().refetch();
  resolveInitial(view.requests.slice(-3), 'Corrected');
  await refresh;
  oldSource.resolve(page([row('far-away', 'Outdated text')]));
  await source;
  assert.equal(view.render().transcripts.length, 1);
  assert.equal(view.render().transcripts[0].text, 'Corrected');
  assert.equal(view.render().timelineSegments[0].text, 'Corrected');
  view.unmount();
});

test('jumping to a source does not skip sequential pages and duplicate load requests are coalesced', async () => {
  const view = createView();
  view.render();
  resolveInitial(view.requests);
  await flush();
  const source = view.render().revealSource('far-away', 250);
  assert.equal(view.requests.at(-1).args.offset, 200);
  view.requests.at(-1).resolve(page([row('far-away')]));
  await source;
  const current = view.render();
  const next = current.loadMore();
  const duplicate = current.loadMore();
  assert.equal(view.requests.length, 5);
  assert.equal(view.requests.at(-1).args.offset, 1);
  view.requests.at(-1).resolve(page([row('next')]));
  await Promise.all([next, duplicate]);
  assert.equal(view.render().loadedCount, 3);
  view.unmount();
});

test('leaving and reopening the same meeting loads it again', async () => {
  const view = createView();
  view.render();
  resolveInitial(view.requests);
  await flush();
  view.navigate(null);
  assert.equal(view.render().isLoading, false);
  assert.equal(view.render().timelineSegments.length, 0);
  view.navigate('first');
  assert.equal(view.requests.length, 6);
  resolveInitial(view.requests.slice(-3));
  await flush();
  assert.equal(view.render().loadedCount, 1);
  view.unmount();
});

const longRows = Array.from({ length: 250 }, (_, index) => ({
  ...row(`row-${index}`), audio_start_time: index * 2, audio_end_time: index * 2 + 1, speaker: 'Me',
}));
function resolveRows(requests, rows = longRows) {
  requests.find(request => request.command === 'api_get_meeting_metadata').resolve({ id: requests[0].args.meetingId, title: 'Synthetic meeting' });
  for (const request of requests.filter(request => request.command === 'api_get_meeting_transcripts')) {
    request.resolve({ transcripts: rows.slice(request.args.offset, request.args.offset + request.args.limit), total_count: rows.length, has_more: request.args.offset + request.args.limit < rows.length });
  }
}

test('the first list page has 100 rows while the timeline spans all 250', async () => {
  const view = createView();
  view.render();
  resolveRows(view.requests);
  await flush();
  const current = view.render();
  assert.equal(current.transcripts.length, 100);
  assert.equal(current.segments.length, 100);
  assert.equal(current.timelineSegments.length, 250);
  assert.equal(current.timelineSegments.at(-1).endTime, 499);
  assert.equal(current.totalCount, 250);
  assert.equal(current.hasMore, true);
  const loading = current.loadMore();
  assert.equal(view.requests.at(-1).args.offset, 100);
  resolveRows(view.requests);
  await loading;
  assert.equal(view.render().transcripts.length, 200);
  assert.equal(view.render().timelineSegments.length, 250);
  view.unmount();
});

test('matching speaker edits optimistically update the list and every timeline row', async () => {
  const view = createView();
  view.render();
  resolveRows(view.requests);
  await flush();
  const saving = view.render().applySpeakerToMatching(' Me ', ' Alex  Example ');
  const current = view.render();
  assert.ok(current.transcripts.every(row => row.speaker === 'Alex Example'));
  assert.ok(current.timelineSegments.every(row => row.speaker === 'Alex Example'));
  view.requests.at(-1).resolve({ updated: 250 });
  assert.equal(await saving, 250);
  view.unmount();
});

test('a single speaker edit updates both views and failed edits refetch both', async () => {
  const view = createView();
  view.render();
  resolveRows(view.requests);
  await flush();
  const saving = view.render().updateSpeaker('row-0', 'Alex');
  const rejected = assert.rejects(saving, /Synthetic speaker failure/);
  assert.equal(view.render().transcripts[0].speaker, 'Alex');
  assert.equal(view.render().timelineSegments[0].speaker, 'Alex');
  assert.equal(view.render().timelineSegments[1].speaker, 'Me');
  view.requests.at(-1).reject(new Error('Synthetic speaker failure'));
  await flush();
  resolveRows(view.requests.slice(-3));
  await rejected;
  assert.equal(view.render().transcripts[0].speaker, 'Me');
  assert.equal(view.render().timelineSegments[0].speaker, 'Me');
  view.unmount();
});

test('a late timeline from an earlier refetch cannot replace the refreshed rows', async () => {
  const view = createView();
  view.render();
  const oldTimeline = view.requests.find(request => request.args.limit === 1000);
  const refresh = view.render().refetch();
  resolveInitial(view.requests.slice(-3), 'Corrected');
  await refresh;
  oldTimeline.resolve(page([row('stale')]));
  await flush();
  assert.equal(view.render().timelineSegments[0].text, 'Corrected');
  view.unmount();
});

test('a failed full timeline read reports the failure while retaining the paged transcript', async () => {
  const view = createView();
  view.render();
  view.requests.find(request => request.command === 'api_get_meeting_metadata').resolve({ id: 'first' });
  view.requests.find(request => request.args.limit === 100).resolve(page([row('loaded')]));
  view.requests.find(request => request.args.limit === 1000).reject(new Error('Synthetic timeline failure'));
  await flush();
  const current = view.render();
  assert.equal(current.transcripts.length, 1);
  assert.equal(current.timelineSegments.length, 0);
  assert.equal(current.error, null);
  assert.equal(current.isLoading, false);
  assert.deepEqual(view.errors, ['Failed to load speaker timeline. Reopen the meeting to retry.']);
  view.unmount();
});
