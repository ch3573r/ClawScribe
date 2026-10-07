import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,deferred,flush} from './hook-harness.mjs';
const meeting=id=>({kind:'meeting',meeting_id:id});
function view(service,{scope=meeting('one'),owner={kind:'meeting',id:'one'}}={}) {
  let configuration;
  const hooks=createHookHarness();const {useKnowledgeSearch}=loadTsModule('src/hooks/useKnowledgeSearch.ts',{react:hooks.react,'@tauri-apps/api/event':{listen:async()=>()=>{}},'@/services/knowledgeService':{knowledgeService:{conversations:async()=>[],cancel:async()=>{},history:async()=>[],...service}}});
  return {...hooks,render:()=>hooks.render(()=>useKnowledgeSearch(scope,owner,configuration)),navigate(id){scope=meeting(id);owner={kind:'meeting',id};},configureScope(next){scope=next;},configureAnswer(next){configuration=next;}};
}
test('hook discards older searches and carries actual selected scope and retrieval mode',async()=>{
  const reads=[];const app=view({search:request=>{const read=deferred();reads.push({request,...read});return read.promise;}});
  app.render();await flush();const first=app.render().search('old','hybrid');const second=app.render().search('new','hybrid');
  reads[1].resolve({passages:[{text:'new evidence'}],mode:'keyword',index_status:{reason:'Model unavailable'}});await second;
  reads[0].resolve({passages:[{text:'old evidence'}],mode:'hybrid'});await first;
  assert.equal(app.render().response.passages[0].text,'new evidence');assert.equal(app.render().response.mode,'keyword');assert.equal(reads[1].request.scope.meeting_id,'one');assert.equal(reads[1].request.document_ids.length,0);app.unmount();
});
test('hook holds one request UUID through history reconciliation and preserves legacy history',async()=>{
  const answer=deferred(),history=deferred();const requests=[];let reads=0;
  const legacy={id:'legacy',role:'assistant',content:'Earlier saved meeting chat',status:'completed',legacy:true,reply:null};
  const assistant={id:'saved',role:'assistant',content:'Canonical answer',status:'completed',legacy:false,reply:null};
  const app=view({history:()=>++reads===1?Promise.resolve([legacy]):history.promise,ask:request=>{requests.push(request);return answer.promise;}});
  app.render();await flush();assert.equal(app.render().messages[0].legacy,true);
  const first=app.render().ask('question','keyword');const second=app.render().ask('question','keyword');assert.equal(requests.length,1);
  answer.resolve({request_id:requests[0].request_id});await flush();
  const third=app.render().ask('question','keyword');assert.equal(requests.length,1,'history is part of the pending turn');
  history.resolve([legacy,{id:'user',role:'user',content:'question',reply:null},assistant]);await Promise.all([first,second,third]);
  assert.equal(app.render().messages.filter(row=>row.id==='saved').length,1);assert.equal(app.render().messages[0].id,'legacy');app.unmount();
});
test('navigation invalidates reply before any old-owner history read and hides old history immediately',async()=>{
  const answer=deferred();const historyOwners=[];const app=view({history:async owner=>{historyOwners.push(owner.id);return [{id:owner.id,role:'assistant',content:owner.id}];},ask:()=>answer.promise});
  app.render();await flush();const first=app.render().ask('question','keyword');app.navigate('two');assert.equal(app.render().messages.length,0);await flush();answer.resolve({});await first;
  assert.deepEqual(historyOwners,['one','two']);assert.equal(app.render().messages[0].id,'two');app.unmount();
});
test('empty selected meetings never broaden through a project filter and cancellation stops context resolution',async()=>{
  let searches=0;const origin=deferred();const refs=[];const scope={kind:'library',filter:{all_meetings:false,meeting_ids:[],tags:['Öffnung'],tag_mode:'any',untagged:false,from:null,to:null}};
  const app=view({search:async()=>{searches++;},resolve:ref=>{refs.push(ref);return origin.promise;}},{scope,owner:null});app.render();await flush();await app.render().search('query','keyword');assert.equal(searches,0);assert.match(app.render().error,/Select one/);
  const inspect=app.render().inspect({source_id:'target'},{title:'Public fixture'},{source_id:'origin'});app.render().cancel();origin.resolve({status:'current'});await inspect;assert.equal(refs.length,1);assert.equal(app.render().preview,null);app.unmount();
});

