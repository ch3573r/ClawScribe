import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

// Timers the test controls. The module captures the functions when it loads.
function clock() {
  const timers = new Map();
  let next = 0;
  const previous = { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
  globalThis.setTimeout = (callback, delay) => { timers.set(++next, { callback, delay }); return next; };
  globalThis.clearTimeout = id => timers.delete(id);
  return {
    timers,
    async fire() {
      const due = [...timers.values()];
      timers.clear();
      for (const timer of due) await timer.callback();
    },
    restore() { Object.assign(globalThis, previous); },
  };
}

function setup(t, run) {
  const time = clock();
  t.after(() => time.restore());
  const { createLatestSearch } = loadTsModule('src/lib/latest-search.ts');
  const shown = [];
  const searching = [];
  const errors = [];
  const search = createLatestSearch({
    run,
    onResults: results => shown.push([...results]),
    onSearching: value => searching.push(value),
    onError: error => errors.push(String(error)),
  });
  return { time, search, shown, searching, errors };
}

const deferredRun = () => {
  const calls = [];
  return { calls, run: query => new Promise((resolve, reject) => calls.push({ query, resolve, reject })) };
};

test('typing waits 200 ms of quiet and searches once, for the last query', async t => {
  const backend = deferredRun();
  const app = setup(t, backend.run);
  const pending = ['m', 'me', 'mee', 'meet'].map(query => app.search.search(query));
  assert.equal(app.time.timers.size, 1, 'each keystroke restarts one timer');
  assert.equal([...app.time.timers.values()][0].delay, 200);
  assert.equal(backend.calls.length, 0, 'nothing is requested while typing');
  const firing = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(backend.calls.map(call => call.query), ['meet']);
  backend.calls[0].resolve(['a result']);
  await firing;
  await Promise.all(pending);
  assert.deepEqual(app.shown, [['a result']]);
  assert.equal(app.searching.at(-1), false);
});

test('an answer to an older query that arrives late is ignored', async t => {
  const backend = deferredRun();
  const app = setup(t, backend.run);
  void app.search.search('first');
  const firstRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  void app.search.search('second');
  const secondRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(backend.calls.map(call => call.query), ['first', 'second']);
  // The newer answer comes first, the older one afterwards.
  backend.calls[1].resolve(['second result']);
  await secondRun;
  backend.calls[0].resolve(['first result']);
  await firstRun;
  assert.deepEqual(app.shown, [['second result']], 'the older answer never replaces the newer one');
  assert.equal(app.searching.at(-1), false);
});

test('an older request finishing does not end the searching state of the newer one', async t => {
  const backend = deferredRun();
  const app = setup(t, backend.run);
  void app.search.search('first');
  const firstRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  void app.search.search('second');
  const secondRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  const before = app.searching.length;
  backend.calls[0].resolve(['first result']);
  await firstRun;
  assert.equal(app.searching.length, before, 'the first request left the state alone');
  assert.equal(app.searching.at(-1), true);
  backend.calls[1].resolve([]);
  await secondRun;
  assert.equal(app.searching.at(-1), false);
});

test('an older failure is ignored, a current failure clears the results and is reported', async t => {
  const backend = deferredRun();
  const app = setup(t, backend.run);
  void app.search.search('first');
  const firstRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  void app.search.search('second');
  const secondRun = app.time.fire();
  await new Promise(resolve => setImmediate(resolve));
  backend.calls[0].reject(new Error('old failure'));
  await firstRun;
  assert.deepEqual(app.errors, []);
  backend.calls[1].reject(new Error('current failure'));
  await secondRun;
  assert.deepEqual(app.errors, ['Error: current failure']);
  assert.deepEqual(app.shown, [[]]);
});

test('clearing the box cancels a pending search at once', async t => {
  const backend = deferredRun();
  const app = setup(t, backend.run);
  const pending = app.search.search('abc');
  await app.search.search('  ');
  await pending;
  assert.equal(app.time.timers.size, 0);
  await app.time.fire();
  assert.equal(backend.calls.length, 0);
  assert.deepEqual(app.shown, [[]]);
  assert.equal(app.searching.at(-1), false);
});
