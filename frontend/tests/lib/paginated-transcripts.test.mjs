import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView() {
  const harness = createHookHarness();
  const requests = [];
  const { usePaginatedTranscripts } = loadTsModule(fileURLToPath(new URL('../../src/hooks/usePaginatedTranscripts.ts', import.meta.url)), {
    react: harness.react,
    '@tauri-apps/api/core': { invoke(command, args) {
      const result = deferred();
      requests.push({ command, args, ...result });
      return result.promise;
    } },
  });
  let meetingId = 'first';
  const render = () => harness.render(() => usePaginatedTranscripts({ meetingId }));
  return { ...harness, requests, render, navigate(id) { meetingId = id; return render(); } };
}

const row = (id, text = id) => ({ id, text, audio_start_time: 1 });
const page = (transcripts, has_more = false) => ({ transcripts, has_more, total_count: 300 });
function resolveInitial(requests, title = 'Meeting', text = title) {
  requests.find(request => request.command === 'api_get_meeting_metadata').resolve({ id: requests[0].args.meetingId, title });
  requests.find(request => request.command === 'api_get_meeting_transcripts').resolve(page([row(title, text)], true));
}

test('late metadata and transcript pages cannot replace a newly opened meeting', async () => {
  const view = createView();
  view.render();
  const oldRequests = view.requests.slice();
  view.navigate('second');
  resolveInitial(view.requests.slice(2), 'Second');
  await flush();
  resolveInitial(oldRequests, 'First');
  await flush();
  const current = view.render();
  assert.equal(current.metadata.id, 'second');
  assert.equal(current.transcripts[0].text, 'Second');
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
  resolveInitial(view.requests.slice(-2), 'Corrected');
  await refresh;
  oldSource.resolve(page([row('far-away', 'Outdated text')]));
  await source;
  assert.equal(view.render().transcripts.length, 1);
  assert.equal(view.render().transcripts[0].text, 'Corrected');
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
  assert.equal(view.requests.length, 4);
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
  view.navigate('first');
  assert.equal(view.requests.length, 4);
  resolveInitial(view.requests.slice(-2));
  await flush();
  assert.equal(view.render().loadedCount, 1);
  view.unmount();
});
