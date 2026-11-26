import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import { loadTsModule } from './load-ts-module.mjs';
import { deferred } from './hook-harness.mjs';

function controller() {
  const module = fs.existsSync('src/lib/knowledge-state.ts') ? loadTsModule('src/lib/knowledge-state.ts') : {};
  assert.equal(typeof module.createKnowledgeController, 'function', 'knowledge generation owner must exist');
  return module.createKnowledgeController;
}
const scope = id => ({kind:'meeting',meeting_id:id});
test('project and date filters are sufficient scope for a library question', () => {
  const { libraryScope, scopeReady } = loadTsModule('src/lib/knowledge-state.ts');
  const projects = { tags: [], untagged: false, mode: 'any' };
  assert.equal(scopeReady(libraryScope([], false, projects)), false);
  assert.equal(scopeReady(libraryScope([], false, { ...projects, tags: ['Roadmap'] })), true);
  assert.equal(scopeReady(libraryScope([], false, { ...projects, untagged: true })), true);
  assert.equal(scopeReady(libraryScope([], false, projects, '2026-10-04')), true);
  assert.equal(scopeReady(libraryScope([], false, projects, '', '2026-10-04')), true);
  assert.equal(scopeReady(libraryScope([], true, projects, '2026-10-04', '2026-10-03')), false);
});

test('an imported file is visible immediately but is only sent after scope verification', () => {
  const { selectedReferences } = loadTsModule('src/lib/knowledge-state.ts');
  const file = { id: 'one', display_name: 'Reference.md' };
  const normalized = (...args) => JSON.parse(JSON.stringify(selectedReferences(...args)));
  assert.deepEqual(normalized([], [], [file]), { files: [file], ids: [], verifying: true });
  assert.deepEqual(normalized([{ attachment: file }], [], [file]), { files: [file], ids: ['one'], verifying: false });
  assert.deepEqual(normalized([{ attachment: file }], ['one'], [file]), { files: [file], ids: ['one'], verifying: false });
});

test('semantic search defaults to hybrid only when available', () => {
  const { knowledgeSearchMode } = loadTsModule('src/lib/knowledge-state.ts');
  assert.equal(knowledgeSearchMode(null, true), 'hybrid');
  assert.equal(knowledgeSearchMode('keyword', true), 'keyword');
  assert.equal(knowledgeSearchMode('hybrid', false), 'keyword');
});
test('old_query_cannot_replace_new_results', async () => {
  const owner = controller()(); const old = deferred(); const latest = deferred(); let visible;
  const a = owner.run('search', () => old.promise, v => { visible = v; });
  const b = owner.run('search', () => latest.promise, v => { visible = v; });
  latest.resolve('new'); await b; old.resolve('old'); await a;
  assert.equal(visible, 'new');
});
test('meeting_navigation_discards_late_reply', async () => {
  const owner = controller()(); const reply = deferred(); let visible = null;
  owner.configure(scope('one')); const pending = owner.run('ask', () => reply.promise, v => { visible = v; });
  owner.configure(scope('two')); reply.resolve('wrong meeting'); await pending;
  assert.equal(visible, null);
});
test('repeated_submit_reuses_request_id', async () => {
  const owner = controller()(); const reply = deferred(); const ids = []; let replies = 0;
  const send = () => owner.submit('question', id => { ids.push(id); return reply.promise; }, () => { replies++; }, () => 'public-request');
  const first = send(); const second = send();
  assert.equal(ids.length, 1); assert.equal(owner.pendingRequest(), 'public-request');
  reply.resolve('one assistant'); await Promise.all([first, second]);
  assert.equal(replies, 1);
});
test('late history clear resolver and playback operations are discarded', async () => {
  for (const kind of ['history','clear','resolve','navigate']) {
    const owner = controller()(); const work = deferred(); let visible = false;
    const pending = owner.run(kind, () => work.promise, () => { visible = true; });
    owner.invalidate(); work.resolve(true); await pending; assert.equal(visible, false, kind);
  }
});
test('failed submission retries the same identity and cancellation invalidates it', async () => {
  const owner = controller()(); const ids = [];
  await assert.rejects(owner.submit('same', id => { ids.push(id); throw Error('provider unavailable'); }, () => {}, () => 'first'));
  await owner.submit('same', async id => { ids.push(id); return 'saved'; }, () => {}, () => 'second');
  assert.deepEqual(ids, ['first','first']);
  owner.invalidate(); await owner.submit('same', async id => { ids.push(id); }, () => {}, () => 'third');
  assert.equal(ids.at(-1), 'third');
});
