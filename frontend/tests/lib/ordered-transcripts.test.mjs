import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
const { OrderedTranscripts,compareTranscripts }=loadTsModule(fileURLToPath(new URL('../../src/lib/orderedTranscripts.ts',import.meta.url)));
const item=(id,time=id,text=`Item ${id}`,partial=false)=>({id:`row-${id}`,sequence_id:id,chunk_start_time:time,text,timestamp:'test',is_partial:partial});
const plain=value=>JSON.parse(JSON.stringify(value));

test('ordered appends preserve old snapshots and deduplicate',()=>{
  const store=new OrderedTranscripts();const first=store.merge([item(0)]);const second=store.merge([item(1),item(2)]);
  assert.equal(first.length,1);assert.equal(second.length,3);assert.equal(store.merge([item(1)]),second);
});
test('late arrivals are ordered by time then sequence',()=>{
  const store=new OrderedTranscripts();store.merge([item(2,2),item(3,2)]);store.merge([item(1,1),item(0,2)]);
  assert.deepEqual(plain(store.getSnapshot().map(x=>x.sequence_id)),[1,0,2,3]);
});
test('final recognition replaces a partial without changing row identity',()=>{
  const store=new OrderedTranscripts();store.merge([item(0,0,'Partial',true)]);const first=store.getSnapshot();
  store.merge([{...item(0,0,'Final'),id:'different-id',word_timestamps:[{text:'Final',start:0,end:1}]}]);
  assert.equal(first[0].text,'Partial');assert.equal(store.getSnapshot()[0].text,'Final');assert.equal(store.getSnapshot()[0].id,first[0].id);
  store.merge([item(0,0,'Late partial',true)]);assert.equal(store.getSnapshot()[0].text,'Final');
});
test('multiple updates for one ID in a batch produce one latest row',()=>{
  const store=new OrderedTranscripts();store.merge([item(0,0,'A',true),item(0,0,'B'),item(0,0,'C',true)]);
  assert.equal(store.getSnapshot().length,1);assert.equal(store.getSnapshot()[0].text,'B');
});
test('history reload cannot overwrite live changes',()=>{
  const store=new OrderedTranscripts();store.merge([item(1,1,'Live')]);store.merge([item(0),item(1,1,'Stale')],true);
  assert.equal(store.getSnapshot()[1].text,'Live');assert.equal(store.getSnapshot().length,2);
});
test('reset clears index and permits reused sequence numbers',()=>{
  const store=new OrderedTranscripts();store.merge([item(0)]);store.clear();store.merge([item(0,0,'Next meeting')]);
  assert.equal(store.getSnapshot().length,1);assert.equal(store.getSnapshot()[0].text,'Next meeting');
});
test('missing sequence IDs use distinct row IDs rather than colliding with zero',()=>{
  const store=new OrderedTranscripts();store.merge([item(0),{...item(1),sequence_id:undefined},{...item(2),sequence_id:undefined}]);
  assert.equal(store.getSnapshot().length,3);
});
test('500 deterministic mixed batches match a straightforward independent oracle',()=>{
  let seed=127;const random=()=>((seed=(seed*1664525+1013904223)>>>0)/2**32);
  const store=new OrderedTranscripts(), reference=new Map();
  for(let step=0;step<500;step++){
    const batch=Array.from({length:1+Math.floor(random()*10)},()=>item(Math.floor(random()*100),Math.floor(random()*100),`Text ${step}`,random()<.4));
    for(const next of batch){const old=reference.get(next.sequence_id);if(old&&!old.is_partial&&next.is_partial)continue;reference.set(next.sequence_id,old?{...next,id:old.id}:next);}
    assert.deepEqual(plain(store.merge(batch)),[...reference.values()].sort(compareTranscripts));
  }
});
