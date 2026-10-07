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
