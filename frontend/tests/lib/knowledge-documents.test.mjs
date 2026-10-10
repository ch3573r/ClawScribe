import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,deferred,flush} from './hook-harness.mjs';

const meeting=id=>({kind:'meeting',meeting_id:id});
const attachment=id=>({id,display_name:'Planning reference',format:'pdf',file_size:1024,sha256:'fixture',extraction_status:'ready',indexing_status:'pending'});
const scoped=(id,ids=['one'])=>({attachment:attachment(id),meeting_ids:ids});
const searchResult=text=>({passages:[],mode:'keyword',index_status:{keyword_ready:true,semantic_enabled:false,semantic_ready:0,pending:0,failed:0,reason:null},fixture:text});
function view(service={},{scope=meeting('one'),owner={kind:'meeting',id:'one'}}={}) {
  let configuration={provider:'codex',model:'fixture'};
  const hooks=createHookHarness();
  const {useKnowledgeSearch}=loadTsModule('src/hooks/useKnowledgeSearch.ts',{
    react:hooks.react,'@tauri-apps/api/event':{listen:async()=>()=>{}},
    '@/services/knowledgeService':{knowledgeService:{conversations:async()=>[],cancel:async()=>{},history:async()=>[],scopeDocuments:async()=>[scoped('selected'),scoped('other')],documentSharing:async()=>false,setDocumentSharing:async()=>{},...service}},
  });
  return {...hooks,render:()=>hooks.render(()=>useKnowledgeSearch(scope,owner,configuration)),scope(next){scope=next;},owner(next){owner=next;},provider(next){configuration=next;}};
}
async function ready(app) { app.render();await flush();app.render();await flush();return app.render(); }
function select(state,id,enabled=true) {
  assert.equal(typeof state.selectDocument,'function','reference selection must be available');
  state.selectDocument(id,enabled);
}

test('empty library scope leaves references empty without reporting a loading failure',async()=>{
  const scope={kind:'library',filter:{all_meetings:false,meeting_ids:[],tags:[],tag_mode:'any',untagged:false,from:null,to:null}};
  const app=view({scopeDocuments:async()=>{throw new Error('Native scope validation rejects an empty selection');}},{scope,owner:null});
  await ready(app);
  assert.deepEqual([...app.render().documents],[]);
  assert.equal(app.render().documentsLoading,false);
  assert.equal(app.render().documentsError,'');
  app.unmount();
});

test('unselected_document_not_retrieved: search and answer carry only explicitly selected IDs',async()=>{
  const searches=[],asks=[];
  const app=view({search:async request=>{searches.push(request);return searchResult('search');},ask:async request=>{asks.push(request);}});
  await ready(app);await app.render().search('before selection','keyword');
  assert.deepEqual([...searches[0].document_ids],[]);
  select(app.render(),'selected');await ready(app);
  await app.render().search('selected only','keyword');await app.render().ask('Question?','keyword');
  assert.deepEqual([...searches[1].document_ids],['selected']);assert.deepEqual([...asks[0].search.document_ids],['selected']);
  select(app.render(),'selected',false);await ready(app);await app.render().ask('Another question?','keyword');
  assert.deepEqual([...asks[1].search.document_ids],[]);app.unmount();
});

test('foreign_attachment_rejected: selection rejects unknown IDs and prunes IDs outside a new scope',async()=>{
  const reads=[];let documents=[scoped('selected')];
  const app=view({scopeDocuments:async()=>documents,search:async request=>{reads.push(request);return searchResult('search');}});
  await ready(app);select(app.render(),'foreign');select(app.render(),'selected');await ready(app);
  await app.render().search('first','keyword');assert.deepEqual([...reads[0].document_ids],['selected']);
  documents=[scoped('new',['two'])];app.scope(meeting('two'));app.owner({kind:'meeting',id:'two'});await ready(app);
  await app.render().search('second','keyword');assert.deepEqual([...reads[1].document_ids],[]);app.unmount();
});

for(const change of ['scope','provider','selection']) {
  test(`${change} change cancels document answer and discards its late response`,async()=>{
    const answer=deferred(),cancelled=[],owners=[];
    const app=view({cancel:async id=>{cancelled.push(id);},ask:()=>answer.promise,history:async owner=>{owners.push(owner.id);return [];}});
    await ready(app);select(app.render(),'selected');await ready(app);
    const work=app.render().ask('Question?','keyword');
    if(change==='scope')app.scope(meeting('two'));
    if(change==='provider')app.provider({provider:'ollama',model:'fixture'});
    if(change==='selection')select(app.render(),'selected',false);
    await ready(app);const historyBefore=owners.length;answer.resolve({});await work;
    assert.equal(cancelled.length,1);assert.equal(owners.length,historyBefore,'late completion must not reload history');
    assert.equal(app.render().messages.length,0);app.unmount();
  });
}

