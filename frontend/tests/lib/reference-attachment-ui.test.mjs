import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,deferred,flush} from './hook-harness.mjs';

const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>typeof node==='string'||typeof node==='number'?String(node):Array.isArray(node)?node.map(text).join(''):node?text(node.props?.children):'';
const meetings=[{id:'one',title:'Planning meeting'},{id:'two',title:'Budget review'}];
const ui={
  '@/components/ui/button':{Button:'button'}, '@/components/ui/input':{Input:'input'},
  '@/components/ui/scroll-area':{ScrollArea:'scroll-area'},
  '@/components/ui/dialog':Object.fromEntries(['Dialog','DialogContent','DialogDescription','DialogHeader','DialogTitle'].map(name=>[name,name])),
};
function attachment({meetingId,targets=meetings,open=async()=> 'planning.pdf',service={},attached=()=>{}}={}) {
  const hooks=createHookHarness();
  let props={meetingId,meetings:targets,onAttached:attached};
  const {AttachReferenceButton}=loadTsModule('src/components/MeetingDetails/AttachReferenceButton.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'lucide-react':{Paperclip:'clip',Check:'check'},'@tauri-apps/plugin-dialog':{open},
    '@/services/knowledgeService':{knowledgeService:{importDocument:async()=>({id:'document'}),cancelDocumentImport:async()=>{},...service}},
  });
  const render=()=>hooks.render(()=>AttachReferenceButton(props));
  return {...hooks,render,props(next){props={...props,...next};},button(label){return nodes(render()).find(node=>node.type==='button'&&text(node)===label);}};
}

test('direct attachment uses the selected meeting and suppresses duplicate file dialogs',async()=>{
  const file=deferred(),imports=[],attached=[];let dialogs=0;
  const app=attachment({meetingId:'one',open:()=>{dialogs++;return file.promise;},service:{importDocument:async(...args)=>{imports.push(args);}},attached:id=>attached.push(id)});
  const click=app.button('Attach file').props.onClick;
  const first=click();click();assert.equal(dialogs,1);
  file.resolve('planning.pdf');await first;await flush();
  assert.equal(imports.length,1);assert.equal(imports[0][0],'one');assert.equal(imports[0][1],'planning.pdf');
  assert.match(imports[0][2],/^[\da-f-]{36}$/i);assert.deepEqual(attached,['one']);app.unmount();
});

test('no single selected meeting opens a searchable destination picker before the file dialog',async()=>{
  let dialogs=0;const imports=[];
  const app=attachment({open:async()=>{dialogs++;return 'planning.docx';},service:{importDocument:async id=>imports.push(id)}});
  app.button('Attach file').props.onClick();assert.equal(dialogs,0);
  const input=nodes(app.render()).find(node=>node.type==='input');
  assert.ok(input,'destination picker must be searchable');
  input.props.onChange({target:{value:'Budget'}});
  assert.equal(app.button('Planning meeting'),undefined);
  app.button('Budget review').props.onClick();await flush();
  assert.equal(dialogs,1);assert.deepEqual(imports,['two']);app.unmount();
});

test('cancelled file picker does not attach and permits a subsequent attempt',async()=>{
  let picks=0,imports=0;
  const app=attachment({meetingId:'one',open:async()=>++picks===1?null:'planning.md',service:{importDocument:async()=>imports++}});
  await app.button('Attach file').props.onClick();await flush();assert.equal(imports,0);
  await app.button('Attach file').props.onClick();await flush();assert.equal(imports,1);app.unmount();
});

test('one available but unselected meeting still requires an explicit destination choice',async()=>{
  let dialogs=0;const imports=[];
  const app=attachment({targets:[meetings[1]],open:async()=>{dialogs++;return 'planning.pdf';},service:{importDocument:async id=>imports.push(id)}});
  try {
    app.button('Attach file').props.onClick();
    assert.equal(dialogs,0,'an available meeting must never replace the selected destination silently');
    assert.ok(nodes(app.render()).find(node=>node.type==='input'));
    app.button('Budget review').props.onClick();await flush();
    assert.deepEqual(imports,['two']);
  } finally { app.unmount(); }
});

