import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):typeof node==='string'||typeof node==='number'?String(node):node?text(node.props?.children):'';
const meetings=[{id:'a',title:'Launch review'},{id:'b',title:'Budget planning'}];
function surface(file,name,props={}) {
  const hooks=createHookHarness();const exports=loadTsModule(file,{
    react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},'lucide-react':{},'next/navigation':{useRouter:()=>({push(){}})},
    '@/components/Sidebar/SidebarProvider':{useSidebar:()=>({meetings,projectTags:[],projectTagsLoading:false,projectTagsError:null})},
    '@/contexts/ConfigContext':{useConfig:()=>({modelConfig:{provider:'fixture',model:'fixture'}})},
    '@/components/LibraryBackup':{LibraryBackup:'backup'},'@/components/Knowledge/KnowledgeArchive':{KnowledgeArchive:'memory'},
    '@/components/ui/button':{Button:'button'},'@/components/ui/dropdown-menu':{},'@/components/ui/select':{},'@/components/ui/input-group':{},
    '@/components/ui/popover':{Popover:'popover',PopoverTrigger:'trigger',PopoverContent:'content'},
    '@/hooks/useKnowledgeSearch':{useKnowledgeSearch:scope=>({scope,messages:[],threads:[],cancel(){},search(){}})},
    './SearchResults':{SearchResults:'results'},'./KnowledgeChat':{KnowledgeChat:'chat'},'./EvidencePreview':{EvidencePreview:'preview'},
  });
  return {...hooks,render:()=>hooks.render(()=>exports[name](props))};
}
test('Meetings opens the archive and keeps Meeting memory mounted across tab changes',()=>{
  const app=surface('src/app/meetings/page.tsx','default');
  const panel=tree=>nodes(tree).find(node=>node.props?.id==='meeting-memory-panel');
  let tree=app.render();assert.equal(panel(tree)?.props.hidden,true);assert.equal(nodes(panel(tree)).filter(node=>node.type==='memory').length,1);
  nodes(tree).find(node=>node.props?.role==='tab'&&text(node)==='Meeting memory').props.onClick();
  tree=app.render();assert.equal(panel(tree).props.hidden,false);
  nodes(tree).find(node=>node.props?.role==='tab'&&text(node)==='Archive').props.onClick();
  assert.equal(nodes(panel(app.render())).filter(node=>node.type==='memory').length,1);app.unmount();
});
test('meeting picker searches titles and exposes selected chips without broadening the scope',()=>{
  const app=surface('src/components/Knowledge/KnowledgeArchive.tsx','KnowledgeArchive',{meetings,projectFilter:{tags:[],untagged:false,mode:'any'}});
  const search=nodes(app.render()).find(node=>node.props?.['aria-label']==='Find meetings');assert.ok(search);
  search.props.onChange({target:{value:'Budget'}});let tree=app.render();
  const labels=nodes(tree).filter(node=>node.type==='label');assert.equal(labels.some(node=>text(node)==='Launch review'),false);
  const budget=labels.find(node=>text(node)==='Budget planning');assert.ok(budget);nodes(budget).find(node=>node.type==='input').props.onChange({target:{checked:true}});
  tree=app.render();assert.ok(nodes(tree).some(node=>node.props?.['aria-label']==='Remove Budget planning'));
  const state=nodes(tree).find(node=>node.type==='chat').props.state;assert.deepEqual([...state.scope.filter.meeting_ids],['b']);assert.equal(state.scope.filter.all_meetings,false);app.unmount();
});
test('configured library chat enables the first question before a conversation exists',()=>{
  const state={scope:{kind:'library',filter:{all_meetings:false,meeting_ids:['a'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}},owner:null,messages:[],threads:[],historyLoading:false,sending:false,creating:false};
  const app=surface('src/components/Knowledge/KnowledgeChat.tsx','KnowledgeChat',{state,mode:'keyword',provider:'fixture',model:'fixture'});
  nodes(app.render()).find(node=>node.type==='textarea').props.onChange({target:{value:'Decision?'}});
  assert.equal(nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Ask').props.disabled,false);app.unmount();
});
