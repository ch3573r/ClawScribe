from pathlib import Path
import hashlib
import json
import subprocess
import sys

root = Path.cwd()
tools = Path(sys.argv[1]).resolve()

def once(text, before, after):
    if text.count(before) != 1:
        raise SystemExit(f'Expected one fixture transformation: {before[:80]}')
    return text.replace(before, after, 1)

for part in ['followup.json', 'followup-extra.json']:
    for change in json.loads((tools / part).read_text()):
        target = root / change['path']
        original = target.read_bytes() if target.exists() else b''
        blob = hashlib.sha1(b'blob '+str(len(original)).encode()+b'\0'+original).hexdigest() if target.exists() else None
        if blob != change['base']:
            raise SystemExit(f'Unexpected follow-up baseline: {change["path"]}: {blob}')
        lines = original.decode().splitlines(keepends=True)
        for start,end,replacement in reversed(change['edits']):
            lines[start:end] = [replacement]
        result = ''.join(lines).encode()
        if hashlib.sha256(result).hexdigest() != change['sha256']:
            raise SystemExit(f'Follow-up result hash mismatch: {change["path"]}')
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(result)
        print('Verified follow-up bytes:', change['path'])

browser = root / 'frontend/tests/browser'
browser.mkdir(parents=True, exist_ok=True)
fixture = (tools / 'browser-fixture.tsx').read_text()
fixture = once(fixture, "import { useCompactLayout } from '@/hooks/useCompactLayout';", "import { useCompactLayout } from '@/hooks/useCompactLayout';\nimport { RecordingStateProvider, useRecordingState, useRecordingClock } from '@/contexts/RecordingStateContext';")
fixture = once(fixture, 'function Fixture() {', '''function LifecycleProbe() {
  const state = useRecordingState();
  const fixture = (window as any).recordingFixture;
  fixture.lifecycleRenders++;
  return <span hidden data-testid="lifecycle-state">{state.status}:{String(state.isPaused)}</span>;
}
function ClockProbe() {
  const clock = useRecordingClock();
  return <span hidden data-testid="clock-value">{clock.activeDuration ?? 'none'}</span>;
}
function Fixture() {''')
fixture = once(fixture, '<h1 style=', '<LifecycleProbe /><ClockProbe /><h1 style=')
fixture = once(fixture, "createRoot(document.getElementById('root')!).render(<React.StrictMode><Fixture /></React.StrictMode>);", "const fixtureRoot = createRoot(document.getElementById('root')!);\n(window as any).unmountFixture = () => fixtureRoot.unmount();\nfixtureRoot.render(<React.StrictMode><RecordingStateProvider><Fixture /></RecordingStateProvider></React.StrictMode>);")
(browser / 'browser-fixture.tsx').write_text(fixture)

mocks = (tools / 'browser-mocks.tsx').read_text()
mocks = once(mocks, "export const RecordingStatus = { STARTING:'starting', RECORDING:'recording' };\nexport function useRecordingState() { return {activeDuration:123,status:RecordingStatus.RECORDING}; }", '''const listeners: Record<string, Set<() => void>> = Object.fromEntries(['started','stopped','paused','resumed'].map(name=>[name,new Set()]));
const recording = root.recordingFixture = {
  state: {is_recording:true,is_paused:false,is_active:true,recording_duration:123.2,active_duration:123.2},
  lifecycleRenders: 0,
  calls: 0,
  holdReads: false,
  pending: [] as (()=>void)[],
  listenerCount() { return Object.values(listeners).reduce((sum,set)=>sum+set.size,0); },
  resolveReads() { const pending=this.pending.splice(0); pending.forEach(resolve=>resolve()); },
  emit(event: string) {
    if(event==='paused') Object.assign(this.state,{is_paused:true,is_active:false});
    if(event==='resumed') Object.assign(this.state,{is_paused:false,is_active:true});
    if(event==='stopped') Object.assign(this.state,{is_recording:false,is_paused:false,is_active:false});
    if(event==='started') Object.assign(this.state,{is_recording:true,is_paused:false,is_active:true,recording_duration:0,active_duration:0});
    listeners[event].forEach(callback=>callback());
  },
};
const subscribe = async (name: string, callback: ()=>void) => {
  listeners[name].add(callback);
  return () => {listeners[name].delete(callback);};
};
export const recordingService = {
  getRecordingState: async () => {
    recording.calls++;
    const snapshot={...recording.state};
    if(recording.holdReads) await new Promise<void>(resolve=>recording.pending.push(resolve));
    return snapshot;
  },
  onRecordingStarted: (callback:()=>void)=>subscribe('started',callback),
  onRecordingStopped: (callback:()=>void)=>subscribe('stopped',callback),
  onRecordingPaused: (callback:()=>void)=>subscribe('paused',callback),
  onRecordingResumed: (callback:()=>void)=>subscribe('resumed',callback),
};''')
(browser / 'browser-mocks.tsx').write_text(mocks)

