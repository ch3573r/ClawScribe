import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,deferred,flush} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):typeof node==='string'||typeof node==='number'?String(node):node?text(node.props?.children):'';
const primitives={
  'react/jsx-runtime':{jsx,jsxs:jsx},'lucide-react':{},
  '@/components/ui/button':{Button:'button'},
  '@/components/ui/dialog':{Dialog:'dialog',DialogContent:'content',DialogTitle:'title',DialogDescription:'description',DialogHeader:'header',DialogFooter:'footer'},
  '@/components/ui/scroll-area':{ScrollArea:'scroll-area'},'@/components/ui/alert':{},
};

test('shared delete dialog focuses Cancel and routes dismissal without confirming',()=>{
  const hooks=createHookHarness(),calls=[];
  const {ConfirmationModal}=loadTsModule('src/components/ConfirmationModel/confirmation-modal.tsx',{...primitives,react:hooks.react});
  const tree=hooks.render(()=>ConfirmationModal({isOpen:true,text:'Delete example meeting?',onConfirm:()=>calls.push('delete'),onCancel:()=>calls.push('cancel')}));
  const dialog=nodes(tree).find(node=>node.type==='dialog');assert.ok(dialog);
  const cancel=nodes(tree).find(node=>node.type==='button'&&text(node)==='Cancel');
  const confirm=nodes(tree).find(node=>node.type==='button'&&text(node)==='Delete');
  assert.equal(confirm.props.variant,'destructive');
  cancel.props.ref.current={focus:()=>calls.push('focus cancel')};
  nodes(tree).find(node=>node.type==='content').props.onOpenAutoFocus({preventDefault:()=>{}});
  dialog.props.onOpenChange(false);assert.deepEqual(calls,['focus cancel','cancel']);
  confirm.props.onClick();assert.deepEqual(calls,['focus cancel','cancel','delete']);hooks.unmount();
});

test('shared confirmation returns focus to the explicit trigger, not the closed menu item',()=>{
  const hooks=createHookHarness(),focused=[];
  const {ConfirmationModal}=loadTsModule('src/components/ConfirmationModel/confirmation-modal.tsx',{...primitives,react:hooks.react});
  const trigger={isConnected:true,focus:()=>focused.push('trigger')};
  const menuItem={isConnected:false,focus:()=>focused.push('closed menu item')};
  const previous=globalThis.document;
  globalThis.document={activeElement:menuItem};
  try {
    for (const closeBy of ['escape','cancel','confirm']) {
      focused.length=0;
      const calls=[];
      const props={isOpen:true,title:'Clear conversation?',confirmLabel:'Clear conversation',text:'Remove everything?',returnFocus:{current:trigger},onConfirm:()=>calls.push('confirm'),onCancel:()=>calls.push('cancel')};
      const tree=hooks.render(()=>ConfirmationModal(props));
      const content=nodes(tree).find(node=>node.type==='content');
      nodes(tree).find(node=>node.type==='button'&&text(node)==='Cancel').props.ref.current={focus:()=>focused.push('cancel')};
      content.props.onOpenAutoFocus({preventDefault:()=>{}});
      assert.deepEqual(focused,['cancel']);
      if (closeBy==='escape') nodes(tree).find(node=>node.type==='dialog').props.onOpenChange(false);
      if (closeBy==='cancel') nodes(tree).find(node=>node.type==='button'&&text(node)==='Cancel').props.onClick();
      if (closeBy==='confirm') nodes(tree).find(node=>text(node)==='Clear conversation'&&node.type==='button').props.onClick();
      assert.deepEqual(calls,[closeBy==='confirm'?'confirm':'cancel']);
      let prevented=false;
      content.props.onCloseAutoFocus({preventDefault:()=>{prevented=true;}});
      assert.equal(prevented,true);
      assert.deepEqual(focused,['cancel','trigger']);
    }
  } finally { globalThis.document=previous; hooks.unmount(); }
});

test('shared confirmation disables the destructive action while the surface is busy',()=>{
  const hooks=createHookHarness();
  const {ConfirmationModal}=loadTsModule('src/components/ConfirmationModel/confirmation-modal.tsx',{...primitives,react:hooks.react});
  const tree=hooks.render(()=>ConfirmationModal({isOpen:true,text:'Remove?',confirmDisabled:true,onConfirm:()=>{},onCancel:()=>{}}));
  assert.equal(nodes(tree).find(node=>node.type==='button'&&node.props.variant==='destructive').props.disabled,true);
  assert.notEqual(nodes(tree).find(node=>node.type==='button'&&text(node)==='Cancel').props.disabled,true);
  hooks.unmount();
});

