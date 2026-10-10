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

test('corner chat is a compact non-modal popup with optional source controls',()=>{
  const hooks=createHookHarness();
  const {MeetingChat}=loadTsModule('src/components/MeetingDetails/MeetingChat.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'lucide-react':{MessageSquare:'chat-icon',X:'close',ChevronDown:'down'},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:()=>({scope:{kind:'meeting',meeting_id:'one'},messages:[],cancel(){},reloadHistory(){}})},
    '@/components/Sidebar/SidebarProvider':{useSidebar:()=>({meetings,projectTags:[],projectTagsError:null,projectTagsLoading:false})},
    '@/components/ui/popover':{Popover:'popover',PopoverTrigger:'trigger',PopoverContent:'popup'},
    '@/components/ui/checkbox':{},'@/components/ui/select':{},'@/components/ui/multi-select':{},
    '@/components/Knowledge/KnowledgeChat':{KnowledgeChat:'knowledge-chat'},
    '@/components/Knowledge/EvidencePreview':{EvidencePreview:'preview'},
    './ReferenceDocuments':{ReferenceDocuments:'references'},
  });
  const render=()=>hooks.render(()=>MeetingChat({meetingId:'one',provider:'ollama',model:'fixture'}));
  const tree=render(),popup=nodes(tree).find(node=>node.type==='popup');
  assert.ok(popup,'corner chat must use a popup instead of a centered modal');
  assert.equal(nodes(tree).find(node=>node.type==='popover').props.modal,false);
  assert.match(popup.props.className,/w-\[26rem\]/);
  assert.match(popup.props.className,/h-\[32rem\]/);
  const secondary=nodes(tree).find(node=>node.type==='knowledge-chat').props.secondary;
  assert.equal(nodes(secondary).some(node=>node.type==='fieldset'),false,'source controls must not crowd the initial popup');
  nodes(secondary).find(node=>node.type==='button'&&text(node)==='Question scope').props.onClick();
  assert.ok(nodes(nodes(render()).find(node=>node.type==='knowledge-chat').props.secondary).some(node=>node.type==='fieldset'));
  hooks.unmount();
});
