import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { matchesGlob } from 'node:path';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const live = { state: 'recording', seconds: 67, paused: false, levels: { mic: { rms: 0.2, peak: 0.3 }, system: null } };
const jsx = (type, props) => ({ type, props });
function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [node];
  return [node, ...nodes(node.props.children)];
}

function panel(t, initial = live) {
  const hooks = createHookHarness();
  const calls = [];
  let snapshot = initial;
  let action = Promise.resolve(true);
  let task;
  t.mock.method(globalThis, 'setTimeout', fn => { task = fn; return 1; });
  t.mock.method(globalThis, 'clearTimeout', () => { task = undefined; });
  const original = globalThis.window;
  globalThis.window = { matchMedia: () => ({ matches: false }), addEventListener() {}, removeEventListener() {} };
  t.after(() => {
    hooks.unmount();
    if (original === undefined) delete globalThis.window;
    else globalThis.window = original;
  });
  const Page = loadTsModule('src/pages/recording-controls.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    'next/head': 'Head',
    'lucide-react': Object.fromEntries(['BookmarkPlus', 'Mic', 'MonitorSpeaker', 'Pause', 'Play', 'Square', 'X'].map(icon => [icon, icon])),
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/input': { Input: 'Input' },
    '@/components/ui/progress': { Progress: 'Progress' },
    '@/components/ui/tooltip': Object.fromEntries(['Tooltip', 'TooltipContent', 'TooltipProvider', 'TooltipTrigger'].map(primitive => [primitive, primitive])),
    '@/components/AudioLevelMeter': { SegmentedLevelMeter: 'Meter', levelToDb: n => n },
    '@/lib/theme': {
      applyAccent() {}, applyThemePreference() {}, DEFAULT_ACCENT_ID: 'claw',
      subscribeToSystemTheme: () => () => {},
    },
    '@tauri-apps/api/core': { invoke: async (name, args) => {
      calls.push({ name, label: args?.label, ...(args?.paused !== undefined ? { paused: args.paused } : {}) });
      return name === 'floating_status' ? snapshot : action;
    } },
  }).default;
  const render = () => nodes(hooks.render(Page));
  render();
  return {
    render, calls, unmount: hooks.unmount,
    find: (type, prop, value) => render().find(n => n?.type === type && (!prop || n.props[prop] === value)),
    control: label => render().find(n => n?.props?.label === label && n.props.onClick),
    setAction(value) { action = value; },
    async refresh(next) { snapshot = next; assert.ok(task); const fn = task; task = undefined; fn(); await flush(); },
    hasTimer: () => !!task,
  };
}

test('the panel reads the active clock and both existing meters; Enter bookmarks or saves one typed note', async t => {
  const app = panel(t);
  await flush();
  assert.ok(app.render().includes('00:01:07'));
  for (const label of ['Microphone', 'System audio']) assert.ok(app.render().some(n => n?.props?.label === label));
  assert.equal(app.find('Input'), undefined);
  const openNote = () => app.control('Add note').props.onClick();
  const submit = () => app.find('form').props.onSubmit({ preventDefault() {} });
  openNote();
  await submit();
  assert.equal(app.calls.at(-1).label, 'Review this');
  assert.equal(app.find('Input'), undefined);
  openNote();
  app.find('Input').props.onChange({ target: { value: '  Follow up  ' } });
  const saved = deferred();
  app.setAction(saved.promise);
  const first = submit();
  await submit();
  assert.equal(app.calls.filter(c => c.name === 'floating_note').length, 2, 'one bookmark and one note');
  assert.deepEqual(app.calls.at(-1), { name: 'floating_note', label: 'Follow up' });
  saved.resolve();
  await first;
  assert.equal(app.find('Input'), undefined);
  assert.ok(app.render().includes('Note saved'));
});

