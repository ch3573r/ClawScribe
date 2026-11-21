import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,flush} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):typeof node==='string'||typeof node==='number'?String(node):node?text(node.props?.children):'';
const meetings=[{id:'a',title:'Launch review'},{id:'b',title:'Budget planning'}];
function surface(file,name,props={},indexStatus={semantic_enabled:false,semantic_ready:0},sidebar={}) {
  const hooks=createHookHarness();const exports=loadTsModule(file,{
    react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},'lucide-react':{},'next/navigation':{useRouter:()=>({push:sidebar.push ?? (()=>{})})},
    'next/link':'link',
    '@/components/ConfirmationModel/confirmation-modal':{ConfirmationModal:()=>null},
    '@/components/Sidebar/SidebarProvider':{useSidebar:()=>({meetings,projectTags:[],projectTagsLoading:false,projectTagsError:null,...sidebar})},
    '@/contexts/ImportDialogContext':{useImportDialog:()=>({openImportDialog:sidebar.openImportDialog})},
    '@/contexts/RecordingStateContext':{useRecordingState:()=>({})},
    '@/contexts/ConfigContext':{useConfig:()=>({modelConfig:{provider:'fixture',model:'fixture'}})},
    '@/components/LibraryBackup':{LibraryBackup:'backup'},'@/components/Knowledge/KnowledgeArchive':{KnowledgeArchive:'memory'},
    '@/components/Archive/ArchiveMeetingRows':{ArchiveMeetingRows:'archive-rows'},'@/components/Archive/ArchiveStorageButton':{ArchiveStorageButton:'storage'},
    '@/components/MeetingDetails/ProjectTags':{ProjectTags:'project-tags'},
    './ArchiveAudioRemoval':{ArchiveAudioRemoval:'audio-removal'},
    '@/components/MeetingDetails/ReferenceDocuments':{ReferenceDocuments:'references'},
    '@/components/MeetingDetails/AttachReferenceButton':{AttachReferenceButton:'attach'},
    '@/components/ui/button':{Button:'button'},'@/components/ui/dropdown-menu':{},'@/components/ui/select':{},'@/components/ui/input-group':{InputGroupInput:'title-filter'},
    '@/components/ui/textarea':{Textarea:'textarea'},'@/components/ui/scroll-area':{ScrollArea:'scroll-area'},'@/components/ui/tooltip':{},
    '@/components/ui/input':{Input:'input'},'@/components/ui/checkbox':{Checkbox:'checkbox'},'@/components/ui/switch':{Switch:'switch'},
    '@/components/ui/tabs':{Tabs:'tabs',TabsList:'tablist',TabsTrigger:'tab',TabsContent:'tab-panel'},
    '@/services/knowledgeService':{knowledgeService:{indexStatus:async()=>indexStatus,history:async()=>[]}},
    '@/components/ui/popover':{Popover:'popover',PopoverTrigger:'trigger',PopoverContent:'content'},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:scope=>({scope,messages:[],threads:[],cancel(){},search(){}})},
    '@/hooks/useLibrarySearch':{useLibrarySearch:(query,active,scope)=>({results:query && active ? sidebar.search?.(query,scope) ?? {titles:[],passages:[],limited:false} : null,searching:false,error:''})},
    './SearchResults':{SearchResults:'results'},'./KnowledgeChat':{KnowledgeChat:'chat'},'./EvidencePreview':{EvidencePreview:'preview'},
  });
  return {...hooks,render:()=>hooks.render(()=>exports[name](props))};
}
test('Meetings opens the archive and keeps Meeting memory mounted across tab changes',()=>{
  const app=surface('src/app/meetings/page.tsx','default');
  const panel=tree=>nodes(tree).find(node=>node.props?.id==='meeting-memory-panel');
  let tree=app.render();assert.equal(panel(tree)?.props.hidden,true);assert.equal(nodes(panel(tree)).filter(node=>node.type==='memory').length,1);
  nodes(tree).find(node=>node.type==='tabs').props.onValueChange('memory');
  tree=app.render();assert.equal(panel(tree).props.hidden,false);
  nodes(tree).find(node=>node.type==='tabs').props.onValueChange('archive');
  assert.equal(nodes(panel(app.render())).filter(node=>node.type==='memory').length,1);app.unmount();
});
test('meeting picker searches titles and exposes selected chips without broadening the scope',()=>{
  const app=surface('src/components/Knowledge/KnowledgeArchive.tsx','KnowledgeArchive',{meetings,projectFilter:{tags:[],untagged:false,mode:'any'}});
  const search=nodes(app.render()).find(node=>node.props?.['aria-label']==='Find meetings');assert.ok(search);
  search.props.onChange({target:{value:'Budget'}});let tree=app.render();
  const labels=nodes(tree).filter(node=>node.type==='label');assert.equal(labels.some(node=>text(node)==='Launch review'),false);
  const budget=labels.find(node=>text(node)==='Budget planning');assert.ok(budget);nodes(budget).find(node=>node.type==='checkbox').props.onCheckedChange(true);
  tree=app.render();assert.ok(nodes(tree).some(node=>node.props?.['aria-label']==='Remove Budget planning'));
  const state=nodes(tree).find(node=>node.type==='chat').props.state;assert.deepEqual([...state.scope.filter.meeting_ids],['b']);assert.equal(state.scope.filter.all_meetings,false);app.unmount();
});
test('ready semantic indexing defaults chat to hybrid while disabled semantic search stays keyword',async()=>{
  for(const [ready,expected] of [[true,'hybrid'],[false,'keyword']]) {
    const app=surface('src/components/Knowledge/KnowledgeArchive.tsx','KnowledgeArchive',{meetings,projectFilter:{tags:[],untagged:false,mode:'any'}},{semantic_enabled:ready,semantic_ready:4});
    app.render();await flush();
    const chat=nodes(app.render()).find(node=>node.type==='chat');
    assert.equal(chat.props.mode,expected);
    assert(!nodes(chat.props.secondary).some(node=>node.type==='results'),'collapsed search has no empty passage panel');
    app.unmount();
  }
});
test('configured library chat enables the first question before a conversation exists',()=>{
  const state={scope:{kind:'library',filter:{all_meetings:false,meeting_ids:['a'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}},owner:null,messages:[],threads:[],historyLoading:false,sending:false,creating:false};
  const app=surface('src/components/Knowledge/KnowledgeChat.tsx','KnowledgeChat',{state,mode:'keyword',provider:'fixture',model:'fixture'});
  nodes(app.render()).find(node=>node.type==='textarea').props.onChange({target:{value:'Decision?'}});
  assert.equal(nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Ask').props.disabled,false);app.unmount();
});

test('archive empty state starts recording and imports through the shared flows',()=>{
  const calls=[];
  const app=surface('src/app/meetings/page.tsx','default',{},undefined,{meetings:[],handleRecordingToggle:()=>calls.push('record'),openImportDialog:()=>calls.push('import')});
  const tree=app.render();
  assert.match(text(tree),/No meetings yet/);
  assert(!nodes(tree).some(node=>node.type==='title-filter'));
  nodes(tree).find(node=>node.type==='button'&&text(node)==='Start recording').props.onClick();
  nodes(tree).find(node=>node.type==='button'&&text(node)==='Import audio or video').props.onClick();
  assert.deepEqual(calls,['record','import']);app.unmount();
});

test('archive filtering keeps controls and clear filters restores all rows',()=>{
  const app=surface('src/app/meetings/page.tsx','default');
  let tree=app.render();assert.match(text(tree),/2 meetings/);
  assert.deepEqual([...nodes(tree).find(node=>node.type==='archive-rows').props.meetings.map(meeting=>meeting.id)],['b','a']);
  nodes(tree).find(node=>node.type==='title-filter').props.onChange({target:{value:'missing'}});
  tree=app.render();assert.match(text(tree),/No meetings match/);
  nodes(tree).find(node=>node.type==='button'&&text(node)==='Clear filters').props.onClick();
  tree=app.render();assert.match(text(tree),/2 meetings/);
  assert.equal(nodes(tree).find(node=>node.type==='archive-rows').props.meetings.length,2);app.unmount();
});

test('archive shows titles and passages and restores date and query when returning',()=>{
  const observed=[], navigated=[];
  const evidence={locator:{kind:'transcript',meeting_id:'b'},chunk_id:'late-segment'};
  const sidebar={
    setCurrentMeeting(){},
    push:url=>navigated.push(url),
    meetings:[{...meetings[0],created_at:'2026-01-01'},{...meetings[1],created_at:'2026-02-01'}],
    search:(query,scope)=>{
      observed.push([query,scope?.filter.from]);
      return {titles:[{id:'a'}],passages:[{meeting_id:'b',title:'Budget planning',text:'Fixture passage 50% 📘',speaker:'Fixture speaker',evidence}],limited:false};
    },
  };
  const app=surface('src/app/meetings/page.tsx','default',{},undefined,sidebar);
  let tree=app.render();
  nodes(tree).find(node=>node.type==='title-filter').props.onChange({target:{value:'50%'}});
  tree=app.render();
  assert.match(text(tree),/Matching titles/);
  assert.match(text(tree),/Fixture passage 50% 📘/);
  const matchingRows=nodes(tree).find(node=>node.type==='archive-rows').props;
  assert.deepEqual(Array.from(matchingRows.meetings,meeting=>meeting.id),['a']);
  const rows=surface('src/components/Archive/ArchiveMeetingRows.tsx','ArchiveMeetingRows',{...matchingRows,active:false});
  const titleLink=nodes(rows.render()).find(node=>node.type==='link'&&text(node)==='Launch review');
  assert.equal(titleLink.props.href,'/meeting-details?id=a&from=archive');
  rows.unmount();
  nodes(tree).find(node=>node.type==='input'&&node.props.type==='date').props.onChange({target:{value:'2026-02-01'}});
  tree=app.render();
  assert.doesNotMatch(text(tree),/Launch review/);
  nodes(tree).find(node=>node.props?.['aria-label']==='Show passage in Budget planning').props.onClick();
  assert.match(navigated[0],/^\/meeting-details\?id=b&evidence=[\w-]+&from=archive$/);
  app.reset();
  tree=app.render(); // Same module and a simulated route remount.
  assert.equal(nodes(tree).find(node=>node.type==='title-filter').props.value,'50%');
  assert.deepEqual(observed.at(-1),['50%','2026-02-01']);
  app.unmount();
});

test('memory empty-state action opens the existing chooser without expanding the scope',()=>{
  const app=surface('src/components/Knowledge/KnowledgeArchive.tsx','KnowledgeArchive',{meetings,projectFilter:{tags:[],untagged:false,mode:'any'}});
  let tree=app.render();const chat=nodes(tree).find(node=>node.type==='chat');
  assert.match(text(chat.props.emptyState),/Choose meetings to ask about/);
  nodes(chat.props.emptyState).find(node=>node.type==='button').props.onClick();
  tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='popover').props.open,true);
  assert.equal(nodes(tree).find(node=>node.type==='chat').props.state.scope.filter.all_meetings,false);app.unmount();
});

