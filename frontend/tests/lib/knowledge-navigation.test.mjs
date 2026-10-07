import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { deferred } from './hook-harness.mjs';
const reference={locator:{kind:'transcript',meeting_id:'target'}};
const resolved={status:'current',navigation:{meeting_id:'target',transcript_id:'far-row',transcript_index:205,start_seconds:14},passage:{}};
function navigation(){const m=loadTsModule('src/lib/knowledge-navigation.ts');assert.equal(typeof m.navigateEvidence,'function','canonical reveal/playback adapter must exist');return m.navigateEvidence;}
test('far-page navigation uses resolver index and checks identity after reveal before playback',async()=>{
  const navigate=navigation();const reveal=deferred();const entered=deferred();let current=true;const calls=[];
  const work=navigate(reference,true,{current:()=>current,resolve:async()=>resolved,reveal:async(id,index)=>{calls.push([id,index]);entered.resolve();await reveal.promise;},play:async()=>calls.push('play')});
  await entered.promise;current=false;reveal.resolve();await work;
  assert.deepEqual(calls,[['far-row',205]]);
});
test('missing stale invalid evidence and unknown offsets never play',async()=>{
  const navigate=navigation();
  for(const status of ['missing','stale','invalid']){const calls=[];await assert.rejects(navigate(reference,true,{current:()=>true,resolve:async()=>({...resolved,status}),reveal:async()=>calls.push('reveal'),play:async()=>calls.push('play')}));assert.deepEqual(calls,[]);}
  const calls=[];await assert.rejects(navigate(reference,true,{current:()=>true,resolve:async()=>({...resolved,navigation:{...resolved.navigation,start_seconds:null}}),reveal:async()=>calls.push('reveal'),play:async()=>calls.push('play')}));assert.equal(calls.includes('play'),false);
});
test('explicit context navigation independently resolves both original references',async()=>{
  const navigate=navigation();const context={locator:{kind:'transcript',meeting_id:'target'},source_id:'question'};const calls=[];
  await navigate(reference,false,{current:()=>true,resolve:async ref=>{calls.push(ref);return resolved;},reveal:async()=>{},play:async()=>{}},context);
  assert.deepEqual(calls,[context,reference]);
});
