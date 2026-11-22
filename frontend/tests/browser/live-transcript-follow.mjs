import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import tailwind from 'tailwindcss';

const require = createRequire(import.meta.url);
const frontend = fileURLToPath(new URL('../../', import.meta.url));
const browserPath = process.env.CLAWSCRIBE_TEST_BROWSER ?? (process.platform === 'win32' &&
  path.join(process.env['ProgramFiles(x86)'] ?? 'C:/Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'));
assert.ok(browserPath, 'Set CLAWSCRIBE_TEST_BROWSER to a Chromium executable');
process.chdir(frontend);

// Exercise real React, Tailwind layout, and TanStack measurements. Replace the
// native services and unrelated Home controls, and construct passages in memory.
const entry = String.raw`
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { TooltipProvider } from '@/components/ui/tooltip';
import { TranscriptProvider, useTranscripts } from '@/contexts/TranscriptContext';
import { TranscriptPanel } from '@/app/_components/TranscriptPanel';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';

function Home() {
  const { addTranscript } = useTranscripts();
  const [started, start] = useState(false);
  const [stopping, stop] = useState(false);
  useEffect(() => {
    window.append = count => {
      for (let i = 0; i < count; i++) {
        const n = ++window.count;
        addTranscript({ sequence_id: n, audio_start_time: n * 4, timestamp: String(n),
          text: 'Passage ' + n + '. ' + 'The park opens before lunch and closes after dinner. '.repeat(n % 4 + 2) });
      }
    };
    window.stop = () => stop(true);
    start(true); // Home initially shows its dashboard while the provider is already mounted.
  }, [addTranscript]);
  return <div className="flex h-full flex-col">
    <div className="flex flex-1 overflow-hidden">
      {started && <TranscriptPanel isProcessingStop={false} isStopping={stopping} />}
    </div>
  </div>;
}

function Saved() {
  const [time, play] = useState(0);
  const [follow, setFollow] = useState(true);
  const [segments] = useState(() => Array.from({ length: 120 }, (_, n) => ({
    id: String(n), timestamp: n * 4, endTime: n * 4 + 3, text: 'Saved passage ' + n,
  })));
  window.play = play;
  window.followPlayback = follow;
  return <VirtualizedTranscriptView segments={segments} activeTime={time} disableAutoScroll
    onSeekToTime={play} followPlayback={follow} onUserScroll={() => setFollow(false)} />;
}

window.count = 0;
const root = createRoot(document.getElementById('root'));
window.openHome = key => root.render(<TooltipProvider><TranscriptProvider key={key}><Home /></TranscriptProvider></TooltipProvider>);
window.openSaved = () => root.render(<TooltipProvider><Saved /></TooltipProvider>);
window.openHome('initial');
`;

const stubs = String.raw`
export const RecordingStatus = { STARTING: 'starting' };
export const useRecordingState = () => ({ isRecording: true, status: 'recording', sessionMode: 'live' });
export const useConfig = () => ({ selectedLanguage: 'en' });
export const usePermissionCheck = () => ({ isChecking: false });
export const useIsLinux = () => false;
export const RecordingCalendarReview = () => null;
export const LiveSourceLanguages = () => null;
export const PermissionWarning = () => null;
export const transcriptService = {
  onTranscriptUpdate: async () => () => {},
  getTranscriptHistory: async () => window.historyRows ?? [],
};
export const recordingService = {
  onRecordingStarted: async () => () => {},
  getRecordingMeetingName: async () => 'Example',
};
export const indexedDBService = {};
`;