test('scope change during file selection ignores a stale file picker result',async()=>{
  const picker=deferred(),imports=[],attached=[];
  const app=attachment({meetingId:'one',open:()=>picker.promise,service:{importDocument:async(...args)=>imports.push(args)},attached:id=>attached.push(id)});
  try {
    const request=app.button('Attach file').props.onClick();
    app.props({meetingId:'two',scopeKey:'new-scope'});app.render();
    picker.resolve('planning.pdf');await request;
    assert.deepEqual(imports,[]);assert.deepEqual(attached,[]);
    assert.equal(app.button('Attach file').props.disabled,false);
  } finally { app.unmount(); }
});

test('scope change during extraction cancels the old request and ignores late success',async()=>{
  const work=deferred(),cancelled=[],attached=[];
  const app=attachment({meetingId:'one',service:{importDocument:()=>work.promise,cancelDocumentImport:async id=>cancelled.push(id)},attached:id=>attached.push(id)});
  try {
    const request=app.button('Attach file').props.onClick();await flush();
    app.props({meetingId:'two',scopeKey:'new-scope'});app.render();
    assert.equal(cancelled.length,1);
    work.resolve({id:'document'});await request;
    assert.deepEqual(attached,[]);
    assert.equal(app.button('Attach file').props.disabled,false);
  } finally { app.unmount(); }
});

test('Cancel extraction is idempotent and preserves an actionable cancelled state',async()=>{
  const work=deferred(),cancellation=deferred(),cancelled=[],attached=[];
  const app=attachment({meetingId:'one',service:{importDocument:()=>work.promise,cancelDocumentImport:id=>{cancelled.push(id);return cancellation.promise;}},attached:id=>attached.push(id)});
  try {
    const request=app.button('Attach file').props.onClick();await flush();
    const cancel=app.button('Cancel extraction').props.onClick;cancel();cancel();
    assert.equal(cancelled.length,1);
    assert.match(text(app.render()),/Cancelling extraction/);
    cancellation.resolve();work.reject('cancelled');await request;await flush();
    assert.deepEqual(attached,[]);assert.match(text(app.render()),/Document import was cancelled/);
    assert.equal(app.button('Attach file').props.disabled,false);
  } finally { app.unmount(); }
});

test('failed import displays a safe actionable error and allows retry',async()=>{
  let attempts=0;const attached=[];
  const app=attachment({meetingId:'one',service:{importDocument:async()=>{if(++attempts===1)throw 'busy';}},attached:id=>attached.push(id)});
  await app.button('Attach file').props.onClick();await flush();
  assert.match(text(app.render()),/Finish recording or transcription/i);assert.deepEqual(attached,[]);
  await app.button('Attach file').props.onClick();await flush();assert.deepEqual(attached,['one']);app.unmount();
});

test('unmount cancels extraction and ignores a late successful attachment',async()=>{
  const work=deferred(),cancelled=[],attached=[];
  const app=attachment({meetingId:'one',service:{importDocument:()=>work.promise,cancelDocumentImport:async id=>cancelled.push(id)},attached:id=>attached.push(id)});
  const request=app.button('Attach file').props.onClick();await flush();
  assert.ok(app.button('Cancel extraction'));app.unmount();
  work.resolve({id:'document'});await request;
  assert.equal(cancelled.length,1);assert.deepEqual(attached,[]);
});

test('empty library offers a clear disabled attachment action',()=>{
  const app=attachment({targets:[]});
  assert.equal(app.button('Attach file').props.disabled,true);
  assert.match(text(app.render()),/Save a meeting/i);app.unmount();
});

