const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');
const {createRequire} = require('node:module');
const {execFileSync} = require('node:child_process');
const tools = createRequire(path.join(process.env.UI_TOOLS, 'entry.cjs'));
const {chromium} = tools('playwright');
const esbuild = tools('esbuild');
const source = path.resolve(process.argv[2]);
const inputs = path.resolve(__dirname);
const frontend = path.join(source,'frontend');
const fixture = path.join(frontend,'tests/browser-fixture');
const site = path.join(fixture,'site');
const output = path.resolve(process.env.BROWSER_RESULTS || 'browser-results');
fs.mkdirSync(site,{recursive:true}); fs.mkdirSync(output,{recursive:true});
for(const [from,to] of [['browser-fixture.tsx','fixture.tsx'],['browser-mocks.tsx','mocks.tsx']])fs.copyFileSync(path.join(inputs,from),path.join(fixture,to));
const records=[];
const failures=[];
let server,browser;
(async()=>{
  await esbuild.build({entryPoints:[path.join(fixture,'fixture.tsx')],outfile:path.join(site,'app.js'),bundle:true,platform:'browser',format:'iife',jsx:'automatic',tsconfig:path.join(frontend,'tsconfig.json'),define:{'process.env.NODE_ENV':'"development"'},alias:{'@':path.join(frontend,'src'),'next/link':path.join(fixture,'mocks.tsx'),'@tauri-apps/api/core':path.join(fixture,'mocks.tsx'),'@/services/recordingService':path.join(fixture,'mocks.tsx')}});
  execFileSync('pnpm',['exec','tailwindcss','-c','tailwind.config.js','-i','src/app/globals.css','-o',path.join(site,'app.css'),'--content','src/**/*.{tsx,ts,js},tests/browser-fixture/*.tsx'],{cwd:frontend,stdio:'inherit'});
  fs.writeFileSync(path.join(site,'index.html'),'<!doctype html><html lang="en"><head><style>:root{--font-app-sans:Arial;--font-source-sans-3:Arial;--font-plex-mono:monospace}body{font-family:Arial,sans-serif}</style><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ClawScribe production component verification</title><link rel="stylesheet" href="/app.css"></head><body><div id="root"></div><script src="/app.js"></script></body></html>');
  server=http.createServer((request,response)=>{
    const file={'/':'index.html','/app.js':'app.js','/app.css':'app.css'}[request.url];
    if(!file){response.writeHead(404);response.end();return;}
    response.setHeader('Content-Type',file.endsWith('.js')?'application/javascript':file.endsWith('.css')?'text/css':'text/html');
    fs.createReadStream(path.join(site,file)).pipe(response);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  browser=await chromium.launch();
  const base=`http://127.0.0.1:${server.address().port}`;
  async function run(name, options, check){
    const page=await browser.newPage({viewport:{width:1100,height:700},reducedMotion:'reduce',...options});
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    try{
      if(options?.holdHistory)await page.addInitScript(()=>{window.holdChatHistory=true;});
      await page.goto(base);
      await page.getByRole('heading',{name:'ClawScribe UI regression fixture'}).waitFor();
      await check(page);
      assert.deepEqual(errors,[],'No uncaught browser errors');
      records.push({name,status:'passed'});console.log('PASS browser: '+name);
    }catch(error){
      records.push({name,status:'failed',error:String(error)});failures.push(name);
      await page.screenshot({path:path.join(output,`failure-${records.length}.png`),fullPage:true}).catch(()=>{});
      console.error('FAIL browser: '+name+'\n'+error.stack);
    }finally{await page.close();}
  }
  const openChat=async page=>{await page.getByRole('button',{name:'Open meeting chat',exact:true}).click();};
  const question=page=>page.getByRole('textbox',{name:'Question about this meeting'});
  const resolve=async(page,command,override)=>page.evaluate(({command,override})=>window.chatFixture.resolve(command,override),{command,override});

  await run('recording reload restores polling without clock-driven lifecycle rerenders',{},async page=>{
    await page.waitForFunction(()=>document.querySelector('[data-testid="lifecycle-state"]').textContent==='recording:false');
    await page.waitForFunction(()=>window.recordingFixture.listenerCount()===4);
    await page.waitForTimeout(100);
    const before=await page.evaluate(()=>window.recordingFixture.lifecycleRenders);
    await page.evaluate(()=>window.recordingFixture.state.active_duration=124.2);
    await page.waitForFunction(()=>document.querySelector('[data-testid="clock-value"]').textContent==='124');
    assert.equal(await page.evaluate(()=>window.recordingFixture.lifecycleRenders),before);
    assert.ok(await page.evaluate(()=>window.recordingFixture.calls)>=2);
  });
  await run('a delayed recording poll cannot overwrite a newer pause or stop',{},async page=>{
    await page.waitForFunction(()=>window.recordingFixture.listenerCount()===4);
    await page.evaluate(()=>window.recordingFixture.holdReads=true);
    await page.waitForFunction(()=>window.recordingFixture.pending.length>0);
    await page.evaluate(()=>{window.recordingFixture.emit('paused');window.recordingFixture.resolveReads();});
    await page.waitForFunction(()=>document.querySelector('[data-testid="lifecycle-state"]').textContent==='recording:true');
    await page.waitForTimeout(100);
    assert.equal(await page.getByTestId('lifecycle-state').textContent(),'recording:true');
    await page.waitForFunction(()=>window.recordingFixture.pending.length>0);
    await page.evaluate(()=>{window.recordingFixture.emit('stopped');window.recordingFixture.resolveReads();});
    await page.waitForFunction(()=>document.querySelector('[data-testid="lifecycle-state"]').textContent==='stopping:false');
    const calls=await page.evaluate(()=>window.recordingFixture.calls);
    await page.waitForTimeout(700);
    assert.equal(await page.evaluate(()=>window.recordingFixture.calls),calls);
  });
  await run('recording provider cleanup removes listeners and pending polling',{},async page=>{
    await page.waitForFunction(()=>window.recordingFixture.listenerCount()===4);
    await page.evaluate(()=>window.recordingFixture.holdReads=true);
    await page.waitForFunction(()=>window.recordingFixture.pending.length>0);
    await page.evaluate(()=>{window.unmountFixture();window.recordingFixture.resolveReads();});
    assert.equal(await page.evaluate(()=>window.recordingFixture.listenerCount()),0);
    const calls=await page.evaluate(()=>window.recordingFixture.calls);
    await page.waitForTimeout(700);
    assert.equal(await page.evaluate(()=>window.recordingFixture.calls),calls);
  });
  await run('keyboard meeting navigation and visible action focus',{},async page=>{
    const link=page.getByRole('link',{name:/Quarterly review/});await link.focus();await page.keyboard.press('Enter');
    assert.equal(new URL(page.url()).hash,'#opened');
    const edit=page.getByRole('button',{name:'Edit meeting title: Quarterly review'});await edit.focus();
    assert.equal(await edit.evaluate(element=>getComputedStyle(element.parentElement).opacity),'1');
    await page.keyboard.press('Enter');assert.equal(new URL(page.url()).hash,'#opened');
  });
  await run('delete modal focus containment, Escape and restore',{},async page=>{
    const trigger=page.getByRole('button',{name:'Delete meeting: Quarterly review'});await trigger.focus();await page.keyboard.press('Enter');
    const dialog=page.getByRole('dialog');await dialog.waitFor();
    assert.equal(await page.getByRole('button',{name:'Cancel',exact:true}).evaluate(element=>element===document.activeElement),true);
    for(let i=0;i<8;i++){await page.keyboard.press('Tab');assert.equal(await dialog.evaluate(element=>element.contains(document.activeElement)),true);}
    await page.keyboard.press('Escape');await dialog.waitFor({state:'hidden'});
    assert.equal(await trigger.evaluate(element=>element===document.activeElement),true);
  });
  await run('delete failure is recoverable and success removes only selected meeting',{},async page=>{
    await page.evaluate(()=>window.failDelete=true);await page.getByRole('button',{name:'Delete meeting: Quarterly review'}).click();
    await page.getByRole('dialog').getByRole('button',{name:'Delete',exact:true}).click();
    await page.getByRole('alert').filter({hasText:'could not be completed'}).waitFor();
    assert.equal(await page.locator('a[href="#opened"]').count(),1,'The background row remains in the DOM while the dialog correctly hides it from assistive technology');
    await page.evaluate(()=>window.failDelete=false);await page.getByRole('dialog').getByRole('button',{name:'Delete',exact:true}).click();
    await page.getByText('Meeting deleted',{exact:true}).waitFor();assert.equal(await page.getByRole('dialog').count(),0);
  });
  await run('history loading prevents new-send overwrite', {holdHistory:true}, async page=>{
    await openChat(page);assert.equal(await question(page).isDisabled(),true);
    assert.equal(await page.getByRole('button',{name:'Send question',exact:true}).isDisabled(),true);
    await resolve(page,'api_chat_history');await question(page).fill('Has this been agreed?');
    await page.getByRole('button',{name:'Send question',exact:true}).click();
    assert.equal(await page.getByRole('button',{name:'Clear chat history',exact:true}).isDisabled(),true);
    await resolve(page,'api_chat_send');await page.getByText('The proposal is not an agreed decision.',{exact:true}).waitFor();
    assert.equal(await page.getByText('Has this been agreed?',{exact:true}).count(),1);
  });
  await run('failed question survives and retries without duplicate message',{},async page=>{
    await openChat(page);await page.evaluate(()=>window.chatFixture.failSends=1);await question(page).fill('Who owns this task?');
    await page.getByRole('button',{name:'Send question',exact:true}).click();await page.getByRole('alert').waitFor();
    assert.equal(await question(page).inputValue(),'Who owns this task?');
    await page.getByRole('button',{name:'Send question',exact:true}).click();await resolve(page,'api_chat_send');
    await page.getByText('The proposal is not an agreed decision.',{exact:true}).waitFor();
    const ids=await page.evaluate(()=>window.chatFixture.calls.filter(call=>call.command==='api_chat_send').map(call=>call.args.requestId));
    assert.equal(ids.length,2);assert.equal(ids[0],ids[1]);assert.equal(await page.getByText('Who owns this task?',{exact:true}).count(),1);
  });
  await run('meeting switch invalidates a pending response and draft',{},async page=>{
    await openChat(page);await question(page).fill('First meeting only');await page.getByRole('button',{name:'Send question',exact:true}).click();
    await page.getByRole('button',{name:'Switch meeting',exact:true}).click();await openChat(page);await resolve(page,'api_chat_send');
    await page.waitForTimeout(100);assert.equal(await page.getByText('First meeting only',{exact:true}).count(),0);
    assert.equal(await page.getByText('The proposal is not an agreed decision.',{exact:true}).count(),0);assert.equal(await question(page).inputValue(),'');
  });
  await run('partial context is disclosed and clearing history is confirmed',{},async page=>{
    await openChat(page);await question(page).fill('What happened?');await page.getByRole('button',{name:'Send question',exact:true}).click();
    await page.evaluate(()=>{const request=window.chatFixture.pending.find(p=>p.command==='api_chat_send');window.chatFixture.resolve('api_chat_send',{id:'reply-'+request.args.requestId,meeting_id:request.args.meetingId,role:'assistant',content:'Excerpt-based answer.',context_truncated:true,created_at:'2026-01-01'});});
    await page.getByText(/This answer used only the beginning and end/).waitFor();
    await page.getByRole('button',{name:'Clear chat history',exact:true}).click();await page.getByRole('dialog',{name:'Clear chat history?'}).waitFor();
    await page.getByRole('button',{name:'Cancel',exact:true}).click();assert.equal(await page.getByText('Excerpt-based answer.',{exact:true}).count(),1);
    await page.getByRole('button',{name:'Clear chat history',exact:true}).click();await page.getByRole('dialog').getByRole('button',{name:'Clear history',exact:true}).click();
    await page.getByRole('dialog').waitFor({state:'hidden'});assert.equal(await page.getByText('Excerpt-based answer.',{exact:true}).count(),0);
  });
  await run('long transcript stays virtualized and resumes live following',{},async page=>{
    const rowCount=await page.locator('[id^="segment-row-"]').count();assert.ok(rowCount>0&&rowCount<100,`Expected a bounded mounted list; got ${rowCount}`);
    await page.getByRole('button',{name:'Append finalized segment',exact:true}).click();
    const scroller=page.locator('section[aria-label="Live transcript fixture"] > div');
    await scroller.evaluate(element=>{element.scrollTop=0;element.dispatchEvent(new Event('wheel'));element.dispatchEvent(new Event('scroll'));});
    await page.getByRole('button',{name:'Jump to live transcript',exact:true}).waitFor();
    await page.getByRole('button',{name:'Append finalized segment',exact:true}).click();
    assert.ok(await scroller.evaluate(element=>element.scrollTop)<100,'Appending must not pull a reader to the bottom');
    await page.getByRole('button',{name:'Jump to live transcript',exact:true}).click();
    await page.waitForFunction(()=>{const e=document.querySelector('section[aria-label="Live transcript fixture"] > div');return e.scrollHeight-e.scrollTop-e.clientHeight<200;});
    await page.getByText('A new finalized transcript segment is immediately readable.',{exact:true}).first().waitFor();
    assert.ok(await page.locator('[id^="segment-row-"]').count()<100);
  });
  for(const mode of [{width:1100,height:700,theme:'light'},{width:800,height:560,theme:'light'},{width:800,height:560,theme:'dark'}]){
    await run(`rendered ${mode.width}x${mode.height} ${mode.theme} layout and accessibility`,{viewport:{width:mode.width,height:mode.height}},async page=>{
      if(mode.theme==='dark')await page.getByRole('button',{name:'Toggle theme',exact:true}).click();
      assert.equal(await page.getByTestId('layout-mode').textContent(),mode.width<1024?'compact':'wide');
      await openChat(page);await question(page).waitFor();await page.waitForTimeout(350);
      const panel=page.locator('section[aria-labelledby]');const box=await panel.boundingBox();assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=mode.width+1&&box.y+box.height<=mode.height+1,JSON.stringify(box));
      await page.screenshot({path:path.join(output,`${mode.width}-${mode.height}-${mode.theme}-chat.png`),fullPage:true});
      await page.addScriptTag({path:tools.resolve('axe-core/axe.min.js')});
      const accessibility=await page.evaluate(async()=>{const r=await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa']}});return r.violations.map(v=>({id:v.id,impact:v.impact,description:v.description,nodes:v.nodes.map(n=>({target:n.target,summary:n.failureSummary}))}));});
      fs.writeFileSync(path.join(output,`${mode.width}-${mode.height}-${mode.theme}-axe.json`),JSON.stringify(accessibility,null,2));
      assert.deepEqual(accessibility,[],'Automated accessibility violations on the fixture surface');
      await page.keyboard.press('Escape');assert.equal(await page.getByRole('button',{name:'Open meeting chat',exact:true}).evaluate(element=>element===document.activeElement),true);
    });
  }
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify({source_commit:process.env.SOURCE_COMMIT,scope:'Actual production React/Radix components in Chromium with system-font fallbacks; provider/device boundary simulated. Not a full application screenshot, native Windows or physical capture acceptance.',checks:records},null,2));
  if(failures.length)throw Error(`${failures.length} browser checks failed: ${failures.join(', ')}`);
  console.log(`All ${records.length} rendered browser checks passed.`);
})().catch(error=>{console.error(error);process.exitCode=1;}).finally(async()=>{if(browser)await browser.close();if(server)await new Promise(resolve=>server.close(resolve));});
