import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,flush,deferred} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):typeof node==='string'||typeof node==='number'?String(node):node?text(node.props?.children):'';
const root={driveId:'drive',itemId:'root',name:'Documents',webUrl:'https://example.com/documents',source:'drive'};
const folder={...root,itemId:'folder',name:'Meeting notes',webUrl:'https://example.com/documents/notes'};

function panel(name,service={},options={}) {
  const hooks=createHookHarness(),saved=[];
  const {IntegrationsSettings}=loadTsModule('src/components/IntegrationsSettings.tsx',{
    react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},'lucide-react':{},
    '@tauri-apps/api/core':{invoke:async()=>{}},sonner:{toast:{}},
    '@/components/UnencryptedHttpOptIn':{UnencryptedHttpOptIn:'http-option'},
    '@/components/IntegrationIcons':Object.fromEntries(['Microsoft365','OneNote','OneDrive','Planner','ToDo','OutlookCalendar'].map(name=>[`${name}Icon`,`${name}-icon`])),
    '@/components/ui/button':{Button:'button'},'@/components/ui/input':{Input:'input'},
    '@/components/ui/dialog':{Dialog:'dialog',DialogTrigger:'trigger',DialogContent:'content',DialogHeader:'header',DialogTitle:'title',DialogDescription:'description',DialogFooter:'footer'},
    '@/components/ui/checkbox':{},'@/components/ui/switch':{Switch:'switch'},'@/components/ui/select':{Select:'select'},'@/components/ui/accordion':{},
    '@/hooks/useMicrosoftExport':{useMicrosoftExport:()=>({connection:{state:'connected',userDisplayName:'Example user',userEmail:'user@example.com',...options.connection},signIn:()=>{}})},
    '@/lib/exportDestinations':{getExportDestinations:()=>options.destinations??{},setExportDestinations:value=>saved.push(value)},
    '@/services/microsoftExportService':{microsoftExportService:{listOneDriveDestinations:async()=>[root],...service}},
    '@/services/teamsDetectionService':{},'@/services/confluenceExportService':{confluenceExportService:options.confluence},'@/lib/meetingCalendar':{},
    '@/lib/autoRecord':{
      CALL_PROVIDERS:{teams:'Microsoft Teams',zoom:'Zoom',webex:'Webex',googleMeet:'Google Meet'},
      getCallSettings:()=>({mode:options.teamsMode??'off',providers:['teams'],autoStop:false}),
      setCallSettings:()=>{},
    },
  });
  const settingsTree=IntegrationsSettings();
  const component=nodes(settingsTree).find(node=>node.type?.name===name);
  assert.ok(component,`Missing panel ${name}`);
  return {...hooks,saved,settingsTree,render:()=>hooks.render(()=>component.type(component.props))};
}

test('OneDrive keeps folder management in its dialog and preserves resolve, create and root selection',async()=>{
  const calls=[];
  const app=panel('OneDrivePanel',{
    resolveOneDriveDestinationUrl:async url=>{calls.push(['resolve',url]);return folder;},
    createOneDriveDestinationFolder:async(parent,name)=>{calls.push(['create',parent.itemId,name]);return {...folder,name};},
  });
  app.render();await flush();let tree=app.render();
  assert.equal(tree.props.title,'OneDrive');
  let dialog=nodes(tree).find(node=>node.type==='dialog');assert.ok(dialog);assert.equal(dialog.props.open,false);
  assert.match(text(dialog),/Resolve folder/);assert.match(text(dialog),/Use the top-level folder/);
  dialog.props.onOpenChange(true);tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='dialog').props.open,true);
  nodes(tree).find(node=>node.type==='input'&&node.props.type==='url').props.onChange({target:{value:'https://example.com/shared'}});
  await nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Resolve folder').props.onClick();
  tree=app.render();assert.match(text(tree),/Meeting notes/);assert.match(text(nodes(tree).find(node=>node.type==='dialog')),/https:\/\/example.com\/documents\/notes/);
  nodes(tree).find(node=>node.type==='input'&&node.props.maxLength===120).props.onChange({target:{value:'Planning'}});
  await nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Create folder').props.onClick();
  await nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Use the top-level folder').props.onClick();
  assert.deepEqual(calls,[['resolve','https://example.com/shared'],['create','folder','Planning']]);
  assert.equal(app.saved.at(-1).oneDriveDestination.itemId,'root');app.unmount();
});

