import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import { loadTsModule } from './load-ts-module.mjs';
function helpers() { const m = fs.existsSync('src/lib/knowledge-state.ts') ? loadTsModule('src/lib/knowledge-state.ts') : {}; assert.equal(typeof m.libraryScope, 'function', 'meeting scope helpers must exist'); return m; }
test('scope_and_project_filter_stay_synchronized', () => {
  const {libraryScope} = helpers(); const filter = {tags:['Öffnung'],untagged:false,mode:'all'};
  const result = libraryScope(['a'], false, filter, '2026-10-01','2026-10-07');
  assert.deepEqual(JSON.parse(JSON.stringify(result)), {kind:'library',filter:{all_meetings:false,meeting_ids:['a'],tags:['Öffnung'],tag_mode:'all',untagged:false,from:'2026-10-01',to:'2026-10-07'}});
  assert.equal(libraryScope([],false,filter).filter.all_meetings,false);
  assert.equal(libraryScope([],true,filter).filter.all_meetings,true);
});
test('exact grouped and ranged citation grammar never salvages malformed tags', () => {
  const {citationParts} = helpers(); const linked = s => citationParts(s,4,[1,2,3,4]).flatMap(p=>p.tags ?? []);
  assert.deepEqual([...linked('[K3, K1] [ K2 - K4 ]')],[3,1,2,3,4]);
  for (const text of ['[K01]','[K1,K9]','[K1-K2,K3]','[K2-K1]','[[K1][K2]]','[K1 [K2]]','[K1000]']) assert.equal(linked(text).length,0,text);
  assert.equal(citationParts('[K1]',4,[2]).flatMap(p=>p.tags??[]).length,0,'backend literal allowlist is required');
});
test('question context rejects invalid map metadata and remains separate from literal tags', () => {
  const {contextLinks} = helpers();
  const ref = id => ({source_id:'meeting:a',source_revision:1,historical:false,locator:{kind:'transcript',meeting_id:'a',transcript_ids:[id],spans:[{transcript_id:id,start_byte:0,end_byte:5}],start_seconds:null}});
  const reply = {evidence:[ref('question'),ref('reply')],evidence_metadata:[{}, {preceding_question_tag:1}],cited_tags:[2],context_links:[{kind:'preceding_question',cited_tag:2,context_tag:1}]};
  assert.equal(contextLinks(reply).length,1); assert.deepEqual(reply.cited_tags,[2]);
  for (const patch of [{context_tag:3},{context_tag:2},{cited_tag:1}]) assert.equal(contextLinks({...reply,context_links:[{...reply.context_links[0],...patch}]}).length,0);
  assert.equal(contextLinks({...reply,evidence_metadata:[{preceding_question_tag:2},{preceding_question_tag:1}]}).length,0);
});
