import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { loadTsModule } from './load-ts-module.mjs';

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