test('folder resolution remains disabled while pending and shows failures inside the dialog',async()=>{
  const request=deferred();
  const app=panel('OneDrivePanel',{resolveOneDriveDestinationUrl:()=>request.promise});
  app.render();await flush();let tree=app.render();
  const dialog=nodes(tree).find(node=>node.type==='dialog');assert.ok(dialog);dialog.props.onOpenChange(true);
  nodes(tree).find(node=>node.type==='input'&&node.props.type==='url').props.onChange({target:{value:'https://example.com/shared'}});
  const pending=nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Resolve folder').props.onClick();
  tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='button'&&text(node).includes('Resolve folder')).props.disabled,true);
  request.reject(new Error('Folder access failed. Choose another folder.'));await pending;await flush();
  tree=app.render();assert.match(text(nodes(tree).find(node=>node.type==='dialog')),/Folder access failed/);app.unmount();
});

test('Microsoft account uses one Microsoft 365 card without a ready badge',()=>{
  const app=panel('MicrosoftSignInPanel');const tree=app.render();
  assert.equal(tree.props.title,'Microsoft 365');assert.equal(tree.props.showBadge,false);
  assert.match(text(tree),/Example user/);assert.match(text(tree),/user@example.com/);app.unmount();
});

test('signed-out Microsoft features share one quiet availability group and keep sign-in actionable',()=>{
  const app=panel('MicrosoftSignInPanel',{}, {connection:{state:'not_connected'}});
  const tree=app.render();
  const group=nodes(tree).find(node=>node.type==='ul');
  assert.ok(group,'Microsoft features should remain discoverable before sign-in');
  assert.deepEqual(nodes(group).filter(node=>node.type==='li').map(text),['OneNote','OneDrive','Planner','To Do','Calendar']);
  assert.equal(nodes(group).filter(node=>String(node.type).endsWith('-icon')).length,5);
  assert.equal((text(tree).match(/Available after you sign in/g)??[]).length,1);
  assert.ok(nodes(tree).find(node=>node.type==='button'&&text(node)==='Sign in with Microsoft'));
  assert.ok(!nodes(app.settingsTree).some(node=>['OneNotePanel','OneDrivePanel','PlannerPanel','ToDoPanel','CalendarPanel'].includes(node.type?.name)));
  assert.equal(tree.props.showBadge,false);app.unmount();
});

test('Confluence test success persists until the next configuration change',async()=>{
  const status={tokenConfigured:true,allowUnencrypted:false,reachable:false,userDisplayName:null,message:'Connection has not been tested.'};
  const app=panel('ConfluencePanel',{}, {
    destinations:{confluenceMode:'rest',confluenceBaseUrl:'https://confluence.example.com',confluenceSpaceKey:'TEAM'},
    confluence:{settingsStatus:async()=>status,connectionStatus:async()=>({...status,reachable:true,message:'Provider connection details'})},
  });
  app.render();await flush();
  assert.doesNotMatch(text(app.render()),/Connection works/);
  for(const [findInput,value] of [
    [node=>node.props.type==='url','https://confluence.example.com/wiki'],
    [node=>node.props.placeholder==='TEAM','NOTES'],
    [node=>node.props.placeholder==='123456789','123'],
    [node=>node.props.type==='password','fixture-token'],
  ]) {
    await nodes(app.render()).find(node=>node.type==='button'&&text(node)==='Test connection').props.onClick();
    let tree=app.render();
    assert.match(text(tree),/Connection works/);
    assert.doesNotMatch(text(tree),/Provider connection details/);
    assert.equal(tree.props.showBadge,false);
    tree=app.render();assert.match(text(tree),/Connection works/,'success should survive an ordinary rerender');
    nodes(tree).find(node=>node.type==='input'&&findInput(node)).props.onChange({target:{value}});
    app.render();await flush();assert.doesNotMatch(text(app.render()),/Connection works/);
  }
  app.unmount();
});

test('Teams automation modes do not add filled status pills',()=>{
  for(const teamsMode of ['off','prompt','auto']) {
    const app=panel('TeamsAutoStartPanel',{}, {teamsMode});app.render();
    assert.equal(app.render().props.showBadge,false);app.unmount();
  }
});

test('a failed top-level folder read stays actionable and permits resolving another folder',async()=>{
  let reads=0;
  const app=panel('OneDrivePanel',{
    listOneDriveDestinations:async()=>{reads++;throw new Error('Could not load the top-level folder. Try another folder.');},
    resolveOneDriveDestinationUrl:async()=>folder,
  });
  app.render();app.render();await flush();app.render();await flush();const tree=app.render();
  assert.equal(reads,1,'a failed read must not restart itself on every state update');
  assert.equal(nodes(tree).find(node=>node.type==='input'&&node.props.type==='url').props.disabled,false);
  assert.match(text(tree),/Could not load the top-level folder/);app.unmount();
});