checks = (tools / 'browser-checks.cjs').read_text()
checks = once(checks, "'@/contexts/RecordingStateContext':path.join(fixture,'mocks.tsx')", "'@/services/recordingService':path.join(fixture,'mocks.tsx')")
checks = once(checks, "assert.equal(await page.getByRole('link',{name:/Quarterly review/}).count(),1);", "assert.equal(await page.locator('a[href=\"#opened\"]').count(),1,'The background row remains in the DOM while the dialog correctly hides it from assistive technology');")
checks = once(checks, '<head><meta charset=', '<head><style>:root{--font-app-sans:Arial;--font-source-sans-3:Arial;--font-plex-mono:monospace}body{font-family:Arial,sans-serif}</style><meta charset=')
checks = once(checks, "  await run('keyboard meeting navigation", '''  await run('recording reload restores polling without clock-driven lifecycle rerenders',{},async page=>{
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
  await run('keyboard meeting navigation''')
checks = checks.replace('Actual production React/Radix components in Chromium; Tauri boundary simulated. Not native Windows or physical capture acceptance.', 'Actual production React/Radix components in Chromium with system-font fallbacks; provider/device boundary simulated. Not a full application screenshot, native Windows or physical capture acceptance.')
(browser / 'browser-checks.cjs').write_text(checks)
(root / 'scripts/test-quality-core.py').write_text((tools / 'core_checks.py').read_text())