test('Meeting memory exposes attachment beside Sources and includes the destination without choosing document excerpts',()=>{
  const hooks=createHookHarness();let reloads=0;
  const {KnowledgeArchive}=loadTsModule('src/components/Knowledge/KnowledgeArchive.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'lucide-react':{ChevronDown:'down',X:'x'},
    '@/contexts/ConfigContext':{useConfig:()=>({modelConfig:{provider:'ollama',model:'fixture'}})},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:scope=>({scope,documents:[],messages:[],selectedDocumentIds:[],reloadDocuments:()=>reloads++})},
    '@/services/knowledgeService':{knowledgeService:{indexStatus:async()=>({})}},
    '@/components/MeetingDetails/AttachReferenceButton':{AttachReferenceButton:'attach'},
    '@/components/MeetingDetails/ReferenceDocuments':{ReferenceDocuments:'references'},
    '@/components/ui/checkbox':{Checkbox:'checkbox'},'@/components/ui/switch':{Switch:'switch'},
    '@/components/ui/popover':{},'@/components/ui/select':{},'@/components/ui/tooltip':{},
    './KnowledgeChat':{KnowledgeChat:'chat'},'./EvidencePreview':{EvidencePreview:'preview'},'./SearchResults':{},
  });
  const render=()=>hooks.render(()=>KnowledgeArchive({meetings,projectFilter:{tags:[],mode:'any',untagged:false}}));
  try {
    const tree=render(),sourcePanel=nodes(tree).find(node=>node.type==='fieldset');
    const action=nodes(sourcePanel).find(node=>node.type==='attach');
    assert.ok(action,'Attach file must be available outside the conversation content');
    action.props.onAttached('two');
    const chat=nodes(render()).find(node=>node.type==='chat');
    assert.deepEqual([...chat.props.state.scope.filter.meeting_ids],['two']);
    assert.deepEqual([...chat.props.state.selectedDocumentIds],[],'attaching must not authorize excerpts');
    assert.equal(reloads,1);
  } finally { hooks.unmount(); }
});

function cornerChat() {
  const hooks=createHookHarness();
  const selected=[],state={scope:{kind:'meeting',meeting_id:'one'},messages:[],documents:[{attachment:{id:'file',display_name:'Plan.pdf'},meeting_ids:['one']}],selectedDocumentIds:[],cancel(){},reloadHistory(){},reloadDocuments(){},selectDocument:(id,enabled)=>selected.push([id,enabled])};
  const {MeetingChat}=loadTsModule('src/components/MeetingDetails/MeetingChat.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'lucide-react':{MessageSquare:'chat-icon',X:'close',ChevronDown:'down'},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:scope=>({...state,scope})},
    '@/components/Sidebar/SidebarProvider':{useSidebar:()=>({meetings,projectTags:[],projectTagsError:null,projectTagsLoading:false})},
    '@/components/ui/popover':{Popover:'popover',PopoverTrigger:'trigger',PopoverContent:'popup'},
    '@/components/ui/checkbox':{Checkbox:'checkbox'},'@/components/ui/switch':{Switch:'switch'},'@/components/ui/select':{},'@/components/ui/multi-select':{},
    '@/components/ui/dropdown-menu':{DropdownMenuLabel:'menu-label',DropdownMenuRadioGroup:'radio-group',DropdownMenuRadioItem:'radio-item',DropdownMenuSeparator:'separator'},
    '@/components/Knowledge/KnowledgeChat':{KnowledgeChat:'knowledge-chat'},
    '@/components/Knowledge/EvidencePreview':{EvidencePreview:'preview'},
    './ReferenceDocuments':{ReferenceDocuments:'references'},
    './AttachReferenceButton':{AttachReferenceButton:'attach'},
  });
  const render=()=>hooks.render(()=>MeetingChat({meetingId:'one',provider:'ollama',model:'fixture'}));
  return {...hooks,render,state,selected,chat:()=>nodes(render()).find(node=>node.type==='knowledge-chat')};
}

test('corner chat keeps fixed sizing, non-modal behavior and configuration in a context overlay',()=>{
  const app=cornerChat(),{render}=app;
  const tree=render(),popup=nodes(tree).find(node=>node.type==='popup');
  assert.ok(popup,'corner chat must use a popup instead of a centered modal');
  assert.equal(nodes(tree).find(node=>node.type==='popover').props.modal,false);
  assert.match(popup.props.className,/w-\[26rem\]/);
  assert.match(popup.props.className,/h-\[min\(36rem,calc\(100vh-6rem\)\)\]/);
  assert.match(popup.props.className,/max-h-\[calc\(100vh-6rem\)\]/);
  assert.equal(app.chat().props.secondary,undefined);
  const context=app.chat().props.contextBar;
  assert.ok(nodes(context).some(node=>node.type==='popup'&&/max-h-\[60vh\]/.test(node.props.className)));
  const popover=nodes(context).find(node=>node.type==='popover');
  popover.props.onOpenChange(true);
  let stopped=0;nodes(app.chat().props.contextBar).find(node=>node.type==='popup').props.onEscapeKeyDown({stopPropagation:()=>stopped++});
  assert.equal(stopped,1,'context Escape must not bubble to the chat');
  let prevented=0;nodes(render()).find(node=>node.type==='popup').props.onEscapeKeyDown({preventDefault:()=>prevented++});
  assert.equal(prevented,1,'outer chat must ignore Escape while Context is open');
  app.unmount();
});

