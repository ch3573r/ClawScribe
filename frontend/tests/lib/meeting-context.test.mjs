import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { deferred, flush } from './hook-harness.mjs';

function contextModule(invoke, legacy = new Map()) {
  const previous = globalThis.window;
  globalThis.window = { localStorage: {
    get length() { return legacy.size; },
    key: index => [...legacy.keys()][index] ?? null,
    getItem: key => legacy.get(key) ?? null,
    removeItem: key => legacy.delete(key),
  } };
  const module = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingContext.ts', import.meta.url)), {
    '@tauri-apps/api/core': { invoke },
  });
  globalThis.window = previous;
  return module;
}

test('legacy context is retained on failure and removed only after successful migration', async () => {
  const legacy = new Map([['clawscribe.meetingContext.synthetic', 'Legacy context'], ['unrelated', 'Keep']]);
  let fail = true;
  let calls = 0;
  const context = contextModule(async (command, args) => {
    assert.equal(command, 'set_meeting_context');
    assert.equal(args.onlyIfMissing, true);
    assert.equal(args.meetingId, 'synthetic');
    calls++;
    if (fail) throw new Error('Synthetic database error');
  }, legacy);
  await assert.rejects(context.migrateLegacyMeetingContexts(), /database error/);
  assert.equal(legacy.get('clawscribe.meetingContext.synthetic'), 'Legacy context');
  fail = false;
  await context.migrateLegacyMeetingContexts();
  await context.migrateLegacyMeetingContexts();
  assert.equal(calls, 2);
  assert.deepEqual([...legacy], [['unrelated', 'Keep']]);
});

test('context writes stay ordered and reads wait for the latest saved value', async () => {
  const first = deferred();
  const calls = [];
  let saved = '';
  const context = contextModule(async (command, args) => {
    if (command === 'get_meeting_context') return saved;
    calls.push(args.context);
    if (args.context === 'First') await first.promise;
    saved = args.context;
  });
  const a = context.setMeetingContext('synthetic', 'First');
  const b = context.setMeetingContext('synthetic', 'Latest');
  const read = context.getMeetingContext('synthetic');
  await flush();
  assert.deepEqual(calls, ['First']);
  first.resolve();
  await Promise.all([a, b]);
  assert.equal(await read, 'Latest');
  await context.setMeetingContext('synthetic', '');
  assert.equal(await context.getMeetingContext('synthetic'), '');
});