async function compile(directory) {
  const entryPath = path.join(directory, 'entry.tsx');
  const mocks = path.join(directory, 'stubs.mjs');
  const loader = path.join(directory, 'loader.cjs');
  await Promise.all([
    writeFile(entryPath, entry),
    writeFile(mocks, stubs),
    writeFile(loader, `const ts = require(${JSON.stringify(require.resolve('typescript'))});
      module.exports = source => ts.transpileModule(source, { compilerOptions: {
        module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2020,
      }}).outputText;`),
  ]);

  const stubModules = [
    '@/contexts/RecordingStateContext', './RecordingStateContext', '@/contexts/ConfigContext',
    '@/hooks/usePermissionCheck', '@/hooks/usePlatform', '@/components/MeetingAttendance',
    '@/components/PermissionWarning', '@/components/LiveSourceLanguages', '@/services/transcriptService',
    '@/services/recordingService', '@/services/indexedDBService',
  ];
  const webpack = require('next/dist/compiled/webpack/webpack');
  webpack.init();
  const compiler = webpack.webpack({
    mode: 'development', devtool: false, entry: entryPath,
    output: { path: directory, filename: 'test.js' },
    resolve: {
      extensions: ['.ts', '.tsx', '.js'],
      modules: [path.join(frontend, 'node_modules'), 'node_modules'],
      alias: { ...Object.fromEntries(stubModules.map(name => [name + '$', mocks])), '@': path.join(frontend, 'src') },
    },
    module: { rules: [{ test: /\.tsx?$/, use: loader }] },
  });

  try {
    await new Promise((resolve, reject) => compiler.run((error, stats) => {
      if (error || stats.hasErrors()) reject(error ?? new Error(stats.toString({ all: false, errors: true })));
      else resolve();
    }));
  } finally {
    await new Promise(resolve => compiler.close(resolve));
  }
  const cssPath = path.join(frontend, 'src/app/globals.css');
  const { css } = await postcss([tailwind]).process(await readFile(cssPath, 'utf8'), { from: cssPath });
  return {
    '/': ['text/html', '<!doctype html><meta charset="UTF-8"><link rel="stylesheet" href="/test.css"><div id="root" style="height:100vh"></div><script src="/test.js"></script>'],
    '/test.js': ['text/javascript', await readFile(path.join(directory, 'test.js'))],
    '/test.css': ['text/css', css],
  };
}

async function until(description, check) {
  for (let attempt = 0; attempt < 100; attempt++) {
    const value = await check();
    if (value) return value;
    await wait(50);
  }
  assert.fail(description);
}

async function connect(profile) {
  const port = await until('Chromium did not start', async () => {
    try {
      return Number((await readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      return null;
    }
  });
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = pages.find(page => page.type === 'page' && page.url === 'about:blank');
  assert.ok(page, 'Expected a fresh Chromium page');
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let id = 0;
  const pending = new Map();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  };
  const send = (method, params) => new Promise((resolve, reject) => {
    const request = ++id;
    const timer = setTimeout(() => {
      pending.delete(request);
      reject(new Error(`Chromium did not respond to ${method}`));
    }, 5000);
    pending.set(request, message => {
      clearTimeout(timer);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    });
    socket.send(JSON.stringify({ id: request, method, params }));
  });
  const evaluate = async expression => {
    const { result, exceptionDetails } = await send('Runtime.evaluate', { expression, returnByValue: true });
    assert.ok(!exceptionDetails, result.description);
    return result.value;
  };
  return { socket, send, evaluate };
}

const layout = String.raw`(() => {
  const rows = [...document.querySelectorAll('[data-segment-id]')];
  const scrollAreas = [];
  for (let parent = rows[0]?.parentElement; parent; parent = parent.parentElement) {
    if (getComputedStyle(parent).overflowY === 'auto') scrollAreas.push(parent);
  }
  const viewport = scrollAreas.find(area => area.scrollHeight > area.clientHeight);
  const jump = [...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Jump to live transcript');
  const rect = jump?.getBoundingClientRect();
  return {
    scrollAreas: scrollAreas.length, top: viewport?.scrollTop,
    gap: viewport && viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop,
    jump: !!rect && document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2)?.closest('button') === jump,
    tail: rows.at(-1)?.dataset.segmentId,
  };
})()`;

async function exercise({ send, evaluate }, url) {
  const metrics = () => evaluate(layout);
  const resize = (width, height, deviceScaleFactor) => send('Emulation.setDeviceMetricsOverride',
    { width, height, deviceScaleFactor, mobile: false });
  const wheel = deltaY => send('Input.dispatchMouseEvent',
    { type: 'mouseWheel', x: 400, y: 190, deltaX: 0, deltaY });
  const append = async (count, expectedTail) => {
    await evaluate(`append(${count})`);
    await until('Expected new speech to render', async () => (await metrics()).tail === String(expectedTail));
    // The typewriter and virtual row measurements settle after initial paint.
    await wait(950);
  };
  const jump = () => evaluate("Array.from(document.querySelectorAll('button')).find(el => el.textContent.trim() === 'Jump to live transcript').click()");
  await resize(800, 410, 2.5);
  await send('Page.navigate', { url });
  await until('Recording view did not mount', () => evaluate('typeof window.append === "function"'));
  await append(10, 10);
  assert.ok((await metrics()).top > 0);
  await wheel(-2800);
  await until('Wheel did not reach the earliest passage', async () => (await metrics()).top === 0);
  await append(1, 11);
  const first = await metrics();
  assert.equal(first.top, 0, 'New speech pulled the reader away from the earliest passage');
  assert.ok(first.jump, 'Jump to live must be visible and hit-testable');
  assert.equal(first.scrollAreas, 1, 'The transcript must have one scroll area');

  await jump();
  await until('Jump did not reach the tail', async () => { const state = await metrics(); return state.gap <= 2 && !state.jump; });
  await wheel(-180);
  const reading = await until('Could not scroll back a second time', async () => {
    const state = await metrics();
    return state.jump && state.gap > 100 && state;
  });
  await append(1, 12);
  const second = await metrics();
  assert.equal(second.top, reading.top);
  assert.ok(second.jump);
  await evaluate('stop()');
  await until('Stop should hide the live jump', async () => !(await metrics()).jump);
  await append(1, 13);
  assert.equal((await metrics()).top, second.top, 'Speech arriving during Stop must leave the reader in place');

  await resize(1200, 560, 1);
  await evaluate(`window.historyRows = Array.from({ length: 9 }, (_, index) => ({
    id: String(index + 1), sequence_id: index + 1, display_time: String(index),
    text: 'The plant needs water before the afternoon. '.repeat(index % 4 + 3),
  })); window.count = 9; openHome('reloaded');`);
  await until('Restored short transcript did not open at the tail', async () => {
    const state = await metrics();
    return state.tail === '9' && state.gap <= 2;
  });
  await evaluate('document.querySelector("[aria-label=Transcript]").focus()');
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
  await until('Short transcript did not detach', async () => (await metrics()).jump);
  await append(1, 10);
  assert.equal((await metrics()).top, 0, 'Crossing the virtualization threshold moved the reader');
  await jump();
  await append(1, 11);
  assert.ok((await metrics()).gap <= 2, 'New speech must follow after jumping to live');

  await wheel(-320);
  await until('Wheel scroll-back did not detach', async () => (await metrics()).jump);
  await wheel(2800);
  await until('Reaching the bottom by wheel did not resume following', async () => !(await metrics()).jump);
  await append(1, 12);
  assert.ok((await metrics()).gap <= 2, 'New speech must follow a manual return to the bottom');

  await wheel(-320);
  await until('Second wheel scroll-back did not detach', async () => (await metrics()).jump);
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'End', code: 'End', windowsVirtualKeyCode: 35 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'End', code: 'End', windowsVirtualKeyCode: 35 });
  await until('End did not resume following', async () => !(await metrics()).jump);
  await append(1, 13);
  assert.ok((await metrics()).gap <= 2, 'New speech must follow after End');

  await evaluate('openSaved()');
  await until('Saved transcript did not mount', () => evaluate('typeof window.play === "function"'));
  await evaluate('play(420)');
  await until('Saved follow did not reveal the late passage', () => evaluate(`(() => {
    const row = document.getElementById('segment-105');
    const region = document.querySelector('[aria-label=Transcript]');
    if (!row || !region) return false;
    const item = row.getBoundingClientRect(), viewport = region.getBoundingClientRect();
    return item.top >= viewport.top && item.bottom <= viewport.bottom;
  })()`));
  await wheel(-260);
  await until('Manual scrolling did not turn off saved playback follow', () => evaluate('!window.followPlayback'));
  assert.equal((await metrics()).jump, false);
}

