import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('sidebar match scope survives navigation once and carries no search text or source contents', () => {
  const { openMatchScope, receiveMatchScope } = loadTsModule('src/lib/archive-search.ts');
  const submitted = [], project = { tags: ['Review'], mode: 'all', untagged: false };
  openMatchScope({ ids: ['b', 'b'], project, from: '2026-01-01', to: '', query: 'private words', saved: ['personal note'] });
  project.tags.pop();
  const stop = receiveMatchScope(scope => submitted.push(scope));
  openMatchScope({ ids: [], project, from: '', to: '' });
  stop();
  receiveMatchScope(scope => submitted.push(scope))();
  assert.deepEqual(JSON.parse(JSON.stringify(submitted)), [
    { ids: ['b'], project: { tags: ['Review'], mode: 'all', untagged: false }, from: '2026-01-01', to: '' },
  ]);
});

test('only a current source in the intended meeting can open after verification', async () => {
  const navigation = loadTsModule('src/lib/archive-search.ts');
  const reference = { meeting_id: 'fixture', source: 'bookmark', id: 'marker', start: 0, end: 6, fingerprint: 'abc' };
  const url = new URL(navigation.savedPassageUrl(reference, true), 'https://example.com');
  const token = url.searchParams.get('savedSource');
  assert.deepEqual([...url.searchParams.keys()], ['id', 'savedSource', 'from']);
  assert.equal(navigation.savedPassage(token, 'other'), null);
  const hooks = createHookHarness(), markerChanges = [], reads = [], subscriptions = new Map();
  const first = deferred(), second = deferred(), replies = [first, second];
  const jsx = (type, props) => ({ type, props });
  const { SavedSearchRoute } = loadTsModule('src/components/MeetingDetails/SavedSearchRoute.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/lib/archive-search': navigation, '@/components/ui/button': { Button: 'button' },
    '@tauri-apps/api/core': { invoke: async (name, args) => {
      assert.equal(name, 'resolve_saved_search_hit');
      assert.equal(args.reference, reference);
      reads.push(name);
      return replies.shift().promise;
    } },
    '@tauri-apps/api/event': { listen: async (event, notify) => {
      subscriptions.set(event, notify); return () => subscriptions.delete(event);
    } },
  });
  const onBookmark = id => markerChanges.push(id);
  const render = () => hooks.render(() => SavedSearchRoute({ token, meetingId: 'fixture', onBookmark }));
  render(); await flush();
  subscriptions.get('library-changed')();
  second.resolve(null); await flush();
  first.resolve({ reference, title: 'Example', text: 'Old private text', seconds: 2 }); await flush();
  const tree = JSON.stringify(render());
  assert.equal(reads.length, 2);
  assert.match(tree, /changed or was removed/);
  assert.doesNotMatch(tree, /Old private text/);
  assert.ok(markerChanges.every(id => id === null));
  hooks.unmount();
});