test('recovery deletion waits for explicit confirmation and keeps failures actionable',async context=>{
  const hooks=createHookHarness(),calls=[],errors=[],request=deferred();
  const diagnostic=context.mock.method(console,'error',()=>{});
  const {TranscriptRecovery}=loadTsModule('src/components/TranscriptRecovery/TranscriptRecovery.tsx',{
    ...primitives,react:hooks.react,sonner:{toast:{error:message=>errors.push(message)}},
  });
  const props={isOpen:true,onClose:()=>{},recoverableMeetings:[{meetingId:'a',title:'Example meeting',lastUpdated:Date.now(),startTime:Date.now(),transcriptCount:1}],onLoadPreview:async()=>[],onRecover:async()=>{},onDelete:id=>{calls.push(id);return request.promise;}};
  const render=()=>hooks.render(()=>TranscriptRecovery(props));
  render();await flush();let tree=render();
  const deleteButton=nodes(tree).find(node=>node.type==='button'&&text(node)==='Delete');
  deleteButton.props.ref.current={focus:()=>calls.push('focus delete')};
  deleteButton.props.onClick();
  assert.deepEqual(calls,[]);
  tree=render();let dialog=nodes(tree).filter(node=>node.type==='dialog').at(-1);assert.equal(dialog.props.open,true);
  nodes(dialog).find(node=>node.type==='button'&&text(node)==='Cancel').props.onClick();
  assert.deepEqual(calls,[]);assert.equal(nodes(render()).filter(node=>node.type==='dialog').at(-1).props.open,false);
  nodes(dialog).find(node=>node.type==='content').props.onCloseAutoFocus({preventDefault(){}});
  assert.deepEqual(calls,['focus delete']);calls.length=0;
  nodes(render()).find(node=>node.type==='button'&&text(node)==='Delete').props.onClick();
  dialog=nodes(render()).filter(node=>node.type==='dialog').at(-1);
  const pending=nodes(dialog).find(node=>node.type==='button'&&node.props.variant==='destructive').props.onClick();
  assert.deepEqual(calls,['a']);
  const failure=new Error('Fixture failure');
  request.reject(failure);await pending;await flush();
  assert.match(errors[0],/delete.*try again/i);
  assert.equal(diagnostic.mock.calls.at(-1).arguments[1],failure);
  assert.equal(nodes(render()).filter(node=>node.type==='dialog').at(-1).props.open,true);hooks.unmount();
});

test('meeting recovery failures preserve diagnostics and leave recovery available',async context=>{
  const hooks=createHookHarness(),errors=[],failure=new Error('Fixture recovery failure');
  const diagnostic=context.mock.method(console,'error',()=>{});
  const {TranscriptRecovery}=loadTsModule('src/components/TranscriptRecovery/TranscriptRecovery.tsx',{
    ...primitives,react:hooks.react,sonner:{toast:{error:message=>errors.push(message)}},
  });
  let closed=false;
  const props={isOpen:true,onClose:()=>{closed=true;},recoverableMeetings:[{meetingId:'a',title:'Example meeting',lastUpdated:Date.now(),startTime:Date.now(),transcriptCount:1}],onLoadPreview:async()=>[],onRecover:async()=>{throw failure;},onDelete:async()=>{}};
  const render=()=>hooks.render(()=>TranscriptRecovery(props));
  render();await flush();
  await nodes(render()).find(node=>node.type==='button'&&text(node)==='Recover').props.onClick();
  assert.equal(closed,false);
  assert.match(errors[0],/recovery data is still available.*try again/i);
  assert.equal(diagnostic.mock.calls.at(-1).arguments[1],failure);
  assert.equal(nodes(render()).find(node=>node.type==='button'&&text(node)==='Recover').props.disabled,false);hooks.unmount();
});

test('permission settings errors give platform-neutral recovery instructions',async()=>{
  const hooks=createHookHarness(),errors=[];
  const {PermissionsStep}=loadTsModule('src/components/onboarding/steps/PermissionsStep.tsx',{
    ...primitives,react:hooks.react,sonner:{toast:{error:message=>errors.push(message)}},
    '@tauri-apps/api/core':{invoke:async()=>{throw new Error('Fixture settings failure');}},
    '../OnboardingContainer':{OnboardingContainer:'onboarding'},'../shared':{PermissionRow:'permission'},
    '@/contexts/OnboardingContext':{useOnboarding:()=>({permissions:{microphone:'denied',systemAudio:'denied'}})},
  });
  const tree=hooks.render(() => PermissionsStep({ onFinish: async () => {}, finishing: false, error: null }));
  for(const row of nodes(tree).filter(node=>node.type==='permission')) await row.props.onAction();
  assert.equal(errors.length,2);assert.ok(errors.every(message=>/settings/i.test(message)&&/try again/i.test(message)));
  assert.ok(errors.every(message=>!/System Preferences|Audio Capture|console/.test(message)));hooks.unmount();
});
