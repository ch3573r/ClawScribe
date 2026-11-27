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

test('manual retry reads a failed completion through the same listeners and clears the delayed retry', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let failed = true;
  let reads = 0;
  const { updates, events } = setup(async () => {
    ++reads;
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
  t.mock.timers.tick(12000);
  await flush();
  assert.equal(reads, 2);
  off();
});

test('a failed final-event read retries on its own and delivers the completed result', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let final = false;
  let attempts = 0;
  const { updates, change } = setup(async () => {
    if (!final) return { status: 'processing' };
    if (++attempts < 3) throw new Error('Read failed');
    return { status: 'completed', data: { markdown: 'Recovered result' } };
  });
  const heard = [];
  const off = updates.subscribe(value => heard.push(value));
  t.after(off);
  await flush();
  final = true;
  change();
  await flush();
  assert.equal(heard.at(-1).status, 'unavailable');
  t.mock.timers.tick(1999);
  assert.equal(attempts, 1);
  t.mock.timers.tick(1);
  await flush();
  assert.equal(attempts, 2);
  t.mock.timers.tick(10000);
  await flush();
  assert.deepEqual(heard.map(value => value.status), ['processing', 'unavailable', 'unavailable', 'completed']);
  assert.equal(heard.at(-1).data.markdown, 'Recovered result');
  t.mock.timers.tick(12000);
  await flush();
  assert.equal(attempts, 3);
});

test('disconnect cancels the delayed retry for a failed read', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let reads = 0;
  const { updates, events } = setup(async () => {
    ++reads;
    throw new Error('Read failed');
  });
  const off = updates.subscribe(() => {});
  await flush();
  off();
  t.mock.timers.tick(12000);
  await flush();
  assert.equal(reads, 1);
  assert.equal(events.size, 0);
});

test('a throwing listener does not stop another subscriber receiving a result', async t => {
  t.mock.method(console, 'error', () => {});
  const { updates } = setup(async () => ({ status: 'completed', data: { markdown: 'Saved notes' } }));
  const offThrowing = updates.subscribe(() => { throw new Error('Listener failed'); });
  const heard = [];
  const offHealthy = updates.subscribe(value => heard.push(value.data.markdown));
  t.after(offThrowing);
  t.after(offHealthy);
  await flush();
  assert.deepEqual(heard, ['Saved notes']);
});

test('a read that resolves after the last unsubscribe cannot reach a new subscriber', async t => {
  const first = deferred();
  let reads = 0;
  const { updates } = setup(() => ++reads === 1 ? first.promise
    : Promise.resolve({ status: 'completed', data: { markdown: 'Current result' } }));
  const disconnected = [];
  const off = updates.subscribe(value => disconnected.push(value));
  await flush();
  off();
  await flush();
  const heard = [];
  const offCurrent = updates.subscribe(value => heard.push(value.data.markdown));
  t.after(offCurrent);
  await flush();
  first.resolve({ status: 'completed', data: { markdown: 'Stale result' } });
  await flush();
  assert.equal(reads, 2);
  assert.deepEqual(disconnected, []);
  assert.deepEqual(heard, ['Current result']);
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