test('Stop failures allow retry, external saving disables edits, and a clean finish closes the panel', async t => {
  const app = panel(t);
  await flush();
  const stop = app.control('Stop recording');
  const failure = deferred();
  app.setAction(failure.promise);
  const first = stop.props.onClick();
  await stop.props.onClick();
  assert.equal(app.calls.filter(c => c.name === 'floating_stop').length, 1);
  failure.reject(new Error('device busy'));
  await first;
  assert.equal(app.control('Stop recording').props.disabled, false);
  assert.ok(app.render().includes('Could not stop the recording. Try Stop again.'));
  app.setAction(Promise.resolve());
  await app.refresh({ state: 'saving', seconds: 70, progress: 40, message: 'Finishing live transcription…' });
  assert.equal(app.find('Progress').props.value, 40);
  assert.equal(app.find('form'), undefined);
  assert.equal(app.control('Stop recording'), undefined);
  assert.ok(app.control('Close controls'));
  await app.refresh({ state: 'ended' });
  assert.equal(app.calls.at(-1).name, 'dismiss_floating_controls');
  assert.equal(app.hasTimer(), false);
});

test('closing while paused leaves capture alone and stops polling', async t => {
  const paused = panel(t, { ...live, paused: true });
  await flush();
  assert.ok(paused.render().includes('Paused'));
  await paused.control('Close controls').props.onClick();
  paused.unmount();
  assert.deepEqual(paused.calls.map(c => c.name), ['floating_status', 'dismiss_floating_controls']);
  assert.equal(paused.hasTimer(), false);
});

test('pause and resume target the bound recording and preserve an unfinished note', async t => {
  const app = panel(t);
  await flush();
  app.control('Add note').props.onClick();
  app.find('Input').props.onChange({ target: { value: 'Draft note' } });
  await app.control('Pause recording').props.onClick();
  assert.equal(app.calls.at(-1).paused, true);
  await app.refresh({ ...live, paused: true });
  await app.control('Resume recording').props.onClick();
  assert.equal(app.calls.at(-1).paused, false);
  assert.equal(app.find('Input').props.value, 'Draft note');
  app.find('main').props.onKeyDown({ key: 'Escape', preventDefault() {} });
  assert.equal(app.find('Input'), undefined);
});

test('checking a missing device leaves recording controls and notes usable', async t => {
  const app = panel(t);
  await flush();
  app.control('Add note').props.onClick();
  await app.refresh({ ...live, health: {
    reconnecting: true, disconnected_devices: [{ name: 'Synthetic output', device_type: 'SystemAudio' }],
    warnings: ['Microphone is quiet.'], details: { dropped_samples: 0 },
  } });
  assert.equal(app.find('Input').props.readOnly, false);
  assert.equal(app.control('Pause recording').props.disabled, false);
  assert.equal(app.control('Stop recording').props.disabled, false);
  assert.ok(app.render().includes('Microphone is quiet.'));
  assert.equal(app.find('Button', 'children', 'Retry').props.disabled, true);
  await app.find('form').props.onSubmit({ preventDefault() {} });
  assert.equal(app.calls.at(-1).name, 'floating_note');
});

test('the panel can reopen during a save and offers review and dismissal actions', async t => {
  const app = panel(t, { state: 'saving', seconds: 70, progress: 90, message: 'Saving…' });
  await flush();
  assert.equal(app.find('Progress').props.value, 90);
  await app.refresh({ state: 'review' });
  assert.ok(app.render().includes('This recording needs review.'));
  assert.equal(app.hasTimer(), false);
  assert.ok(app.calls.every(c => c.name === 'floating_status'));
  await app.find('Button', 'children', 'Open ClawScribe').props.onClick();
  assert.equal(app.calls.at(-1).name, 'floating_open_main');
  await app.find('Button', 'children', 'Dismiss').props.onClick();
  assert.equal(app.calls.at(-1).name, 'dismiss_floating_controls');
});

test('app and Windows capabilities grant no plugin access to the controls webview', () => {
  const read = relative => JSON.parse(readFileSync(new URL('../../src-tauri/' + relative, import.meta.url), 'utf8'));
  for (const config of [read('tauri.conf.json'), read('tauri.windows.conf.json')]) {
    for (const capability of config.app?.security?.capabilities ?? []) {
      const labels = [...(capability.windows ?? []), ...(capability.webviews ?? [])];
      for (const pattern of labels) {
        assert.equal(matchesGlob('recording-controls-00000000-0000-4000-8000-000000000001', pattern), false, pattern);
        assert.equal(matchesGlob('call-recording-prompt', pattern), false, pattern);
      }
    }
  }
});
