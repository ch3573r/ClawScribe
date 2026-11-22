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
  '@/components/ui/tooltip':{Tooltip:'tooltip',TooltipTrigger:'tooltip-trigger',TooltipContent:'tooltip-content'},
  '@/components/ui/dialog':Object.fromEntries(['Dialog','DialogContent','DialogDescription','DialogHeader','DialogTitle'].map(name=>[name,name])),
};
function attachment({meetingId,targets=meetings,open=async()=> 'planning.pdf',service={},attached=()=>{}}={}) {
  const hooks=createHookHarness();
  let props={meetingId,meetings:targets,onAttached:attached};
  const {AttachReferenceButton}=loadTsModule('src/components/MeetingDetails/AttachReferenceButton.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'lucide-react':{Paperclip:'clip',Check:'check'},'@tauri-apps/api/core':{invoke:async command=>{if(command!=='pick_reference_document')throw new Error('unexpected '+command);return open();}},
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
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:scope=>({scope,documents:[],messages:[],selectedDocumentIds:[],reloadDocuments:()=>reloads++,refreshDocuments(){}})},
    '@/services/knowledgeService':{knowledgeService:{indexStatus:async()=>({})}},
    '@/components/MeetingDetails/AttachReferenceButton':{AttachReferenceButton:'attach'},
    '@/components/MeetingDetails/ReferenceDocuments':{ReferenceDocuments:'references'},
    '@/components/ui/checkbox':{Checkbox:'checkbox'},'@/components/ui/switch':{Switch:'switch'},
    '@/components/ui/popover':{},'@/components/ui/select':{},'@/components/ui/tooltip':{},
    './KnowledgeChat':{KnowledgeChat:'chat'},'./EvidencePreview':{EvidencePreview:'preview'},'./SearchResults':{},
  });
  const render=()=>hooks.render(()=>KnowledgeArchive({meetings,projectFilter:{tags:[],mode:'any',untagged:false}}));
  try {
    const initialChat=nodes(render()).find(node=>node.type==='chat');
    const sourcePanel=nodes(initialChat.props.secondary).find(node=>node.type==='fieldset');
    const action=nodes(sourcePanel).find(node=>node.type==='attach');
    assert.ok(action,'Attach file must be available alongside the source controls before a conversation starts');
    action.props.onAttached('two');
    const chat=nodes(render()).find(node=>node.type==='chat');
    assert.deepEqual([...chat.props.state.scope.filter.meeting_ids],['two']);
    assert.deepEqual([...chat.props.state.selectedDocumentIds],[],'attaching must not authorize excerpts');
    assert.equal(reloads,1);
  } finally { hooks.unmount(); }
});