test('scope changes discard late document lists before they can be selected',async()=>{
  const old=deferred();let calls=0;
  const app=view({scopeDocuments:()=>++calls===1?old.promise:Promise.resolve([scoped('new',['two'])])});
  app.render();app.scope(meeting('two'));app.owner({kind:'meeting',id:'two'});await ready(app);
  old.resolve([scoped('old')]);await flush();
  assert.ok(Array.isArray(app.render().documents),'scoped references must be exposed');
  assert.deepEqual([...app.render().documents].map(row=>row.attachment.id),['new']);app.unmount();
});

test('permission changes belong to the current conversation owner and reject a late old-owner read',async()=>{
  const old=deferred();const writes=[];
  const app=view({documentSharing:owner=>owner.id==='one'?old.promise:Promise.resolve(false),setDocumentSharing:async(owner,enabled)=>{writes.push({owner,enabled});}});
  app.render();app.owner({kind:'meeting',id:'two'});await ready(app);old.resolve(true);await flush();
  assert.equal(app.render().sharingEnabled,false,'old permission must never enable the new owner');
  assert.equal(typeof app.render().setDocumentSharing,'function');await app.render().setDocumentSharing(true);
  assert.equal(writes.length,1);assert.equal(writes[0].owner.id,'two');assert.equal(writes[0].enabled,true);app.unmount();
});

