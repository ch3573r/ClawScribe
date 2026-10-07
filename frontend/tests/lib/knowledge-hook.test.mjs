import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,deferred,flush} from './hook-harness.mjs';
const meeting=id=>({kind:'meeting',meeting_id:id});
function view(service,{scope=meeting('one'),owner={kind:'meeting',id:'one'}}={}) {
  const hooks=createHookHarness();const {useKnowledgeSearch}=loadTsModule('src/hooks/useKnowledgeSearch.ts',{react:hooks.react,'@/services/knowledgeService':{knowledgeService:{conversations:async()=>[],cancel:async()=>{},history:async()=>[],...service}}});
  return {...hooks,render:()=>hooks.render(()=>useKnowledgeSearch(scope,owner)),navigate(id){scope=meeting(id);owner={kind:'meeting',id};}};
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
