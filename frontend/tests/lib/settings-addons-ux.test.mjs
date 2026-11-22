import assert from 'node:assert/strict';
import test from 'node:test';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness,flush,deferred} from './hook-harness.mjs';
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.actions),...nodes(node.props?.aside),...nodes(node.props?.title),...nodes(node.props?.description),...nodes(node.props?.children)]:[];
const text=node=>Array.isArray(node)?node.map(text).join(''):typeof node==='string'||typeof node==='number'?String(node):node?.props?text(node.props.title)+text(node.props.description)+text(node.props.children):'';
const root={driveId:'drive',itemId:'root',name:'Documents',webUrl:'https://example.com/documents',source:'drive'};
const folder={...root,itemId:'folder',name:'Meeting notes',webUrl:'https://example.com/documents/notes'};
const homePath=['C:','Users','someone','AppData','Roaming','ClawScribe'].join(String.fromCharCode(92));
const names=(list,suffix)=>Object.fromEntries(list.map(name=>[name,name]));
const ui=(path,list)=>({[`@/components/ui/${path}`]:names(list)});

function baseMocks(hooks,extra={}) {
  return {
    react:{...hooks.react,useSyncExternalStore:(_s,get)=>get()},'react/jsx-runtime':{jsx,jsxs:jsx},'lucide-react':{},
    '@tauri-apps/api/core':{invoke:async()=>{}},sonner:{toast:{error(){}}},
    '@/components/UnencryptedHttpOptIn':{UnencryptedHttpOptIn:'http-option'},
    '@/components/IntegrationIcons':Object.fromEntries(['Microsoft365','OneNote','OneDrive','Planner','ToDo','Teams'].map(name=>[`${name}Icon`,`${name}-icon`])),
    ...ui('button',['Button']),...ui('input',['Input']),...ui('switch',['Switch']),...ui('checkbox',['Checkbox']),
    ...ui('dialog',['Dialog','DialogTrigger','DialogContent','DialogHeader','DialogTitle','DialogDescription','DialogFooter']),
    ...ui('select',['Select','SelectContent','SelectItem','SelectTrigger','SelectValue']),
    ...ui('tabs',['Tabs','TabsList','TabsTrigger']),...ui('accordion',['Accordion','AccordionContent','AccordionItem','AccordionTrigger']),
    ...extra,
  };
}

function microsoft(name,service={},options={}) {
  const hooks=createHookHarness(),saved=[];
  const connection={state:'connected',userDisplayName:'Example user',userEmail:'user@example.com',...options.connection};
  const ms={connection,signIn:()=>{},signOut:()=>{},cancelSignIn:()=>{},requestMissingPermissions:()=>{},signingIn:false,notebooks:[],plans:[],buckets:[],todoLists:[],loadNotebooks:async()=>{},loadPlans:async()=>{},loadBuckets:async()=>{},loadToDoLists:async()=>{},...options.ms};
  const mod=loadTsModule('src/components/settings/Microsoft365Settings.tsx',baseMocks(hooks,{
    '@/hooks/useMicrosoftExport':{useMicrosoftExport:()=>ms},
    '@/lib/exportDestinations':{getExportDestinations:()=>options.destinations??{},setExportDestinations:value=>saved.push(value)},
    '@/services/microsoftExportService':{ONENOTE_LARGE_LIBRARY_MESSAGE:'large library',microsoftExportService:{listOneDriveDestinations:async()=>[root],...service}},
  }));
  const render=()=>hooks.render(()=>mod[name]({ms}));
  return {...hooks,saved,render};
}

