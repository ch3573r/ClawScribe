import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

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
    empty: [],
    onResults: results => shown.push([...results]),
    onSearching: value => searching.push(value),
    onError: error => errors.push(String(error)),
  });
  return { time, search, shown, searching, errors };
}

test('library search debounces literal Unicode, sends the current scope and skips hidden or oversized queries', async t => {
  const time = clock();
  t.after(() => time.restore());
  const hooks = createHookHarness();
  const calls = [];
  const { useLibrarySearch, MAX_LIBRARY_QUERY } = loadTsModule('src/hooks/useLibrarySearch.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push({command, args});
      return { titles: [{ id: 'fixture' }], passages: [], limited: false };
    } },
  });
  t.after(hooks.unmount);
  const scope = { kind: 'library', filter: { all_meetings: true, meeting_ids: [], tags: ['Öffnung'], tag_mode: 'all', untagged: false, from: '2026-01-01', to: null } };
  const render = (text, active = true) => hooks.render(() => useLibrarySearch(text, active, scope));
  render('Ä');
  render(' Äpfel 50%_ 📘 ');
  assert.equal(time.timers.size, 1);
  await time.fire();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'search_saved_meetings');
  assert.equal(calls[0].args.query, 'Äpfel 50%_ 📘');
  assert.equal(calls[0].args.scope.filter.tags[0], 'Öffnung');
  assert.equal(render(' Äpfel 50%_ 📘 ').results.titles[0].id, 'fixture');
  render('hidden', false);
  assert.match(render('📘'.repeat(MAX_LIBRARY_QUERY + 1)).error, /up to 256/);
  await flush();
  assert.equal(time.timers.size, 0);
  assert.equal(calls.length, 1);
});

test('archive load more preserves other sources and discards pages from an older query', async t => {
  const time = clock();
  const hooks = createHookHarness();
  t.after(time.restore);
  t.after(hooks.unmount);
  const next = [null, '17', null];
  let finishPage;
  const { useLibrarySearch } = loadTsModule('src/hooks/useLibrarySearch.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (_command, args) => {
      if (args.savedCursor) {
        assert.deepEqual(Array.from(args.savedCursor), next);
        return new Promise(resolve => { finishPage = resolve; });
      }
      return {
        titles: [{ id: args.query }], passages: [], saved: [{ text: args.query }],
        limited: false, saved_next: next,
      };
    } },
  });
  const render = text => hooks.render(() => useLibrarySearch(text, true));
  render('first');
  await time.fire();
  const load = render('first').loadMore();
  assert.equal(render('first').loadingMore, true);
  finishPage({ titles: [], passages: [], saved: [{ text: 'older' }], limited: false, saved_next: null });
  await load;
  assert.deepEqual(Array.from(render('first').results.saved, hit => hit.text), ['first', 'older']);
  assert.equal(render('first').results.titles[0].id, 'first');

  render('second');
  await time.fire();
  const superseded = render('second').loadMore();
  render('third');
  await time.fire();
  finishPage({ saved: [{ text: 'stale' }], limited: false, saved_next: null });
  await superseded;
  assert.deepEqual(Array.from(render('third').results.saved, hit => hit.text), ['third']);
  assert.equal(render('third').loadingMore, false);
});

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