test('context summarizes scope and selected file count and bounds the searchable meeting picker',()=>{
  const app=cornerChat();
  const context=()=>app.chat().props.contextBar;
  const summary=()=>nodes(context()).find(node=>node.type==='button'&&node.props['aria-label']?.startsWith('Context:'));
  try {
    assert.equal(summary().props['aria-label'],'Context: This meeting');
    app.state.selectedDocumentIds=['file'];assert.equal(text(summary()),'This meeting · 1 file');
    nodes(context()).find(node=>node.type==='button'&&text(node)==='Other saved meetings').props.onClick();
    assert.equal(text(summary()),'1 meeting · 1 file');
    const input=nodes(context()).find(node=>node.type==='input'&&node.props['aria-label']==='Find a meeting');assert.ok(input);
    const picker=nodes(context()).find(node=>node.type==='scroll-area');assert.match(picker.props.className,/h-56/);
    input.props.onChange({target:{value:'Budget'}});
    assert.equal(nodes(context()).some(node=>node.type==='label'&&text(node)==='Planning meeting'),false);
    nodes(context()).find(node=>node.type==='label'&&text(node)==='Budget review').props.children[0].props.onCheckedChange(true);
    assert.equal(text(summary()),'2 meetings · 1 file');
    nodes(context()).find(node=>node.type==='switch').props.onCheckedChange(true);
    assert.equal(text(summary()),'All meetings · 1 file');
    nodes(context()).find(node=>node.type==='switch').props.onCheckedChange(false);
    assert.equal(text(summary()),'2 meetings · 1 file','all mode preserves the selection');
  } finally {app.unmount();}
});

test('composer attachment and context Add file share one importer and selected chips deselect only',()=>{
  const app=cornerChat();let clicks=0;
  try {
    app.state.selectedDocumentIds=['file'];const chat=app.chat();
    assert.equal(chat.props.composerStart.type,'attach');
    assert.equal(chat.props.composerStart.props.variant,'icon');assert.equal(chat.props.composerStart.props.meetingId,'one');
    chat.props.composerStart.props.buttonRef.current={click:()=>clicks++};
    nodes(chat.props.contextBar).find(node=>node.type==='references').props.onAddFile();assert.equal(clicks,1);
    const remove=nodes(chat.props.attachments).find(node=>node.props?.['aria-label']==='Remove Plan.pdf from this question');
    assert.ok(remove);remove.props.onClick();assert.deepEqual(app.selected,[['file',false]]);
    chat.props.composerStart.props.onBusyChange(true);
    assert.equal(app.chat().props.composerStart.props.disabled,true,'a remounted composer cannot start another import before the old one settles');
    assert.equal(nodes(app.chat().props.contextBar).find(node=>node.type==='references').props.attachmentBusy,true);
    nodes(app.chat().props.contextBar).find(node=>node.type==='references').props.onBusyChange(true);
    assert.equal(app.chat().props.composerStart.props.disabled,true);
    assert.equal(nodes(app.chat().props.contextBar).find(node=>node.type==='references').props.disabled,true,'reopened Context must preserve an in-flight action lock');
  } finally {app.unmount();}
});

test('search mode uses header radio menu and cancels before changing mode',()=>{
  const app=cornerChat();let cancelled=0;app.state.cancel=()=>cancelled++;
  try {
    const chat=app.chat(),extra=chat.props.headerMenuExtra;
    assert.equal(nodes(extra).find(node=>node.type==='menu-label').props.children,'Search mode');
    const radio=nodes(extra).find(node=>node.type==='radio-group');assert.equal(radio.props.value,'keyword');
    radio.props.onValueChange('hybrid');assert.equal(app.chat().props.mode,'hybrid');assert.equal(cancelled,1);
    assert.equal(nodes(chat.props.contextBar).some(node=>node.type==='radio-group'||text(node)==='Retrieval'),false);
    assert.deepEqual([...chat.props.suggestions],['What did we decide?','List the action items','Summarize the key points']);
  } finally {app.unmount();}
});