test('OneDrive keeps folder management in its dialog and preserves resolve, create and root selection',async()=>{
  const calls=[];
  const app=microsoft('OneDriveCard',{
    resolveOneDriveDestinationUrl:async url=>{calls.push(['resolve',url]);return folder;},
    createOneDriveDestinationFolder:async(parent,name)=>{calls.push(['create',parent.itemId,name]);return {...folder,name};},
  });
  app.render();await flush();let tree=app.render();
  assert.equal(tree.props.title,'OneDrive');
  let dialog=nodes(tree).find(node=>node.type==='Dialog');assert.ok(dialog);assert.equal(dialog.props.open,false);
  assert.match(text(dialog),/Resolve folder/);assert.match(text(dialog),/Use the top-level folder/);
  dialog.props.onOpenChange(true);tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='Dialog').props.open,true);
  nodes(tree).find(node=>node.type==='Input'&&node.props.type==='url').props.onChange({target:{value:'https://example.com/shared'}});
  await nodes(app.render()).find(node=>node.type==='Button'&&text(node)==='Resolve folder').props.onClick();
  tree=app.render();assert.match(text(tree),/Meeting notes/);assert.match(text(nodes(tree).find(node=>node.type==='Dialog')),/https:\/\/example.com\/documents\/notes/);
  nodes(tree).find(node=>node.type==='Input'&&node.props.maxLength===120).props.onChange({target:{value:'Planning'}});
  await nodes(app.render()).find(node=>node.type==='Button'&&text(node)==='Create folder').props.onClick();
  await nodes(app.render()).find(node=>node.type==='Button'&&text(node)==='Use the top-level folder').props.onClick();
  assert.deepEqual(calls,[['resolve','https://example.com/shared'],['create','folder','Planning']]);
  assert.equal(app.saved.at(-1).oneDriveDestination.itemId,'root');app.unmount();
});

test('folder resolution remains disabled while pending and shows failures inside the dialog',async()=>{
  const request=deferred();
  const app=microsoft('OneDriveCard',{resolveOneDriveDestinationUrl:()=>request.promise});
  app.render();await flush();let tree=app.render();
  const dialog=nodes(tree).find(node=>node.type==='Dialog');assert.ok(dialog);dialog.props.onOpenChange(true);
  nodes(tree).find(node=>node.type==='Input'&&node.props.type==='url').props.onChange({target:{value:'https://example.com/shared'}});
  const pending=nodes(app.render()).find(node=>node.type==='Button'&&text(node)==='Resolve folder').props.onClick();
  tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='Button'&&text(node).includes('Resolve folder')).props.disabled,true);
  request.reject(new Error('Folder access failed. Choose another folder.'));await pending;await flush();
  tree=app.render();assert.match(text(nodes(tree).find(node=>node.type==='Dialog')),/Folder access failed/);app.unmount();
});

test('a failed top-level folder read stays actionable and permits resolving another folder',async()=>{
  let reads=0;
  const app=microsoft('OneDriveCard',{
    listOneDriveDestinations:async()=>{reads++;throw new Error('Could not load the top-level folder. Try another folder.');},
    resolveOneDriveDestinationUrl:async()=>folder,
  });
  app.render();app.render();await flush();app.render();await flush();const tree=app.render();
  assert.equal(reads,1,'a failed read must not restart itself on every state update');
  assert.equal(nodes(tree).find(node=>node.type==='Input'&&node.props.type==='url').props.disabled,false);
  assert.match(text(tree),/Could not load the top-level folder/);app.unmount();
});

test('the account row shows who is signed in with Connected and Sign out, and no filled badge',()=>{
  const app=microsoft('MicrosoftAccountCard');const tree=app.render();
  assert.match(text(tree),/Signed in as Example user/);assert.match(text(tree),/Connected/);
  assert.ok(nodes(tree).find(node=>node.type==='Button'&&text(node).includes('Sign out')));
  assert.equal(tree.props.showBadge,undefined);
  assert.ok(!nodes(tree).find(node=>node.type==='Button'&&text(node).includes('Sign in')));app.unmount();
});

test('signed out, the account row offers one sign-in action and says what it unlocks',()=>{
  const app=microsoft('MicrosoftAccountCard',{}, {connection:{state:'not_connected',userDisplayName:null,userEmail:null}});
  const tree=app.render();
  assert.match(text(tree),/Not signed in/);assert.match(text(tree),/Signed out/);
  assert.match(text(tree),/OneNote, OneDrive, Planner and To Do/);
  assert.equal(nodes(tree).filter(node=>node.type==='Button'&&text(node)==='Sign in with Microsoft').length,1);
  assert.ok(!nodes(tree).find(node=>node.type==='Button'&&text(node).includes('Sign out')));app.unmount();
});

