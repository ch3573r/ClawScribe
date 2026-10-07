import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,flush} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>!node||typeof node!=='object'?[]:Array.isArray(node)?node.flatMap(nodes):[node,...nodes(node.props?.children)];
const text=node=>typeof node==='string'||typeof node==='number'?String(node):Array.isArray(node)?node.map(text).join(''):node?text(node.props?.children):'';
const idle={stage:'idle',downloaded_bytes:0,total_bytes:487351240,current_file:null,error:null};
function settings(t,configuration,service={}) {
  const hooks=createHookHarness(); t.after(() => hooks.unmount());
  const {KnowledgeSettings}=loadTsModule('src/components/KnowledgeSettings.tsx',{
    react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},'@/components/ui/button':{Button:'button'},
    '@/services/knowledgeService':{knowledgeService:{indexStatus:async()=>({keyword_ready:true,semantic_ready:4,pending:0,failed:0}),modelStatus:async()=>configuration,...service}},
  });
  return {...hooks,render:()=>hooks.render(KnowledgeSettings)};
}
test('healthy E5 settings offer verification instead of a repair download',async(t)=>{
  const app=settings(t,{enabled:true,ready:true,installed:true,model:'intfloat/multilingual-e5-small',download:idle});
  app.render();await flush();const tree=app.render();
  assert(!nodes(tree).some(node=>node.type==='button'&&/Download|repair/.test(text(node))));
  assert(nodes(tree).some(node=>node.type==='button'&&/Verify installed files/.test(text(node))));
  assert.match(text(tree),/CPU.*ONNX/);app.unmount();
});
test('revisited settings show native transfer bytes and cancellation independently of local action state',async(t)=>{
  let cancels=0;const app=settings(t,{enabled:true,ready:false,installed:false,model:'intfloat/multilingual-e5-small',download:{...idle,stage:'downloading',downloaded_bytes:243675620,current_file:'onnx/model.onnx'}},{cancelDownload:async(t)=>{cancels++;}});
  app.render();await flush();const tree=app.render();
  const progress=nodes(tree).find(node=>node.type==='progress'&&node.props['aria-label']==='Model download progress');
  assert.equal(progress?.props.value,243675620);assert.equal(progress?.props.max,487351240);
  assert.match(text(tree),/50%/);assert.match(text(tree),/onnx\/model.onnx/);
  await nodes(tree).find(node=>node.type==='button'&&text(node)==='Cancel download').props.onClick();
  assert.equal(cancels,1);app.unmount();
});
test('integrity verification never displays a completed transfer percentage',async(t)=>{
  const app=settings(t,{enabled:true,ready:false,installed:false,model:'intfloat/multilingual-e5-small',download:{...idle,stage:'verifying',downloaded_bytes:487351240,current_file:'tokenizer.json'}});
  app.render();await flush();const tree=app.render();assert.match(text(tree),/Verifying/);assert.doesNotMatch(text(tree),/100%/);app.unmount();
});
test('failed native download survives a new settings view and offers a retry',async(t)=>{
  const app=settings(t,{enabled:true,ready:false,installed:false,model:'intfloat/multilingual-e5-small',download:{...idle,stage:'error',downloaded_bytes:12000,error:'Local embedding model is unavailable'}});
  app.render();await flush();const tree=app.render();
  assert(nodes(tree).some(node=>node.props?.role==='alert'&&/Retry/.test(text(node))));
  assert(nodes(tree).some(node=>node.type==='button'&&/Retry download/.test(text(node))));app.unmount();
});
test('status refresh moves the transfer through verification to readiness',async(t)=>{
  let configuration={enabled:true,ready:false,installed:false,model:'intfloat/multilingual-e5-small',download:{...idle,stage:'downloading',downloaded_bytes:243675620,current_file:'onnx/model.onnx'}};
  const app=settings(t,configuration,{modelStatus:async()=>configuration});
  app.render();await flush();assert.match(text(app.render()),/50%/);
  configuration={...configuration,download:{...configuration.download,stage:'verifying',downloaded_bytes:487351240}};
  nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Refresh status').props.onClick();await flush();
  assert.match(text(app.render()),/Verifying/);assert.doesNotMatch(text(app.render()),/100%/);
  configuration={...configuration,ready:true,installed:true,download:{...configuration.download,stage:'ready',current_file:null}};
  nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Refresh status').props.onClick();await flush();
  assert.match(text(app.render()),/Ready for local semantic retrieval/);
  assert(!nodes(app.render()).some(node=>node.type==='button'&&text(node)==='Cancel download'));
});