const libraryScope={kind:'library',filter:{all_meetings:false,meeting_ids:['one'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}};
test('first library question creates one durable owner and reconciles one answer despite repeated submission',async()=>{
  const creation=deferred(),answer=deferred();let creates=0;const requests=[];
  const owner={kind:'library',id:'saved-first'};
  const history=[{id:'user',role:'user',content:'Decision?'},{id:'assistant',role:'assistant',content:'Saved answer'}];
  const app=view({createConversation:()=>{creates++;return creation.promise;},ask:request=>{requests.push(request);return answer.promise;},history:async()=>history},{scope:libraryScope,owner:null});
  app.render();await flush();const first=app.render().ask('Decision?','hybrid');const duplicate=app.render().ask('Decision?','hybrid');
  assert.equal(creates,1);creation.resolve(owner);await flush();app.render();await flush();
  assert.equal(requests.length,1);assert.equal(requests[0].owner.id,'saved-first');assert.deepEqual([...requests[0].search.scope.filter.meeting_ids],['one']);
  answer.resolve({});await Promise.all([first,duplicate]);app.render();await flush();
  assert.equal(app.render().owner.id,'saved-first');assert.equal(app.render().messages.filter(row=>row.role==='assistant').length,1);app.unmount();
});
test('cancelled first library question never submits after late conversation creation',async()=>{
  const creation=deferred();let asks=0;
  const app=view({createConversation:()=>creation.promise,ask:async()=>{asks++;}},{scope:libraryScope,owner:null});
  app.render();await flush();const first=app.render().ask('Decision?','keyword');app.render().cancel();creation.resolve({kind:'library',id:'late'});await first;
  assert.equal(asks,0);assert.equal(app.render().owner,null);app.unmount();
});
test('changing scope during first conversation creation never submits or adopts its late owner',async()=>{
  const creation=deferred();let asks=0;
  const app=view({createConversation:()=>creation.promise,ask:async()=>{asks++;}},{scope:libraryScope,owner:null});
  app.render();await flush();const first=app.render().ask('Decision?','keyword');app.navigate('two');app.render();await flush();
  creation.resolve({kind:'library',id:'late-library'});await first;assert.equal(asks,0);assert.equal(app.render().owner.id,'two');app.unmount();
});
test('empty library scope never creates a conversation on first question',async()=>{
  let creates=0;const app=view({createConversation:async()=>{creates++;return {kind:'library',id:'wrong'};}},{scope:{...libraryScope,filter:{...libraryScope.filter,meeting_ids:[]}},owner:null});
  app.render();await flush();await app.render().ask('Decision?','keyword');assert.equal(creates,0);assert.match(app.render().error,/Select one/);app.unmount();
});
test('failed first answer retains the new durable conversation and actionable error for retry',async()=>{
  let creates=0,asks=0;const requests=[];
  const app=view({createConversation:async()=>{creates++;return {kind:'library',id:'saved-failure'};},ask:async request=>{requests.push(request);if(++asks===1)throw 'Provider unavailable; retry';},history:async()=>[{id:'failed',role:'assistant',content:'Provider unavailable',status:'failed'}]},{scope:libraryScope,owner:null});
  app.render();await flush();await app.render().ask('Decision?','keyword');app.render();await flush();
  assert.equal(app.render().owner.id,'saved-failure');assert.match(app.render().error,/Provider unavailable/);assert.equal(app.render().messages[0].status,'failed');
  await app.render().ask('Decision?','keyword');assert.equal(creates,1);assert.equal(requests.length,2);assert.equal(requests[1].owner.id,'saved-failure');app.unmount();
});
test('first answer history failure retries the original request after owner adoption without duplicating its saved answer',async()=>{
  const requests=[];const saved=new Map();let historyReads=0;
  const app=view({createConversation:async()=>({kind:'library',id:'saved-retry'}),ask:async request=>{requests.push(request);saved.set(request.request_id,{id:request.request_id,role:'assistant',content:'Canonical answer',status:'completed'});},history:async()=>{if(++historyReads===1)throw 'History temporarily unavailable';return [...saved.values()];}},{scope:libraryScope,owner:null});
  app.render();await flush();await app.render().ask('Decision?','keyword');app.render();await flush();
  assert.equal(app.render().owner.id,'saved-retry');assert.match(app.render().error,/History temporarily unavailable/);
  await app.render().ask('Decision?','keyword');assert.equal(requests.length,2);assert.equal(requests[1].request_id,requests[0].request_id);assert.equal(saved.size,1);assert.equal(app.render().messages.length,1);app.unmount();
});
for(const invalidation of ['cancel','scope','provider','thread','question']) {
  test(`automatic owner adoption never restores a retry UUID after ${invalidation} changes`,async()=>{
    const requests=[];let reads=0;
    const app=view({createConversation:async()=>({kind:'library',id:'original-thread'}),ask:async request=>{requests.push(request);},history:async()=>{if(++reads===1)throw 'History unavailable';return [];}},{scope:libraryScope,owner:null});
    app.render();await flush();await app.render().ask('Decision?','keyword');app.render();await flush();
    if(invalidation==='cancel')app.render().cancel();
    if(invalidation==='scope')app.configureScope({...libraryScope,filter:{...libraryScope.filter,meeting_ids:['two']}});
    if(invalidation==='provider')app.configureAnswer({provider:'other',model:'other-model'});
    if(invalidation==='thread')app.render().selectConversation({kind:'library',id:'other-thread'});
    app.render();await flush();await app.render().ask(invalidation==='question'?'Another question?':'Decision?','keyword');
    assert.equal(requests.length,2);assert.notEqual(requests[1].request_id,requests[0].request_id);app.unmount();
  });
}