function cornerChat(service, options = {}) {
  const hooks=createHookHarness();
  const selected=[],state={scope:{kind:'meeting',meeting_id:'one'},messages:[],documents:[{attachment:{id:'file',display_name:'Plan.pdf'},meeting_ids:['one']}],selectedDocumentIds:[],cancel(){},reloadHistory(){},reloadDocuments(){},refreshDocuments(){},selectDocument:(id,enabled)=>selected.push([id,enabled])};
  const {useKnowledgeSearch}=service?loadTsModule('src/hooks/useKnowledgeSearch.ts',{
    react:hooks.react,'@tauri-apps/api/event':{listen:async()=>()=>{}},
    '@/services/knowledgeService':{knowledgeService:{conversations:async()=>[],conversationScope:async()=>({all_meetings:false,meeting_ids:['one'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}),cancel:async()=>{},history:async()=>[],scopeDocuments:async()=>[],documentSharing:async()=>false,...service}},
  }):{useKnowledgeSearch:scope=>({...state,scope})};
  const {MeetingChat}=loadTsModule('src/components/MeetingDetails/MeetingChat.tsx',{
    ...ui,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    'react-dom':{createPortal:(children,container)=>jsx('portal',{children,container})},
    'lucide-react':{MessageSquare:'chat-icon',X:'close',ChevronDown:'down'},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch},
    '@/components/Sidebar/SidebarProvider':{useSidebar:()=>({meetings,projectTags:[],projectTagsError:null,projectTagsLoading:false})},
    '@/components/ui/popover':{Popover:'popover',PopoverTrigger:'trigger',PopoverContent:'popup'},
    '@/components/ui/checkbox':{Checkbox:'checkbox'},'@/components/ui/switch':{Switch:'switch'},'@/components/ui/select':{},'@/components/ui/multi-select':{},
    '@/components/ui/dropdown-menu':{DropdownMenuLabel:'menu-label',DropdownMenuRadioGroup:'radio-group',DropdownMenuRadioItem:'radio-item',DropdownMenuSeparator:'separator'},
    '@/components/Knowledge/KnowledgeChat':{KnowledgeChat:'knowledge-chat'},
    '@/components/Knowledge/EvidencePreview':{EvidencePreview:'preview'},
    './ReferenceDocuments':{ReferenceDocuments:'references'},
    './AttachReferenceButton':{AttachReferenceButton:'attach'},
  });
  const render=()=>hooks.render(()=>MeetingChat({meetingId:'one',provider:'ollama',model:'fixture',...options}));
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

test('meeting chat docks at 1280px and returns to its popover without changing scope', () => {
  const previousWindow = globalThis.window;
  let onResize;
  const media = { matches: false, addEventListener: (_, callback) => { onResize = callback; }, removeEventListener: () => { onResize = null; } };
  globalThis.window = { matchMedia: query => { assert.equal(query, '(min-width: 1280px)'); return media; } };
  const dockTarget = { id: 'summary-dock' }, dockChanges = [];
  const app = cornerChat(undefined, { dockTarget, onDockOpenChange: open => dockChanges.push(open) });
  try {
    const launcher = nodes(app.render()).find(node => node.props?.['aria-label'] === 'Chat with this meeting' && node.type === 'button');
    assert.equal(text(launcher), 'Chat with this meeting');
    assert.doesNotMatch(launcher.props.className, /fixed|rounded-full/);
    nodes(app.render()).find(node => node.type === 'popover').props.onOpenChange(true);
    const scope = app.chat().props.state.scope;
    media.matches = true; onResize();
    let tree = app.render();
    assert.equal(nodes(tree).find(node => node.type === 'portal').props.container, dockTarget);
    assert.equal(nodes(tree).filter(node => node.type === 'knowledge-chat').length, 1);
    assert.equal(nodes(tree).some(node => node.type === 'popup'), false);
    assert.equal(dockChanges.at(-1), true);
    assert.deepEqual(app.chat().props.state.scope, scope);
    media.matches = false; onResize(); tree = app.render();
    assert.ok(nodes(tree).some(node => node.type === 'popup'));
    assert.equal(nodes(tree).some(node => node.type === 'portal'), false);
    assert.equal(dockChanges.at(-1), false);
  } finally {
    app.unmount();
    assert.equal(onResize, null);
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
});

test('chat focuses on open, retains composer focus and selection on resize, and closes its dock with Escape', () => {
  const previousWindow = globalThis.window, previousDocument = globalThis.document;
  let onResize, focusCount = 0, launcherFocus = 0, selectionRestores = 0;
  const media = { matches: false, addEventListener: (_, callback) => { onResize = callback; }, removeEventListener() {} };
  const doc = { activeElement: null, createElement: () => host };
  const composer = { selectionStart: 4, selectionEnd: 4, selectionDirection: 'none', focus() { focusCount++; doc.activeElement = this; }, setSelectionRange(start, end) { selectionRestores++; this.selectionStart = start; this.selectionEnd = end; } };
  const host = { ownerDocument: doc, parent: null, className: '', contains: element => element === composer, querySelector: () => composer, remove() { this.parent = null; if (doc.activeElement === composer) doc.activeElement = null; } };
  const popup = { appendChild(node) { node.parent = this; } };
  const dock = { appendChild(node) { node.parent = this; } };
  const launcher = { focus() { launcherFocus++; doc.activeElement = this; } };
  globalThis.window = { matchMedia: () => media };
  globalThis.document = doc;
  const app = cornerChat(undefined, { dockTarget: dock });
  try {
    let tree = app.render();
    nodes(tree).find(node => node.type === 'button' && node.props['aria-label'] === 'Chat with this meeting').props.ref.current = launcher;
    const content = nodes(tree).find(node => node.type === 'popup');
    nodes(content).find(node => node.type === 'div').props.ref(popup);
    nodes(tree).find(node => node.type === 'popover').props.onOpenChange(true);
    tree = app.render();
    assert.equal(nodes(tree).find(node => node.type === 'portal').props.container, host);
    assert.equal(host.parent, popup);
    assert.equal(doc.activeElement, composer, 'popover attachment must focus its composer after mounting');
    assert.equal(focusCount, 1);
    let prevented = 0; content.props.onOpenAutoFocus({ preventDefault() { prevented++; } });
    assert.equal(prevented, 1, 'Radix must leave focus to the delayed host attachment');
    media.matches = true; onResize(); host.remove(); tree = app.render();
    assert.equal(nodes(tree).find(node => node.type === 'portal').props.container, host);
    assert.equal(host.parent, dock);
    assert.equal(doc.activeElement, composer, 'moving the host must restore a focused composer');
    assert.equal(selectionRestores, 1);
    assert.equal(composer.selectionStart, 4);
    content.props.onCloseAutoFocus({ preventDefault() { prevented++; } });
    assert.equal(prevented, 2, 'closing the old popover while docking must not focus the launcher');
    const surface = nodes(tree).find(node => node.type === 'div' && node.props.onKeyDown);
    surface.props.onKeyDown({ key: 'Escape', defaultPrevented: false, preventDefault() {}, stopPropagation() {} });
    tree = app.render();
    assert.equal(nodes(tree).some(node => node.type === 'portal'), false);
    assert.equal(launcherFocus, 1);
    assert.equal(doc.activeElement, launcher);
    nodes(tree).find(node => node.type === 'popover').props.onOpenChange(true); app.render();
    assert.equal(doc.activeElement, composer, 'opening the dock must also focus the composer');
  } finally {
    app.unmount();
    assert.equal(host.parent, null);
    if (previousWindow === undefined) delete globalThis.window; else globalThis.window = previousWindow;
    if (previousDocument === undefined) delete globalThis.document; else globalThis.document = previousDocument;
  }
});

test('chat waits for its visible dock before focusing and cancels pending focus on close', () => {
  const previousWindow = globalThis.window, previousDocument = globalThis.document;
  const frames = new Map(), cancelled = [];
  let nextFrame = 0, onResize, focusCount = 0, pendingVisible = false;
  const media = { matches: true, addEventListener: (_, callback) => { onResize = callback; }, removeEventListener() {} };
  const doc = { activeElement: null, createElement: () => host };
  const composer = { selectionStart: 0, selectionEnd: 0, selectionDirection: 'none', focus() {
    if (host.parent === dock) assert.equal(dock.visible, true, 'focus must wait for the parent to reveal the dock');
    focusCount++; doc.activeElement = this;
  }, setSelectionRange() {} };
  const host = { ownerDocument: doc, className: '', parent: null, contains: element => element === composer, querySelector: () => composer,
    remove() { this.parent = null; if (doc.activeElement === composer) doc.activeElement = null; } };
  const dock = { visible: false, appendChild(node) { node.parent = this; } };
  const popup = { appendChild(node) { node.parent = this; } };
  const launcher = { focus() { doc.activeElement = this; } };
  globalThis.document = doc;
  globalThis.window = { matchMedia: () => media,
    requestAnimationFrame: callback => { frames.set(++nextFrame, callback); return nextFrame; },
    cancelAnimationFrame: frame => { cancelled.push(frame); frames.delete(frame); },
  };
  const app = cornerChat(undefined, { dockTarget: dock, onDockOpenChange: value => { pendingVisible = value; } });
  const flushFrames = () => { for (const [id, callback] of frames) { frames.delete(id); callback(); } };
  try {
    app.render(); let tree = app.render();
    nodes(tree).find(node => node.type === 'button' && node.props['aria-label'] === 'Chat with this meeting').props.ref.current = launcher;
    nodes(tree).find(node => node.type === 'popover').props.onOpenChange(true); tree = app.render();
    assert.equal(pendingVisible, true); assert.equal(focusCount, 0); assert.equal(frames.size, 1);
    dock.visible = pendingVisible; flushFrames();
    assert.equal(focusCount, 1); assert.equal(doc.activeElement, composer);
    media.matches = false; onResize(); tree = app.render();
    nodes(nodes(tree).find(node => node.type === 'popup')).find(node => node.type === 'div').props.ref(popup);
    tree = app.render(); assert.equal(frames.size, 1);
    const pendingFrame = nextFrame;
    nodes(tree).find(node => node.type === 'popover').props.onOpenChange(false); app.render();
    assert.ok(cancelled.includes(pendingFrame)); assert.equal(frames.size, 0);
    flushFrames(); assert.equal(focusCount, 1); assert.equal(doc.activeElement, launcher);
  } finally {
    app.unmount();
    if (previousWindow === undefined) delete globalThis.window; else globalThis.window = previousWindow;
    if (previousDocument === undefined) delete globalThis.document; else globalThis.document = previousDocument;
  }
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

const referenceFile={attachment:{id:'file',display_name:'Plan.pdf',format:'pdf',file_size:1024,sha256:'fixture',extraction_status:'ready',indexing_status:'failed'},meeting_ids:['one']};
const references=app=>nodes(app.chat().props.contextBar).find(node=>node.type==='references');
async function readyChat(app) { app.chat();await flush();app.chat();await flush();return app.chat().props.state; }
async function libraryChat(app) {
  nodes(app.chat().props.contextBar).find(node=>node.type==='button'&&text(node)==='Other saved meetings').props.onClick();
  await readyChat(app);
  await app.chat().props.state.selectConversation({kind:'library',id:'saved-thread'});
  return readyChat(app);
}

test('old-scope retry completion preserves a new library conversation transcripts-only answer',async()=>{
  const answer=deferred(),cancelled=[],requests=[];let documentReads=0,answered=false;
  const app=cornerChat({scopeDocuments:async()=>{documentReads++;return [referenceFile];},cancel:async id=>cancelled.push(id),ask:request=>{requests.push(request);return answer.promise;},history:async()=>answered?[{id:'saved-answer',role:'assistant',content:'Canonical answer',status:'completed',reply:null}]:[]});
  try {
    await readyChat(app);const complete=references(app).props.onDocumentsChanged;
    await libraryChat(app);
    const work=app.chat().props.state.ask('What did we decide?','keyword');
    assert.equal(requests[0].owner.id,'saved-thread');assert.deepEqual([...requests[0].search.document_ids],[]);
    assert.equal(app.chat().props.state.sending,true);
    nodes(app.chat().props.contextBar).find(node=>node.type==='popover').props.onOpenChange(false);app.chat();
    const readsBefore=documentReads;complete('file','retry');await readyChat(app);
    assert.equal(documentReads,readsBefore+1,'late completion must refetch the active document list');
    assert.deepEqual(cancelled,[]);assert.equal(app.chat().props.state.sending,true);
    assert.equal(app.chat().props.state.controller.pendingRequest(),requests[0].request_id);
    answered=true;answer.resolve({});await work;
    assert.equal(app.chat().props.state.messages[0].id,'saved-answer');
    assert.equal(app.chat().props.state.sending,false);
  } finally {app.unmount();}
});

for(const scenario of [{action:'delete',otherScope:true},{action:'detach',otherScope:false}]) {
  test(`${scenario.otherScope?'other-scope delete':'same-scope detach'} completion deselects the active file and cancels its answer`,async()=>{
    const answer=deferred(),cancelled=[],requests=[];let documentReads=0,historyReads=0;
    const app=cornerChat({scopeDocuments:async()=>{documentReads++;return [referenceFile];},cancel:async id=>cancelled.push(id),ask:request=>{requests.push(request);return answer.promise;},history:async()=>{historyReads++;return [];}});
    try {
      await readyChat(app);const complete=references(app).props.onDocumentsChanged;
      if(scenario.otherScope)await libraryChat(app);
      app.chat().props.state.selectDocument('file',true);await readyChat(app);
      const work=app.chat().props.state.ask('Question with a reference?','keyword');
      assert.deepEqual([...requests[0].search.document_ids],['file']);
      const readsBefore=documentReads;complete('file',scenario.action);await readyChat(app);
      assert.deepEqual([...app.chat().props.state.selectedDocumentIds],[]);
      assert.deepEqual(cancelled,[requests[0].request_id]);assert.equal(app.chat().props.state.sending,false);
      assert.equal(documentReads,readsBefore+1,'evidence removal must still reload documents');
      const historyBefore=historyReads;answer.resolve({});await work;
      assert.equal(historyReads,historyBefore,'cancelled completion must not reconcile stale history');
    } finally {app.unmount();}
  });
}

for(const scenario of [
  {action:'retry',otherScope:false,selected:true,present:true,refreshes:1},
  {action:'detach',otherScope:true,selected:true,present:true,refreshes:1},
  {action:'delete',otherScope:true,selected:false,present:true,refreshes:1},
  ...['retry','detach','delete'].map(action=>({action,otherScope:true,selected:false,present:false,refreshes:0})),
]) {
  test(`${scenario.otherScope?'other':'same'}-scope ${scenario.action} completion with a ${scenario.present?(scenario.selected?'selected':'unselected'):'missing'} file uses the non-resetting path`,()=>{
    const app=cornerChat();let refreshes=0,reloads=0;
    app.state.refreshDocuments=()=>refreshes++;app.state.reloadDocuments=()=>reloads++;
    try {
      const complete=references(app).props.onDocumentsChanged;
      if(scenario.otherScope)nodes(app.chat().props.contextBar).find(node=>node.type==='button'&&text(node)==='Other saved meetings').props.onClick();
      app.state.selectedDocumentIds=scenario.selected?['file']:[];
      if(!scenario.present)app.state.documents=[];
      app.chat();complete('file',scenario.action);
      assert.equal(refreshes,scenario.refreshes);assert.equal(reloads,0);assert.deepEqual(app.selected,[]);
    } finally {app.unmount();}
  });
}

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
