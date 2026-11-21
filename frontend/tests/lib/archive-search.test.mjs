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

test('only individually checked, visible note revisions cross the handoff as locators', () => {
  const hooks = createHookHarness();
  const jsx = (type, props) => ({type, props});
  const { useResultNotes } = loadTsModule('src/hooks/useResultNotes.tsx', {
    react: hooks.react, 'react/jsx-runtime': {jsx,jsxs:jsx}, '@/components/ui/checkbox': {Checkbox:'checkbox'},
  });
  const {openMatchScope, receiveMatchScope} = loadTsModule('src/lib/archive-search.ts');
  const hit = (source, id) => ({reference: {meeting_id:'matched', source, id, start:0,end:7,fingerprint:id},text:'Private fixture excerpt'});
  const note = hit('note','n'), bookmark = hit('bookmark','b'), summary = hit('summary','s');
  const render = (view='archive selection') => hooks.render(() => useResultNotes(view));
  const check = (row, value) => render().control(row).props.children[0].props.onCheckedChange(value);
  try {
    assert.equal(render().control(summary),null);
    assert.equal(render().references([note,bookmark]).length,0);
    check(note,true); check(bookmark,true); check(note,false);
    assert.equal(render().references([note]).length,0);
    assert.equal(render().references([hit('bookmark','revised')]).length,0);
    const refs = render().references([summary,bookmark,note]);
    const scope = {ids:['matched'],project:{tags:[],mode:'any',untagged:false},from:'',to:'',
      notes:[...refs.map(ref=>({...ref,text:bookmark.text})),{...note.reference,meeting_id:'outside'}]};
    openMatchScope(scope);
    let transferred; receiveMatchScope(value=>{transferred=value;})();
    assert.deepEqual(JSON.parse(JSON.stringify(transferred.notes)),[bookmark.reference]);
    assert.doesNotMatch(JSON.stringify(transferred),/Private fixture|outside/);
    assert.equal(render('new query').references([bookmark]).length,0);
  } finally { hooks.unmount(); }
});
