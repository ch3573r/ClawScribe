import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {flush} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>!node||typeof node!=='object'?[]:Array.isArray(node)?node.flatMap(nodes):[node,...nodes(node.props?.children)];
const text=node=>typeof node==='string'||typeof node==='number'?String(node):Array.isArray(node)?node.map(text).join(''):node?text(node.props?.children):'';
function preview({offset=14,contextStatus='current'}={}){
  const calls=[];const reference={source_id:'meeting:target',locator:{kind:'transcript',meeting_id:'target'}};
  const metadata={title:'Original fixture',date:'2026-10-01',speaker:null,metadata_truncated:true};
  const resolved={status:'current',navigation:{meeting_id:'target',transcript_id:'row',transcript_index:205,start_seconds:offset},passage:{title:'Current fixture',date:'2026-10-02',text:'Public invented excerpt',metadata_truncated:false}};
  const state={preview:{reference,metadata,resolved},previewBusy:false,error:'',controller:{ticket:()=>()=>true},closePreview:()=>calls.push('preview closed'),inspect:async(...args)=>calls.push(['inspect',...args])};
  const {EvidencePreview}=loadTsModule('src/components/Knowledge/EvidencePreview.tsx',{'react/jsx-runtime':{jsx,jsxs:jsx},'@/components/ui/button':{Button:'button'},'@/components/ui/dialog':{Dialog:'dialog',DialogContent:'content',DialogDescription:'description',DialogHeader:'header',DialogTitle:'title'},'next/navigation':{useRouter:()=>({push:url=>calls.push(['route',url])})},'@/components/Sidebar/SidebarProvider':{useSidebar:()=>({setCurrentMeeting:meeting=>calls.push(['meeting',meeting])})},'@/services/knowledgeService':{knowledgeService:{resolve:async ref=>{calls.push(['resolve',ref]);return ref===reference?resolved:{status:contextStatus};}}}});
  const render=()=>EvidencePreview({state,onNavigate:()=>calls.push('chat closed')});
  return {state,calls,reference,render,button:name=>nodes(render()).find(node=>node.type==='button'&&text(node)===name)};
}
test('source preview distinguishes original/current metadata and closes chat after a verified route',async()=>{
  const app=preview();assert.match(text(app.render()),/Original fixture/);assert.match(text(app.render()),/Current fixture/);assert.match(text(app.render()),/Source labels were shortened/);
  app.button('Show in transcript').props.onClick();await flush();
  assert.deepEqual(app.calls.filter(item=>typeof item==='string'),['chat closed','preview closed']);
  const route=app.calls.find(item=>Array.isArray(item)&&item[0]==='route')[1];assert.match(route,/^\/meeting-details\?id=target&evidence=[a-f0-9-]+$/);assert.equal(route.includes('excerpt'),false);
});
test('context actions recheck the originating source and never route stale support',async()=>{
  const app=preview({contextStatus:'stale'});app.state.preview.contextReference={source_id:'meeting:origin'};
  app.button('Show in transcript').props.onClick();await flush();
  assert.equal(app.calls.some(item=>Array.isArray(item)&&item[0]==='route'),false);
  assert.equal(app.calls.find(item=>Array.isArray(item)&&item[0]==='inspect')[3],app.state.preview.contextReference);
});
test('unknown recording offsets disable playback while preserving transcript reveal',()=>{
  const app=preview({offset:null});assert.equal(app.button('Play from here').props.disabled,true);assert.equal(app.button('Show in transcript').props.disabled,false);
});