const testCache = path.join(frontend, 'node_modules/.cache');
await mkdir(testCache, { recursive: true });
const directory = await mkdtemp(path.join(testCache, 'clawscribe-live-transcript-'));
let browser, server, connection;
try {
  const files = new Map(Object.entries(await compile(directory)));
  server = createServer((request, response) => {
    const file = files.get(request.url);
    if (!file) return response.writeHead(404).end();
    response.writeHead(200, { 'Content-Type': file[0] }).end(file[1]);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const profile = path.join(directory, 'Chromium');
  await mkdir(profile);
  browser = spawn(browserPath, ['--headless=new', '--no-first-run', '--no-default-browser-check',
    '--disable-background-networking', '--disable-extensions', '--remote-debugging-port=0',
    '--disable-features=msEdgeStartupBoost,msEdgeBackgroundMode', `--user-data-dir=${profile}`,
    'about:blank'], { windowsHide: true, stdio: 'ignore' });
  connection = await Promise.race([
    connect(profile),
    once(browser, 'error').then(([error]) => { throw error; }),
  ]);
  await exercise(connection, `http://127.0.0.1:${server.address().port}/`);
  console.log('Live transcript browser regression passed.');
} finally {
  if (connection) {
    await Promise.race([connection.send('Browser.close').catch(() => {}), wait(1000)]);
    connection.socket.close();
  }
  if (browser?.pid && browser.exitCode === null && browser.signalCode === null) {
    const exited = once(browser, 'exit');
    browser.kill();
    await exited;
  }
  if (server?.listening) await new Promise(resolve => server.close(resolve));
  await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
}