test('memory remembers its source selection and dates across route navigation',()=>{
  const app=surface('src/components/Knowledge/KnowledgeArchive.tsx','KnowledgeArchive',{meetings,projectFilter:{tags:[],untagged:false,mode:'any'}});
  let tree=app.render();
  nodes(tree).find(node=>node.type==='checkbox').props.onCheckedChange(true);
  nodes(tree).find(node=>node.props?.type==='date').props.onChange({target:{value:'2026-02-01'}});
  tree=app.render();
  const selected=nodes(tree).find(node=>node.type==='chat').props.state.scope.filter;
  assert.deepEqual([...selected.meeting_ids],[meetings[0].id]);
  app.reset();
  const restored=nodes(app.render()).find(node=>node.type==='chat').props.state.scope.filter;
  assert.deepEqual([...restored.meeting_ids],[meetings[0].id]);
  assert.equal(restored.from,'2026-02-01');
  assert.equal(restored.all_meetings,false);
  app.unmount();
});

test('memory lets the user draft a question before authorizing sources',()=>{
  const state={scope:{kind:'library',filter:{all_meetings:false,meeting_ids:[],tags:[],tag_mode:'any',untagged:false,from:null,to:null}},owner:null,messages:[],threads:[]};
  const app=surface('src/components/Knowledge/KnowledgeChat.tsx','KnowledgeChat',{state,mode:'keyword',provider:'fixture',model:'fixture',emptyState:jsx('p',{children:'Choose meetings'})});
  const tree=app.render();const field=nodes(tree).find(node=>node.type==='textarea');assert.ok(!field.props.disabled);
  field.props.onChange({target:{value:'Draft question'}});assert.equal(nodes(app.render()).find(node=>node.type==='textarea').props.value,'Draft question');
  assert.equal(nodes(tree).find(node=>node.type==='button'&&text(node)==='Ask').props.disabled,true);app.unmount();
});

test('transcript results omit empty panels and explain fallback without internal status codes',()=>{
  const state={loading:false,response:null};
  let app=surface('src/components/Knowledge/SearchResults.tsx','SearchResults',{state});
  assert.equal(app.render(),null);app.unmount();
  app=surface('src/components/Knowledge/SearchResults.tsx','SearchResults',{state:{...state,response:{passages:[],mode:'keyword',index_status:{reason:'model_unavailable'}}}});
  const copy=text(app.render());
  assert.match(copy,/search model is unavailable/);assert.match(copy,/Results use keyword search/);
  assert.doesNotMatch(copy,/model_unavailable/);app.unmount();
});