test('the Microsoft 365 page shows the four destination cards only while connected',()=>{
  for(const [state,expected] of [['connected',['OneNoteCard','ToDoCard','OneDriveCard','PlannerCard']],['not_connected',[]]]) {
    const app=microsoft('Microsoft365Settings',{}, {connection:{state}});
    const found=nodes(app.render()).map(node=>node.type?.name).filter(name=>/Card$/.test(name??'')&&name!=='SettingsCard');
    assert.deepEqual(found.filter(name=>name!=='MicrosoftAccountCard'),expected);app.unmount();
  }
});

test('reload actions are labelled icon buttons in the card header',()=>{
  const app=microsoft('OneNoteCard');const tree=app.render();
  const action=nodes(tree.props.actions).find(node=>node.props?.label);
  assert.equal(action.props.label,'Reload notebooks');
  assert.equal(tree.props.description,'Each meeting gets a new dated section');app.unmount();
});

function confluence(options) {
  const hooks=createHookHarness();
  const {ConfluenceCard}=loadTsModule('src/components/settings/ConfluenceSettings.tsx',baseMocks(hooks,{
    '@/lib/exportDestinations':{getExportDestinations:()=>options.destinations??{},setExportDestinations:()=>{}},
    '@/services/confluenceExportService':{confluenceExportService:options.confluence},
  }));
  return {...hooks,render:()=>hooks.render(ConfluenceCard)};
}

