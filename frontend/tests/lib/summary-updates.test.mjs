import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(invoke) {
  const events = new Map();
  const { createSummaryUpdates } = loadTsModule('src/lib/summary-updates.ts', {
    '@tauri-apps/api/core': { invoke },
    '@tauri-apps/api/event': { listen: async (event, handler) => {
      events.set(event, handler);
      return () => events.delete(event);
    } },
  });
  return {
    updates: createSummaryUpdates('saved-meeting'),
    events,
    change: (meetingId = 'saved-meeting') => events.get('summary-job-changed')?.({ payload: meetingId }),
  };
}

test('the page and generation watch one read per summary change', async () => {
  let result = { status: 'processing' };
  let reads = 0;
  const { updates, events, change } = setup(async (command, args) => {
    assert.equal(command, 'api_get_summary');
    assert.equal(args.meetingId, 'saved-meeting');
    ++reads;
    return result;
  });
  const page = [], generation = [];
  const offPage = updates.subscribe(value => page.push(value));
  await flush();
  const offGeneration = updates.subscribe(value => generation.push(value));
  change('a-different-meeting');
  await flush();
  assert.equal(reads, 1);
  assert.equal(generation[0].status, 'processing');
  result = { status: 'processing', progress: { completed: 1, total: 3, level: 1 } };
  change();
  await flush();
  assert.equal(generation.at(-1).progress.completed, 1);
  result = { status: 'completed', data: { markdown: 'Saved notes' } };
  change();
  await flush();
  assert.equal(reads, 3);
  assert.equal(page.at(-1).data.markdown, 'Saved notes');
  assert.equal(generation.at(-1), page.at(-1));
  offPage();
  offGeneration();
  await flush();
  assert.equal(events.size, 0);
});

test('a change during a read refreshes the snapshot before publishing it', async () => {
  const first = deferred();
  let reads = 0;
  const { updates, change } = setup(() => ++reads === 1 ? first.promise : Promise.resolve({ status: 'processing' }));
  const heard = [];
  const off = updates.subscribe(value => heard.push(value.status));
  await flush();
  change();
  change();
  first.resolve({ status: 'completed', data: { markdown: 'Previous run' } });
  await flush();
  assert.equal(reads, 2);
  assert.deepEqual(heard, ['processing']);
  off();
});

test('retry reads a failed completion through the same listeners', async () => {
  let failed = true;
  const { updates, events } = setup(async () => {
    if (failed) throw new Error('Read failed');
    return { status: 'completed', data: { markdown: 'Recovered result' } };
  });
  const heard = [];
  const off = updates.subscribe(value => heard.push(value.status));
  await flush();
  assert.deepEqual(heard, ['unavailable']);
  const originalListener = events.get('summary-job-changed');
  failed = false;
  updates.refresh();
  await flush();
  assert.deepEqual(heard, ['unavailable', 'completed']);
  assert.equal(events.get('summary-job-changed'), originalListener);
  off();
});

test('watching a newly started run skips the previous completion', async () => {
  let result = { status: 'completed', data: { markdown: 'Previous run' } };
  const { updates } = setup(async () => result);
  const offPage = updates.subscribe(() => {});
  await flush();
  result = { status: 'processing' };
  const heard = [];
  const offRun = updates.subscribe(value => heard.push(value.status), { refresh: true });
  await flush();
  assert.deepEqual(heard, ['processing']);
  offRun();
  offPage();
});
