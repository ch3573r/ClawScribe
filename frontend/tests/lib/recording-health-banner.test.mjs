import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const messages = banner => Array.from(banner.props.children[1].props.children, item => item.props.children);

test('the global saving banner scopes the next start and cancellation to the displayed session', async () => {
  const commands = [], notices = [];
  const state = { isStopping: true, sessionId: 'stopping-session', statusMessage: 'Saving the recording to your library…' };
  const { default: MainContent } = loadTsModule('src/components/MainContent/index.tsx', {
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ sidebarOffset: 40, isSidebarResizing: false }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => state },
    '@/components/RecordingHealthBanner': { RecordingHealthBanner: 'Health' },
    '@/components/ui/button': { Button: 'button' },
    '@tauri-apps/api/core': { invoke: async (name, args) => { commands.push([name, { ...args }]); return true; } },
    sonner: { toast: { info: (...args) => notices.push(args) } },
  });
  const banner = () => MainContent({ children: 'current page' }).props.children[1].type();
  const [stage, start] = banner().props.children;
  assert.equal(stage.props.children, state.statusMessage);
  await start.props.onClick();
  state.sessionId = 'replacement';
  notices[0][1].action.onClick();
  assert.deepEqual(commands, [
    ['queue_recording_after_save', { sessionId: 'stopping-session', enabled: true }],
    ['queue_recording_after_save', { sessionId: 'stopping-session', enabled: false }],
  ]);
  state.isStopping = false;
  assert.equal(banner(), null);
});

test('repeated segment warnings keep the recording banner visible', () => {
  const hooks = createHookHarness();
  const listeners = new Map();
  const { RecordingHealthBanner } = loadTsModule(fileURLToPath(new URL('../../src/components/RecordingHealthBanner.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    'lucide-react': { AlertTriangle: 'Icon' },
  });
  assert.equal(hooks.render(RecordingHealthBanner), null);
  listeners.get('transcription-error')({ payload: {} });
  for (let i = 0; i < 10; i++) listeners.get('transcription-warning')({ payload: 'Synthetic failure' });
  const banner = hooks.render(RecordingHealthBanner);
  assert.equal(banner.props.role, 'alert');
  assert.equal(messages(banner).length, 2);
  assert.match(messages(banner)[0], /Live transcription failed/);
  assert.match(messages(banner)[1], /Live transcription is incomplete/);
  listeners.get('recording-started')({});
  assert.equal(hooks.render(RecordingHealthBanner), null);
  hooks.unmount();
});

function createWarningView() {
  const hooks = createHookHarness();
  const listeners = new Map();
  const { RecordingHealthBanner } = loadTsModule(fileURLToPath(new URL('../../src/components/RecordingHealthBanner.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    'lucide-react': { AlertTriangle: 'Icon' },
  });
  return { ...hooks, listeners, render: () => hooks.render(RecordingHealthBanner) };
}

test('late system audio clears the matching recording warning', () => {
  const view = createWarningView();
  const systemWarning = 'No system-audio samples have arrived yet.';
  try {
    view.render();
    view.listeners.get('recording-warning')({ payload: systemWarning });
    assert.deepEqual(messages(view.render()), [systemWarning]);
    view.listeners.get('recording-warning-cleared')?.({ payload: systemWarning });
    assert.equal(view.render(), null, 'Recovered system audio must remove its stale warning');
  } finally {
    view.unmount();
  }
});

test('system audio recovery cannot hide a newer capture warning', () => {
  const view = createWarningView();
  const systemWarning = 'No system-audio samples have arrived yet.';
  const captureWarning = 'Some audio could not be saved.';
  try {
    view.render();
    view.listeners.get('recording-warning')({ payload: systemWarning });
    view.listeners.get('recording-warning')({ payload: captureWarning });
    view.listeners.get('recording-warning-cleared')?.({ payload: systemWarning });
    assert.deepEqual(messages(view.render()), [captureWarning]);
  } finally {
    view.unmount();
  }
});

test('clearing the most recent microphone warning still shows the earlier system warning', () => {
  const view = createWarningView();
  try {
    view.render();
    view.listeners.get('recording-warning')({ payload: 'System samples missing' });
    view.listeners.get('recording-warning')({ payload: 'Microphone samples stopped' });
    assert.deepEqual(messages(view.render()), ['System samples missing', 'Microphone samples stopped']);
    view.listeners.get('recording-warning-cleared')({ payload: 'Microphone samples stopped' });
    assert.deepEqual(messages(view.render()), ['System samples missing']);
  } finally { view.unmount(); }
});
