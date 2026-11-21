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

test('the Ask handoff keeps a local question draft and its selected meetings', () => {
  const { openMatchScope, receiveMatchScope } = loadTsModule('src/lib/archive-search.ts');
  const submitted = [];
  openMatchScope({ ids: ['b'], project: { tags: [], mode: 'any', untagged: false }, from: '', to: '', question: 'Äpfel im März' });
  receiveMatchScope(scope => submitted.push(scope))();
  assert.equal(submitted[0].question, 'Äpfel im März');
  assert.deepEqual([...submitted[0].ids], ['b']);
});

test('search highlights translate UTF-8 offsets without splitting umlauts or emoji', () => {
  const { searchHighlightParts } = loadTsModule('src/lib/archive-search.ts');
  const parts = searchHighlightParts('📘 Äpfel auf der Straße', [[5, 11], [20, 27]]);
  assert.deepEqual(JSON.parse(JSON.stringify(parts)), [
    { text: '📘 ', match: false }, { text: 'Äpfel', match: true },
    { text: ' auf der ', match: false }, { text: 'Straße', match: true },
  ]);
});

function timedEvidenceRoute(t, { onReveal, resolve } = {}) {
  const navigation = loadTsModule('src/lib/knowledge-navigation.ts');
  const reference = { locator: { kind: 'transcript', meeting_id: 'fixture' }, chunk_id: 'late-match' };
  const token = new URL(navigation.transcriptPassageUrl(reference, true, true), 'https://example.com').searchParams.get('evidence');
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const actions = [];
  const verified = { status: 'current', passage: {},
    navigation: { meeting_id: 'fixture', transcript_id: 'late', transcript_index: 221, start_seconds: 724 },
  };
  const jsx = (type, props) => ({ type, props });
  const { EvidenceRoute } = loadTsModule('src/components/Knowledge/EvidenceRoute.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/lib/knowledge-navigation': navigation, '@/components/ui/button': { Button: 'button' },
    '@/services/knowledgeService': { knowledgeService: { resolve: async () => resolve ? resolve(verified) : verified } },
  });
  const reveal = async (id, index, current) => {
    actions.push(['reveal', id, index]);
    if (onReveal) {
      await onReveal(id);
      if (current()) actions.push(['focus', id]);
    }
  };
  const onPlay = async seconds => actions.push(['play', seconds]);
  const render = (audioReady, routeToken = token) => hooks.render(() => EvidenceRoute({ token: routeToken, meetingId: 'fixture', audioReady, audioStatus: 'found', audioError: null, onReveal: reveal, onPlay }));
  return { actions, render };
}

test('a timed search result reveals its text before the player is ready and plays the verified offset', async t => {
  const { actions, render } = timedEvidenceRoute(t);
  render(false);
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221]]);
  render(true);
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221], ['play', 724]]);
});

test('audio readiness rechecks the source and awaits the same pending reveal before playback', async t => {
  const pending = deferred();
  let reads = 0;
  const { actions, render } = timedEvidenceRoute(t, {
    onReveal: () => pending.promise,
    resolve: verified => { reads++; return verified; },
  });
  render(false);
  await flush();
  render(true);
  await flush();
  assert.equal(reads, 2);
  assert.deepEqual(actions, [['reveal', 'late', 221]]);

  pending.resolve();
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221], ['focus', 'late'], ['play', 724]]);
});

test('audio readiness reveals a newly verified position and cancels the older pending reveal', async t => {
  const pending = deferred();
  let reads = 0;
  const { actions, render } = timedEvidenceRoute(t, {
    onReveal: id => id === 'late' ? pending.promise : undefined,
    resolve: verified => ++reads === 1 ? verified : {
      ...verified, navigation: { ...verified.navigation, transcript_id: 'updated', transcript_index: 222, start_seconds: 725 },
    },
  });
  render(false);
  await flush();
  render(true);
  await flush();
  pending.resolve();
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221], ['reveal', 'updated', 222], ['focus', 'updated'], ['play', 725]]);
});

test('audio readiness blocks pending focus and playback when source verification fails', async t => {
  const pending = deferred();
  let reads = 0;
  const { actions, render } = timedEvidenceRoute(t, {
    onReveal: () => pending.promise,
    resolve: verified => ++reads === 1 ? verified : { status: 'stale' },
  });
  render(false);
  await flush();
  render(true);
  await flush();
  pending.resolve();
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221]]);
  assert.match(JSON.stringify(render(true)), /Retry opening passage/);
});

test('leaving a pending route cancels focus and playback after audio becomes ready', async t => {
  const pending = deferred();
  const { actions, render } = timedEvidenceRoute(t, { onReveal: () => pending.promise });
  render(false);
  await flush();
  render(true);
  await flush();
  render(true, null);
  pending.resolve();
  await flush();
  assert.deepEqual(actions, [['reveal', 'late', 221]]);
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

test('personal excerpts accept known endpoints and legacy aliases case-insensitively', () => {
  const { supportsPersonalExcerpts } = loadTsModule('src/lib/archive-search.ts');
  for (const provider of ['OpenAI', 'CLAUDE', 'Groq', 'Ollama', 'OpenRouter', 'BuiltIn-AI',
    'Local-Llama', 'LocalLlama', 'OpenAI-Compatible', 'API-Key', 'Custom-OpenAI', 'OpenClaw', 'OpenClaw-Managed']) {
    assert.equal(supportsPersonalExcerpts(provider), true, provider);
  }
  for (const provider of ['Claude-Code', 'Codex', 'Codex-Login', 'Codex-ChatGPT', 'unknown', 'openai-other', '', null, undefined]) {
    assert.equal(supportsPersonalExcerpts(provider), false, provider);
  }
});

test('switching to an account-managed provider discards checked personal excerpts', () => {
  const hooks = createHookHarness();
  const jsx = (type, props) => ({ type, props });
  const { useResultNotes } = loadTsModule('src/hooks/useResultNotes.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, '@/components/ui/checkbox': { Checkbox: 'checkbox' },
  });
  const { supportsPersonalExcerpts } = loadTsModule('src/lib/archive-search.ts');
  const note = { reference: { meeting_id: 'fixture', source: 'note', fingerprint: 'current' }, text: 'Local excerpt' };
  const render = provider => hooks.render(() => useResultNotes('search', supportsPersonalExcerpts(provider)));
  render('builtin-ai').control(note).props.children[0].props.onCheckedChange(true);
  assert.equal(render('builtin-ai').references([note]).length, 1);
  assert.equal(render('codex').control(note), null);
  assert.equal(render('claude-code').references([note]).length, 0);
  assert.equal(render('builtin-ai').references([note]).length, 0);
  hooks.unmount();
});