test('enabling library document sharing lazily creates one durable conversation owner',async()=>{
  const creation=deferred();let creates=0;const writes=[];
  const scope={kind:'library',filter:{all_meetings:false,meeting_ids:['one'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}};
  const app=view({createConversation:()=>{creates++;return creation.promise;},setDocumentSharing:async(owner,enabled)=>{writes.push({owner,enabled});}},{scope,owner:null});
  await ready(app);assert.equal(typeof app.render().setDocumentSharing,'function');
  const first=app.render().setDocumentSharing(true),duplicate=app.render().setDocumentSharing(true);
  assert.equal(creates,1);creation.resolve({kind:'library',id:'references-thread'});await Promise.all([first,duplicate]);await ready(app);
  assert.equal(writes.length,1);assert.equal(writes[0].owner.id,'references-thread');assert.equal(app.render().owner.id,'references-thread');app.unmount();
});

test('failed library sharing remains visible after its newly created owner is adopted',async()=>{
  const scope={kind:'library',filter:{all_meetings:false,meeting_ids:['one'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}};
  const app=view({createConversation:async()=>({kind:'library',id:'references-thread'}),setDocumentSharing:async()=>{throw new Error('Permission storage unavailable');}},{scope,owner:null});
  await ready(app);await app.render().setDocumentSharing(true);await ready(app);
  assert.equal(app.render().owner.id,'references-thread');
  assert.equal(app.render().sharingEnabled,false);
  assert.match(app.render().sharingError,/could not be saved/i);
  app.unmount();
});

const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>typeof node==='string'||typeof node==='number'?String(node):Array.isArray(node)?node.map(text).join(''):node?text(node.props?.children):'';

for(const [provider,selectedDocumentIds] of [['codex',[]],['builtin-ai',['selected']]]) {
  test(`${provider} questions without external reference context stay available during permission reads`,()=>{
    const hooks=createHookHarness();
    const {KnowledgeChat}=loadTsModule('src/components/Knowledge/KnowledgeChat.tsx',{
      react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
      'lucide-react':{Info:'info',MoreHorizontal:'more'},
      '@/services/knowledgeService':{knowledgeService:{}},
      '@/components/ui/button':{Button:'button'},'@/components/ui/textarea':{Textarea:'textarea'},
      '@/components/ui/scroll-area':{ScrollArea:'scroll-area'},
      '@/components/ui/tooltip':{},'@/components/ui/dropdown-menu':{},
    });
    const state={scope:meeting('one'),owner:{kind:'meeting',id:'one'},threads:[],messages:[],historyLoading:false,sending:false,creating:false,selectedDocumentIds,sharingEnabled:false,sharingLoading:true,ask:async()=>{}};
    const render=()=>hooks.render(()=>KnowledgeChat({state,mode:'keyword',provider,model:'fixture'}));
    nodes(render()).find(node=>node.type==='textarea').props.onChange({target:{value:'What was decided?'}});
    assert.equal(nodes(render()).find(node=>node.type==='button'&&text(node)==='Ask').props.disabled,false);
    hooks.unmount();
  });
}
const documentRef={source_id:'document:reference',source_revision:1,chunk_id:'block',fingerprint:'fixture',locator:{kind:'document',document_id:'reference',page:7,paragraph:3,spans:[{transcript_id:'block',start_byte:0,end_byte:24}]}};
test('document evidence opens the actual page and paragraph without transcript or playback controls',()=>{
  const {EvidencePreview}=loadTsModule('src/components/Knowledge/EvidencePreview.tsx',{
    'react/jsx-runtime':{jsx,jsxs:jsx},'next/navigation':{useRouter:()=>({push(){}})},'@/components/Sidebar/SidebarProvider':{useSidebar:()=>({setCurrentMeeting(){}})},
    '@/components/ui/button':{Button:'button'},'@/components/ui/dialog':Object.fromEntries(['Dialog','DialogContent','DialogDescription','DialogHeader','DialogTitle'].map(name=>[name,name])),
    '@/services/knowledgeService':{knowledgeService:{}},
  });
  const metadata={title:'Planning reference',date:'2026-10-01',speaker:null,metadata_truncated:false};
  const tree=EvidencePreview({state:{preview:{reference:documentRef,metadata,resolved:{status:'current',navigation:null,passage:{...metadata,evidence:documentRef,meeting_id:'one',text:'Verified extracted passage',rank:1}}},previewBusy:false,error:'',closePreview(){},inspect(){}}});
  assert.match(text(tree),/Page 7.*paragraph 3/i);assert.match(text(tree),/Verified extracted passage/);
  assert.equal(nodes(tree).some(node=>node.type==='button'&&/Show in transcript|Play from here/.test(text(node))),false);
});

const element=tag=>({children,asChild,...props})=>React.createElement(tag,props,children);
test('document anchors remain readable in a map above 64 without changing small transcript maps',()=>{
  const {KnowledgeChat}=loadTsModule('src/components/Knowledge/KnowledgeChat.tsx',{
    'lucide-react':{Info:element('span'),MoreHorizontal:element('span')},'@/services/knowledgeService':{knowledgeService:{}},
    '@/components/ui/button':{Button:element('button')},'@/components/ui/textarea':{Textarea:element('textarea')},'@/components/ui/scroll-area':{ScrollArea:element('div')},
    '@/components/ui/tooltip':Object.fromEntries(['Tooltip','TooltipTrigger','TooltipContent','TooltipProvider'].map(name=>[name,element('span')])),
    '@/components/ui/dropdown-menu':Object.fromEntries(['DropdownMenu','DropdownMenuContent','DropdownMenuItem','DropdownMenuTrigger','DropdownMenuSeparator','DropdownMenuLabel'].map(name=>[name,element('div')])),
  });
  const metadata={title:'Planning reference',date:'2026-10-01',speaker:null,metadata_truncated:false};
  const transcript={...documentRef,source_id:'meeting:one',locator:{kind:'transcript',meeting_id:'one',transcript_ids:['row'],spans:[{transcript_id:'row',start_byte:0,end_byte:24}],start_seconds:12}};
  function render(count,reference) {
    const reply={request_id:'request',message_id:'answer',content:`Evidence [K${count}].`,evidence:Array.from({length:count},()=>reference),evidence_metadata:Array.from({length:count},()=>metadata),cited_tags:[count],context_links:[],retrieval_mode:'keyword',provider:'codex',model:'fixture'};
    const state={scope:meeting('one'),owner:{kind:'meeting',id:'one'},threads:[],messages:[{id:'answer',role:'assistant',content:reply.content,status:'completed',reply}],historyLoading:false,sending:false,creating:false,inspect(){},ask:async()=>{}};
    return renderToStaticMarkup(React.createElement(KnowledgeChat,{state,mode:'keyword',provider:'codex',model:'fixture'}));
  }
  const large=render(700,documentRef);assert.match(large,/aria-label="Open source 700"/);assert.match(large,/Page 7.*paragraph 3/i);assert.doesNotMatch(large,/Time unavailable|\[K700\]/);
  const small=render(2,transcript);assert.match(small,/aria-label="Open source 2"/);assert.match(small,/0:12/);assert.doesNotMatch(small,/\[K2\]/);
});
