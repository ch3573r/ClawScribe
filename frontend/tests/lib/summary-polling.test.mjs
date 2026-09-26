import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createHookHarness, deferred } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(t, invoke) {
  const timers = new Map();
  let nextTimer = 0;
  t.mock.method(globalThis, 'setInterval', callback => {
    const id = ++nextTimer;
    timers.set(id, callback);
    return id;
  });
  t.mock.method(globalThis, 'clearInterval', id => timers.delete(id));
  const harness = createHookHarness();
  const { useSummaryPolling } = loadTsModule(fileURLToPath(new URL('../../src/hooks/useSummaryPolling.ts', import.meta.url)), {
    react: harness.react,
    '@tauri-apps/api/core': { invoke },
  });
  const render = () => harness.render(useSummaryPolling);
  t.after(() => harness.unmount());
  return { render, timers, harness };
}

test('starting and finishing another meeting does not clear the first poll', async t => {
  const { render, timers } = setup(t, async (_command, { meetingId }) => ({ status: meetingId === 'b' ? 'completed' : 'pending', data: {} }));
  const first = [], second = [];
  const initial = render();
  initial.startSummaryPolling('a', 'a', result => first.push(result));
  render().startSummaryPolling('b', 'b', result => second.push(result));
  render();
  assert.equal(timers.size, 2);
  await timers.get(2)();
  render();
  assert.equal(timers.size, 1);
  await timers.get(1)();
  assert.equal(first[0].status, 'pending');
  assert.equal(second[0].status, 'completed');
  // Even a callback retained from an earlier render stops the current poll.
  initial.stopSummaryPolling('a');
  assert.equal(timers.size, 0);
});

test('missing jobs and completed jobs without data terminate with visible errors', async t => {
  let status = 'idle';
  const { render, timers } = setup(t, async () => ({ status }));
  const updates = [];
  render().startSummaryPolling('a', 'a', result => updates.push(result));
  await timers.get(1)();
  assert.equal(updates[0].status, 'idle');
  await timers.get(1)();
  assert.equal(updates[1].status, 'error');
  assert.match(updates[1].error, /no longer active/);
  assert.equal(timers.size, 0);
  status = 'completed';
  render().startSummaryPolling('a', 'a', result => updates.push(result));
  await timers.get(2)();
  assert.equal(updates[2].status, 'error');
  assert.equal(timers.size, 0);
});

test('overlapping ticks coalesce and stopped requests cannot overwrite replacements', async t => {
  const pending = deferred();
  let calls = 0;
  const { render, timers } = setup(t, () => { calls++; return pending.promise; });
  const updates = [];
  const hook = render();
  hook.startSummaryPolling('a', 'a', result => updates.push(result));
  const tick = timers.get(1);
  const request = tick();
  await tick();
  assert.equal(calls, 1);
  hook.startSummaryPolling('a', 'a', result => updates.push(result));
  pending.resolve({ status: 'completed', data: {} });
  await request;
  assert.equal(updates.length, 0);
  assert.equal(timers.size, 1);
  assert.ok(timers.has(2));
});

test('a hung status request reaches the polling limit and ignores its late result', async t => {
  const pending = deferred();
  const { render, timers } = setup(t, () => pending.promise);
  const updates = [];
  render().startSummaryPolling('a', 'a', result => updates.push(result));
  const tick = timers.get(1);
  const request = tick();
  for (let i = 1; i < 200; i++) await tick();
  assert.equal(timers.size, 0);
  assert.equal(updates.length, 1);
  assert.match(updates[0].error, /16 minutes 40 seconds/);
  pending.resolve({ status: 'completed', data: {} });
  await request;
  assert.equal(updates.length, 1);
});

test('unmount removes every timer and ignores requests still arriving', async t => {
  const pending = deferred();
  const { render, timers, harness } = setup(t, () => pending.promise);
  const updates = [];
  render().startSummaryPolling('a', 'a', result => updates.push(result));
  const request = timers.get(1)();
  render().startSummaryPolling('b', 'b', result => updates.push(result));
  harness.unmount();
  assert.equal(timers.size, 0);
  pending.resolve({ status: 'completed', data: {} });
  await request;
  assert.equal(updates.length, 0);
});

test('Tauri string failures are reported and stop polling', async t => {
  const { render, timers } = setup(t, async () => { throw 'Synthetic database failure'; });
  const updates = [];
  render().startSummaryPolling('a', 'a', result => updates.push(result));
  await timers.get(1)();
  assert.equal(updates[0].error, 'Synthetic database failure');
  assert.equal(timers.size, 0);
});