notes = '''# Meeting quality and resource verification

## Unreleased implementation

This work changes chat state, recording lifecycle, transcript updates and resource
handling. Published 0.5.36 installers are unchanged. A future installer needs a new
numeric version. This guide describes verification commands, not a passing run.

### Behaviour to preserve

- Chat waits for history, prevents duplicate sends and send/clear races, preserves
  failed questions, and rejects stale replies after a meeting switch. Native
  per-meeting locking and stable SQLite request IDs protect retries and persistence.
- Long-meeting chat visibly discloses beginning/end excerpts. This is not retrieval
  of every relevant fact from the omitted middle or provider-specific token budgeting.
- Transcript ingestion preserves final corrections and chronological order with
  an indexed append path. Final text is shown immediately, without a typewriter delay.
- The recording clock has a separate subscription; clock ticks do not invalidate
  unrelated lifecycle consumers. Polls do not overlap or overwrite newer events.
- Keyboard-accessible navigation/dialogs, focusable transcript scrolling, readable
  speaker labels and compact transcript/notes views support the smaller minimum
  800 by 560 window. This is not a complete Windows accessibility certification.
- Each raw capture/writer queue is limited to 32 MiB and 4096 items. On overload,
  recording stops and is explicitly marked incomplete. This is NOT lossless overflow
  or a process-wide memory cap. Accepted queued audio drains before finalization;
  final short mixer windows are retained. Failed recordings do not auto-export as
  complete. Checkpoints are retained until merged audio is validated and durable.
- Encoding/finalization run outside async executor threads. Encoder diagnostics are
  bounded and pipe I/O is drained concurrently. A stalled child has a deadline.
- Local-summary prompt decoding uses batches capped at 512 tokens and a conservative
  maximum of four threads. Context overflow is an error, not a successful partial
  answer. No benchmark claim about local model accuracy or target-PC speed follows.

## Automated checks

From the repository root:

```text
cd frontend
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm build
```

`python scripts/test-quality-core.py` tests actual dependency-light Rust modules
and the chat repository against real in-memory SQLite. It is not the complete
native application. On Windows, stage the pinned helpers/runtime as described in
`windows-release.md`, then run `frontend/scripts/test-windows-native.ps1`. The
required suite list includes the queue, lifecycle, tail, checkpoint and chat cases;
a missing/empty test selection fails rather than pretending validation occurred.

Rendered browser checks live in `frontend/tests/browser`. They bundle the actual
production components and styles, using explicit simulated device/provider
boundaries and system-font fallbacks. They test keyboard focus, destructive-action
recovery, delayed chat responses, recording-clock subscriptions, 3000-row transcript
virtualization and light/dark layouts at 1100x700 and 800x560. Automated axe checks
cover only that fixture surface. They are not screenshots of the entire desktop app.

Install pinned verification tools outside the application dependency tree, set
`UI_TOOLS` to that directory, install Playwright Chromium, then run:

```text
node frontend/tests/browser/browser-checks.cjs .
```

The tool directory needs Playwright 1.56.1, esbuild 0.25.10 and axe-core 4.10.3.
`BROWSER_RESULTS` optionally selects the output directory for JSON and screenshots.
No live provider request, meeting recording, account sign-in or publication occurs.

For the reproducible, isolated transcript algorithm benchmark:

```text
node --expose-gc frontend/tests/performance/transcript-index.bench.mjs
```

It reads and hashes the pinned baseline Git object before comparing actual callback
code with the production index. A shallow checkout must fetch that baseline commit.
Times are cumulative ingestion work, not UI freezes or full-meeting speedups.

## Separate release acceptance

Native build success does not prove real microphone/system-audio capture, accurate
recognition, a stable two-hour meeting, install/upgrade safety, screen-reader
behaviour, or i5-1235U/8 GB performance. Test those on Windows, including pause/stop,
short final speech, device changes, slow disk, recovery and simultaneous model use.
Provider-wide context budgeting, evidence-linked notes and measured model comparisons
remain separate work; review generated notes against the transcript and audio.
'''
(root / 'docs/quality-verification.md').write_text(notes)
p = root / 'CHANGELOG.md'
s = p.read_text()
index = s.index('## ')
s = s[:index] + '''## Unreleased

- Fix meeting-chat history/send/clear races, recoverable retries, stale meeting replies, and idempotent native persistence.
- Disclose partial transcript coverage; bound chat source/history preparation without claiming full-context retrieval.
- Improve keyboard navigation, destructive confirmations, transcript focus/contrast, and compact meeting views; restore native text context menus.
- Reduce transcript ingestion overhead, retain final corrections, display finalized text immediately, and isolate recording clock subscriptions.
- Bound capture/writer queues with explicit incomplete-recording errors, drain accepted audio and short mixer tails before finalization, and preserve recovery checkpoints on failure.
- Move encoder/finalization work off async executor threads, bound child diagnostics/time, and avoid marking failed recordings ready for automatic handoff.
- Cap local-summary prompt batch/thread defaults and reject exhausted context instead of returning incomplete output as successful.
- Add reproducible frontend, native, SQLite, rendered-browser and transcript benchmark checks. See `docs/quality-verification.md` for scope and outstanding real-device acceptance.

''' + s[index:]
p.write_text(s)
p = root / 'README.md'
s = p.read_text()
marker = '## Development\n'
assert s.count(marker) == 1
s = s.replace(marker, '''## Unreleased Quality Work

Source changes after the 0.5.36 tag improve chat recovery, keyboard controls,
transcript update efficiency, and bounded recording resources. They are not yet
included in the published 0.5.36 installers. See
[quality verification](docs/quality-verification.md) for the implemented behaviour,
repeatable checks, and the distinction between automated and real-device evidence.

''' + marker, 1)
p.write_text(s)
subprocess.run(['git','add','--','frontend','llama-helper','scripts/test-quality-core.py','docs/quality-verification.md','README.md','CHANGELOG.md'],check=True)
subprocess.run(['git','diff','--cached','--check'],check=True)
subprocess.run(['node','scripts/verify-public-repo-safety.mjs'],check=True)