test('Confluence test success persists until the next configuration change',async()=>{
  const status={tokenConfigured:true,allowUnencrypted:false,reachable:false,userDisplayName:null,message:'Connection has not been tested.'};
  const app=confluence({
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
    await nodes(app.render()).find(node=>node.type==='Button'&&text(node)==='Test connection').props.onClick();
    let tree=app.render();
    assert.match(text(tree),/Connection works/);
    assert.doesNotMatch(text(tree),/Provider connection details/);
    tree=app.render();assert.match(text(tree),/Connection works/,'success should survive an ordinary rerender');
    nodes(tree).find(node=>node.type==='Input'&&findInput(node)).props.onChange({target:{value}});
    app.render();await flush();assert.doesNotMatch(text(app.render()),/Connection works/);
  }
  app.unmount();
});

test('Confluence is one card of rows with both methods, no export-flow box and a disabled test without a base URL',async()=>{
  const status={tokenConfigured:false,allowUnencrypted:false,reachable:false,userDisplayName:null,message:'Connection has not been tested.'};
  const app=confluence({destinations:{confluenceMode:'draft'},confluence:{settingsStatus:async()=>status}});
  app.render();await flush();let tree=app.render();
  assert.doesNotMatch(text(tree),/Export flow/);
  assert.deepEqual(nodes(tree).filter(node=>node.type==='TabsTrigger').map(text),['Browser draft','Direct REST']);
  assert.ok(nodes(tree).find(node=>node.type==='Input'&&node.props.type==='url'),'draft mode asks for the create page URL');
  nodes(tree).find(node=>node.type==='Tabs').props.onValueChange('rest');app.render();
  tree=app.render();assert.equal(nodes(tree).find(node=>node.type==='Button'&&text(node)==='Test connection').props.disabled,true);
  assert.match(text(tree),/Connection has not been tested/);app.unmount();
});

function callCard(options={}) {
  const hooks=createHookHarness(),saved=[],core=[];
  const scans=options.scans??[];
  const {CallDetectionSettings}=loadTsModule('src/components/settings/CallDetectionSettings.tsx',baseMocks(hooks,{
    '@/lib/autoRecord':{
      CALL_PROVIDERS:{teams:'Microsoft Teams',zoom:'Zoom',webex:'Webex',googleMeet:'Google Meet'},
      getCallSettings:()=>({mode:options.mode??'off',providers:['teams'],autoStop:false}),
      setCallSettings:value=>saved.push(value),
    },
    '@/services/callDetectionService':{setCoreCallSettings:async value=>{if(options.coreFails) throw new Error('not saved');core.push(value);}},
    '@/services/teamsDetectionService':{getCallDetectionStatus:async providers=>{scans.push(providers);return options.scan??{supported:true,complete:true,active:[],ended:[],openWindows:[]};}},
  }));
  return {...hooks,saved,core,scans,render:()=>hooks.render(CallDetectionSettings)};
}

test('call detection saves settings, offers one check and reports a plain result with its time',async()=>{
  const app=callCard({mode:'prompt'});
  app.render();let tree=app.render();
  assert.equal(tree.props.title,'Call detection');
  assert.equal(nodes(tree).filter(node=>node.type==='Checkbox').length,4);
  nodes(tree).find(node=>node.type==='Select').props.onValueChange('auto');await flush();
  assert.equal(app.saved.at(-1).mode,'auto');
  assert.equal(app.core.at(-1).mode,'auto','the core, which watches for calls, gets the same settings');
  nodes(app.render()).find(node=>node.type==='Switch').props.onCheckedChange(true);await flush();
  assert.equal(app.saved.at(-1).autoStop,true);
  assert.equal(app.core.at(-1).autoStop,true);
  tree=app.render();assert.match(text(tree),/Not checked yet/);
  const check=()=>nodes(app.render()).find(node=>node.type==='Button'&&/Check now|Checking/.test(text(node)));
  await check().props.onClick();await flush();
  tree=app.render();assert.equal(JSON.stringify(app.scans),'[["teams"]]');
  assert.match(text(tree),/No call right now/);assert.match(text(tree),/Checked \d/);
  app.unmount();
});

test('call detection keeps the old settings when the core could not save the new ones',async()=>{
  const app=callCard({mode:'prompt',coreFails:true});
  app.render();const tree=app.render();
  nodes(tree).find(node=>node.type==='Select').props.onValueChange('auto');await flush();
  assert.deepEqual(app.saved,[],'nothing is kept that the core does not have');
  assert.deepEqual(app.core,[]);
  app.unmount();
});

test('call detection reports an active call by app and an incomplete scan as a problem',async()=>{
  const active=callCard({scan:{supported:true,complete:true,active:[{provider:'teams',window:'1-a',key:'k'}],ended:[],openWindows:['1-a']}});
  active.render();await nodes(active.render()).find(node=>node.type==='Button'&&/Check now/.test(text(node))).props.onClick();await flush();
  assert.match(text(active.render()),/Call active in Microsoft Teams/);active.unmount();
  const partial=callCard({scan:{supported:true,complete:false,active:[],ended:[],openWindows:[]}});
  partial.render();await nodes(partial.render()).find(node=>node.type==='Button'&&/Check now/.test(text(node))).props.onClick();await flush();
  assert.match(text(partial.render()),/Could not check all meeting windows/);partial.unmount();
});

function diagnostics(options={}) {
  const hooks=createHookHarness();
  const snapshot={
    app:{version:'0.5.60',os:'windows',arch:'x64',buildProfile:'release',appDataDir:homePath,logsDir:null},
    systemResources:null,acceleration:{label:'Vulkan GPU acceleration',useGpu:true,gpuAvailableButUnused:false,runtimeDetectedGpu:'GPU',cpuCores:8},
    transcription:{selectedProvider:'parakeet',selectedModel:'Parakeet Ultra',apiKeyConfigured:false,engines:[{provider:'parakeet',selectedModelPresent:true,downloadedModels:1,totalModels:5}]},
    audio:{inputDevices:3,outputDevices:5,totalDevices:8,deviceNames:[]},recording:{},
    diarization:{inProgress:false,directmlFast:true,directmlSlow:false,directmlUnavailable:false,preferredProvider:'directml',runtimeDlls:[{name:'a',present:true}]},
    integrations:{},errors:[],...options.snapshot,
  };
  const invoke=async name=>{
    if(name==='get_diagnostics_snapshot') return snapshot;
    if(name==='codex_check_installation') return {found:true,authStatus:'authenticated',message:'ok'};
  };
  const mod=loadTsModule('src/components/settings/DiagnosticsSettings.tsx',baseMocks(hooks,{
    '@tauri-apps/api/core':{invoke},
    '@/lib/autoRecord':{CALL_PROVIDERS:{teams:'Microsoft Teams'},CALL_SETTINGS_CHANGED:'changed',getCallSettings:()=>({mode:'off',providers:['teams'],autoStop:false})},
    '@/services/teamsDetectionService':{getCallDetectionStatus:async()=>({supported:true,complete:true,active:[],ended:[],openWindows:['1-a','2-b']})},
    '@/services/icalService':{getICalendarServerSnapshot:()=>({}),getICalendarSnapshot:()=>options.calendars??{sources:[],events:[],issues:{},loading:false,error:null},subscribeToICalendars:()=>()=>{},refreshICalendars:async()=>{}},
  }));
  return {...hooks,mod,render:()=>hooks.render(mod.DiagnosticsSettings)};
}

test('diagnostics lists problems first, keeps healthy rows quiet and shows the call check driven by the selected apps',async()=>{
  const app=diagnostics({calendars:{sources:[{id:'s',name:'Work'}],events:[],issues:{s:'Could not be refreshed'},loading:false,error:null}});
  app.render();await flush();app.render();await flush();
  const tree=app.render();
  const rows=nodes(tree).filter(node=>node.type?.name==='HealthRow').map(node=>node.props.item);
  assert.equal(rows[0].id,'calendars');assert.equal(rows[0].tone,'warning');
  assert.ok(rows.slice(1).every(row=>row.tone==='ok'),'only the calendar problem is coloured');
  assert.deepEqual(rows.map(row=>row.name).slice(0,2),['Calendars','Transcription backend']);
  assert.ok(rows.some(row=>row.name==='Codex app-server'));
  assert.match(text(tree),/ClawScribe 0\.5\.60 · windows x64 · release/);
  const call=nodes(tree).find(node=>node.type?.name==='CallDetectionCard');
  assert.ok(call);
  assert.doesNotMatch(text(tree),/Signals|Confidence/);
  app.unmount();
});

test('sorting puts errors, then warnings first and is stable',()=>{
  const app=diagnostics();
  const sorted=app.mod.sortHealthItems([{id:'a',tone:'ok'},{id:'b',tone:'warning'},{id:'c',tone:'error'},{id:'d',tone:'ok'},{id:'e',tone:'warning'}]);
  assert.deepEqual(sorted.map(item=>item.id),['c','b','e','a','d']);app.unmount();
});

test('copied diagnostics redact emails and home paths',()=>{
  const app=diagnostics();
  const copy=app.mod.sanitizeDiagnosticsForCopy({
    app:{appDataDir:homePath},transcription:{},audio:{deviceNames:[]},recording:{},diarization:{},
    integrations:{codex:{accountEmail:'person@example.com'}},errors:['failed for person@example.com'],
  });
  const json=JSON.stringify(copy);
  assert.doesNotMatch(json,/someone|person@example/);
  assert.match(json,/%APPDATA%/);app.unmount();
});

test('settings tabs keep deep links working and resolve the old names',()=>{
  const {resolveSettingsTab,SETTINGS_TABS}=loadTsModule('src/components/settings/settings-tabs.ts');
  for(const tab of SETTINGS_TABS) assert.equal(resolveSettingsTab(tab),tab);
  assert.equal(resolveSettingsTab('integrations'),'ms365');
  assert.equal(resolveSettingsTab('beta'),'transcription');
  assert.equal(resolveSettingsTab('nope'),null);
  assert.equal(resolveSettingsTab(null),null);
  assert.deepEqual([...SETTINGS_TABS],['general','privacy','appearance','recording','shortcuts','transcription','summary','memory','ms365','calendars','confluence','webhook','assistant','diagnostics']);
});
