import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
const { MeetingChatSession } = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingChatSession.ts', import.meta.url)));
const message = (id, role = 'assistant', content = 'Answer') => ({ id, meeting_id: 'one', role, content, created_at: '2026-01-01' });
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; };
const plain = value => JSON.parse(JSON.stringify(value));

test('history must finish before sending; no late overwrite', async () => {
  const history = deferred(); let sends=0;
  const session = new MeetingChatSession('one', command => command === 'api_chat_history' ? history.promise : (sends++, Promise.resolve(message('reply'))));
  const loading = session.load(); session.setDraft('Question'); await session.send('model','version');
  assert.equal(sends,0); assert.equal(session.getSnapshot().draft,'Question');
  history.resolve([message('old')]); await loading; await session.send('model','version');
  assert.equal(sends,1); assert.equal(session.getSnapshot().messages.length,3);
});
test('history failure requires retry and keeps the draft', async () => {
  let calls=0;
  const session=new MeetingChatSession('one', async () => { if (++calls===1) throw Error('failure'); return []; });
  session.setDraft('Question'); await session.load();
  assert.equal(session.getSnapshot().loaded,false); assert.match(session.getSnapshot().error,/Retry/);
  await session.load(); assert.equal(session.getSnapshot().loaded,true); assert.equal(session.getSnapshot().draft,'Question');
});
test('duplicate sends and clear while answering cannot race', async () => {
  const reply=deferred(); let sends=0,clears=0;
  const session=new MeetingChatSession('one', async command => {
    if(command==='api_chat_history')return [];
    if(command==='api_chat_clear'){clears++;return 0;}
    sends++;return reply.promise;
  });
  await session.load();session.setDraft('Question');
  const sending=session.send('model','version'); await session.send('model','version');await session.clear();
  assert.equal(sends,1);assert.equal(clears,0);
  reply.resolve(message('reply'));await sending;await session.clear();
  assert.equal(clears,1);assert.equal(session.getSnapshot().messages.length,0);
});
test('failed sends restore question and reuse request identity on retry', async () => {
  const requests=[];
  const session=new MeetingChatSession('one', async (command,args) => {
    if(command==='api_chat_history')return [];
    requests.push(args);
    if(requests.length===1)throw Error('unavailable');
    return message('reply-'+args.requestId);
  });
  await session.load();session.setDraft('Question');await session.send('model','version');
  assert.equal(session.getSnapshot().draft,'Question');assert.equal(session.getSnapshot().messages[0].status,'failed');
  await session.send('model','version');
  assert.equal(requests[0].requestId,requests[1].requestId);
  assert.equal(session.getSnapshot().messages.length,2);assert.equal(session.getSnapshot().error,null);
});
test('editing a failed question creates a distinct request', async () => {
  const requests=[];const session=new MeetingChatSession('one',async(command,args)=>{
    if(command==='api_chat_history')return [];requests.push(args);throw Error('failure');
  });
  await session.load();session.setDraft('Question');await session.send('model','version');
  session.setDraft('Another question');await session.send('model','version');
  assert.notEqual(requests[0].requestId,requests[1].requestId);
});
test('a newer draft is not overwritten by a failed request', async()=>{
  const reply=deferred();const session=new MeetingChatSession('one',command=>command==='api_chat_history'?Promise.resolve([]):reply.promise);
  await session.load();session.setDraft('Question');const sending=session.send('model','version');session.setDraft('Next question');reply.reject(Error('failure'));await sending;
  assert.equal(session.getSnapshot().draft,'Next question');
});
for(const operation of ['history','send'])test(`meeting/unmount invalidation rejects late ${operation}`,async()=>{
  const pending=deferred();const session=new MeetingChatSession('one',command=>command==='api_chat_history'&&operation==='send'?Promise.resolve([]):pending.promise);
  let running;
  if(operation==='send'){await session.load();session.setDraft('Question');running=session.send('model','version');}
  else running=session.load();
  session.invalidate();const before=plain(session.getSnapshot());
  pending.resolve(operation==='send'?message('late'):[message('late')]);await running;
  assert.deepEqual(plain(session.getSnapshot()),before);
});
test('wrong-meeting history is never displayed',async()=>{
  const session=new MeetingChatSession('one',async()=>[{...message('wrong'),meeting_id:'two'}]);await session.load();
  assert.equal(session.getSnapshot().loaded,false);assert.equal(session.getSnapshot().messages.length,0);
});
test('wrong-meeting reply is never displayed',async()=>{
  const session=new MeetingChatSession('one',async command=>command==='api_chat_history'?[]:{...message('wrong'),meeting_id:'two'});
  await session.load();session.setDraft('Question');await session.send('model','version');
  assert.ok(session.getSnapshot().messages.every(m=>m.meeting_id==='one'));assert.equal(session.getSnapshot().draft,'Question');
});
test('failed clear keeps history and produces an actionable error',async()=>{
  const session=new MeetingChatSession('one',async command=>{if(command==='api_chat_history')return [message('old')];throw Error('failure');});
  await session.load();await session.clear();assert.equal(session.getSnapshot().messages.length,1);assert.match(session.getSnapshot().error,/kept/);
});
test('send requires provider and model',async()=>{
  const session=new MeetingChatSession('one',async()=>[]);await session.load();session.setDraft('Question');await session.send();
  assert.equal(session.getSnapshot().messages.length,0);assert.equal(session.getSnapshot().draft,'Question');
});
test('coverage metadata propagates and is reset on clear',async()=>{
  const session=new MeetingChatSession('one',async command=>command==='api_chat_history'?[]:command==='api_chat_clear'?0:{...message('reply'),context_truncated:true});
  await session.load();session.setDraft('Question');await session.send('provider','model');
  assert.equal(session.getSnapshot().contextTruncated,true);await session.clear();assert.equal(session.getSnapshot().contextTruncated,false);
});
test('external-store snapshots stay stable between actual changes',()=>{
  const session=new MeetingChatSession('one',async()=>[]);let notifications=0;const unsubscribe=session.subscribe(()=>notifications++);
  const before=session.getSnapshot();assert.equal(session.getSnapshot(),before);session.setDraft('Question');
  assert.notEqual(session.getSnapshot(),before);assert.equal(before.draft,'');assert.equal(notifications,1);
  unsubscribe();session.setDraft('Changed');assert.equal(notifications,1);
});
