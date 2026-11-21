import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function settings(overrides={}) {
  const snapshot={ready:true,channel:'stable',isChecking:false,isInstalling:false,isSavingChannel:false,updateInfo:null,error:null,setChannel:async()=>{},checkForUpdates:async()=>{},...overrides};
  const {UpdateChannelSettings}=loadTsModule('src/components/UpdateChannelSettings.tsx',{
    './UpdateCheckProvider':{useUpdateCheckContext:()=>snapshot},
    '@/services/updateService':{updateErrorMessage:String},
  });
  return renderToStaticMarkup(React.createElement(UpdateChannelSettings,{id:'preview-option'}));
}

test('prerelease opt-in is named, described and unchecked by default',()=>{
  const html=settings();
  assert.match(html, /<label[^>]*for="preview-option"[^>]*>Include prereleases/);
  assert.match(html, /role="switch"[^>]*aria-checked="false"/);
  assert.match(html, /aria-describedby="preview-option-description"/);
  assert.match(html, /Stable releases only \(default\)/);
});

test('opted-in UI labels previews and explains the return to stable',()=>{
  const html=settings({channel:'preview',updateInfo:{available:true,version:'0.5.39',prerelease:true}});
  assert.match(html, /aria-checked="true"/);
  assert.match(html, /Prerelease 0.5.39 available/);
  assert.match(html, /never downgrades/);
});

test('channel selection waits for persisted preference initialization',()=>{
  const html=settings({ready:false});
  assert.match(html, /role="switch"[^>]*disabled=""/);
});

test('installation disables both the channel switch and another update check',()=>{
  const html=settings({isInstalling:true});
  assert.match(html, /role="switch"[^>]*disabled=""/);
  const buttons=html.match(/<button\b[^>]*>/g)||[];
  assert.equal(buttons.length,2);
  assert.ok(buttons.every(button=>/\sdisabled=""/.test(button)));
});

test('manual update errors remain visible and announced',()=>{
  const html=settings({error:'GitHub limited preview update checks. Try again later.'});
  assert.match(html, /role="alert"/);
  assert.match(html, /GitHub limited preview update checks/);
});

function loadUpdateModule() {
  return loadTsModule('src/components/UpdateChannelSettings.tsx', {
    './UpdateCheckProvider': { useUpdateCheckContext: () => ({}) },
    '@/services/updateService': { updateErrorMessage: String },
  });
}

test('the status line says what the update check currently knows', () => {
  const { describeUpdateStatus } = loadUpdateModule();
  const idle = { ready: true, currentVersion: '0.5.60', isChecking: false, isInstalling: false, isSavingChannel: false, updateInfo: null };
  assert.equal(describeUpdateStatus(idle).text, '0.5.60 · not checked yet');
  assert.equal(describeUpdateStatus({ ...idle, ready: false, currentVersion: '' }).text, 'Loading update settings…');
  assert.equal(describeUpdateStatus({ ...idle, isChecking: true }).text, 'Checking for updates…');
  assert.equal(describeUpdateStatus({ ...idle, isInstalling: true }).tone, 'active');
  assert.equal(describeUpdateStatus({ ...idle, updateInfo: { available: false } }).tone, 'ok');
  assert.equal(describeUpdateStatus({ ...idle, updateInfo: { available: true, version: '0.5.61', prerelease: false } }).text, 'Stable 0.5.61 available');
});

test('the check button and status sit in the card footer, with no time invented', () => {
  const html = settings({ currentVersion: '0.5.60' });
  assert.match(html, /Check now/);
  assert.match(html, /role="status"/);
  assert.doesNotMatch(html, /Checked /);
  const { checkedLabel } = loadUpdateModule();
  const at = new Date(2026, 10, 19, 10, 42);
  assert.match(checkedLabel(at, new Date(2026, 10, 19, 12, 0)), /^Checked today /);
  assert.doesNotMatch(checkedLabel(at, new Date(2026, 10, 20, 12, 0)), /today/);
});

test('automatic checks keep running in the tray, change channel and stop when disabled', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
  const window = new EventTarget();
  const previousWindow = globalThis.window;
  globalThis.window = window;
  t.after(() => { globalThis.window = previousWindow; });
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  let enabled = true;
  let nextCheck;
  const calls = [], notifications = [];
  const snapshot = { ready: true, channel: 'stable' };
  const updateService = {
    initialize: async () => {},
    checkForUpdates: async force => {
      calls.push([snapshot.channel, force]);
      return nextCheck?.promise;
    },
    getSnapshot: () => snapshot,
  };
  const { UpdateCheckProvider } = loadTsModule('src/components/UpdateCheckProvider.tsx', {
    react: { ...hook.react, useSyncExternalStore: () => snapshot },
    'react/jsx-runtime': { jsx: () => null, jsxs: () => null },
    '@/services/updateService': { updateService },
    '@/lib/updatePreferences': { AUTO_UPDATE_CHECK_CHANGED_EVENT: 'auto-update', getAutoUpdateCheckEnabled: () => enabled },
    './UpdateDialog': { UpdateDialog: () => null },
    './UpdateNotification': { showUpdateNotification: info => notifications.push(info) },
    sonner: { toast: { dismiss() {} } },
  });
  const render = () => hook.render(() => UpdateCheckProvider({ children: null }));
  render(); render();
  t.mock.timers.tick(2000);
  assert.deepEqual(calls, [['stable', false]]);
  t.mock.timers.tick(60 * 60 * 1000);
  assert.deepEqual(calls, [['stable', false], ['stable', false]]);
  snapshot.channel = 'preview';
  render();
  nextCheck = deferred();
  t.mock.timers.tick(2000);
  assert.deepEqual(calls.at(-1), ['preview', false]);
  enabled = false;
  window.dispatchEvent(new Event('auto-update'));
  render();
  nextCheck.resolve({ available: true, version: '0.5.61' });
  await flush();
  t.mock.timers.tick(48 * 60 * 60 * 1000);
  assert.equal(calls.length, 3);
  assert.deepEqual(notifications, []);
  window.dispatchEvent(new Event('check-updates-from-tray'));
  assert.deepEqual(calls.at(-1), ['preview', true]);
});