test('long file selections keep the composer bounded and offer Context for the remaining files',()=>{
  const app=cornerChat();
  try {
    app.state.documents=Array.from({length:20},(_,index)=>({attachment:{id:`file-${index}`,display_name:`Long reference filename ${index}.pdf`},meeting_ids:['one']}));
    app.state.selectedDocumentIds=app.state.documents.map(row=>row.attachment.id);
    const chips=app.chat().props.attachments;
    assert.equal(nodes(chips).filter(node=>node.props?.['aria-label']?.startsWith('Remove ')).length,2);
    const more=nodes(chips).find(node=>node.props?.['aria-label']==='Manage 18 more reference files in Context');
    assert.ok(more);more.props.onClick();
    assert.equal(nodes(app.chat().props.contextBar).find(node=>node.type==='popover').props.open,true);
    assert.match(text(nodes(app.chat().props.contextBar).find(node=>node.props?.['aria-label']?.startsWith('Context:'))),/20 files/);
  } finally {app.unmount();}
});

test('reference completion refreshes the current state owner after Context closes',()=>{
  const app=cornerChat();let reloads=0;app.state.reloadDocuments=()=>reloads++;
  try {
    const references=nodes(app.chat().props.contextBar).find(node=>node.type==='references');
    references.props.onDocumentsChanged('file','detach');
    assert.deepEqual(app.selected,[['file',false]]);assert.equal(reloads,1);
    references.props.onActionError('Could not update reference');
    assert.equal(nodes(app.chat().props.contextBar).find(node=>node.type==='references').props.actionError,'Could not update reference');
  } finally {app.unmount();}
});

test('icon attachment uses the same busy, cancel and safe error flow',async()=>{
  const work=deferred(),cancelled=[],attached=[],busy=[];
  const app=attachment({meetingId:'one',service:{importDocument:()=>work.promise,cancelDocumentImport:async id=>cancelled.push(id)},attached:id=>attached.push(id)});
  app.props({variant:'icon',onBusyChange:value=>busy.push(value)});
  const clip=()=>nodes(app.render()).find(node=>node.props?.['aria-label']==='Attach file to this meeting');
  try {
    assert.ok(clip());const request=clip().props.onClick();await flush();
    assert.equal(clip().props.disabled,true);assert.match(text(app.render()),/Extracting locally/);
    app.button('Cancel extraction').props.onClick();await flush();assert.equal(cancelled.length,1);
    work.reject('cancelled');await request;assert.match(text(app.render()),/Document import was cancelled/);
    assert.equal(clip().props.disabled,false);assert.deepEqual(attached,[]);assert.deepEqual(busy,[true,false]);
  } finally {app.unmount();}
});

test('closing chat during extraction releases the shared attachment lock after cancellation settles',async()=>{
  const work=deferred(),busy=[],cancelled=[];
  const app=attachment({meetingId:'one',service:{importDocument:()=>work.promise,cancelDocumentImport:async id=>cancelled.push(id)}});
  app.props({variant:'icon',onBusyChange:value=>busy.push(value)});
  const clip=nodes(app.render()).find(node=>node.props?.['aria-label']==='Attach file to this meeting');
  const request=clip.props.onClick();await flush();app.unmount();
  assert.equal(cancelled.length,1);work.reject('cancelled');await request;
  assert.deepEqual(busy,[true,false],'parent Context controls must unlock when an unmounted import finishes');
});

test('closing chat during file selection releases the shared lock without importing a late selection',async()=>{
  const picker=deferred(),busy=[],imports=[];
  const app=attachment({meetingId:'one',open:()=>picker.promise,service:{importDocument:async(...args)=>imports.push(args)}});
  app.props({variant:'icon',onBusyChange:value=>busy.push(value)});
  const clip=nodes(app.render()).find(node=>node.props?.['aria-label']==='Attach file to this meeting');
  const request=clip.props.onClick();app.unmount();picker.resolve('planning.pdf');await request;
  assert.deepEqual(imports,[]);assert.deepEqual(busy,[true,false]);
});
