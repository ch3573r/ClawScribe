import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

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

function health(warnings = []) {
  return {
    warnings, disconnected_devices: [], capture_incomplete: false, reconnecting: false,
    destination_free_bytes: null, scratch_free_bytes: null, transcription: null,
    details: { dropped_samples: 0, system_silence_periods: 0, backend: null, unfinished_segments: 0, empty_live_transcript: null },
  };
}

function createWarningView(t, invoke) {
  const hooks = createHookHarness();
  const state = { sessionId: 'session-a', isRecording: true, isStopping: false };
  let nextPoll;
  t.mock.method(globalThis, 'setTimeout', callback => { nextPoll = callback; return 1; });
  t.mock.method(globalThis, 'clearTimeout', () => { nextPoll = undefined; });
  const { RecordingHealthBanner } = loadTsModule('src/components/RecordingHealthBanner.tsx', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke },
    '@/contexts/RecordingStateContext': { useRecordingState: () => state },
    '@/components/ui/button': { Button: 'button' },
  });
  t.after(hooks.unmount);
  return {
    state,
    async poll() { nextPoll(); await flush(); },
    render: () => hooks.render(RecordingHealthBanner),
  };
}

function content(node) {
  if (Array.isArray(node)) return node.flatMap(content);
  if (typeof node === 'string') return [node];
  return node?.props ? content(node.props.children) : [];
}

test('quiet periods remain neutral and a restored window retains actionable warnings', async t => {
  const gap = 'Samples were dropped.';
  let snapshot = health();
  snapshot.details.system_silence_periods = 3;
  const view = createWarningView(t, async (command, args) => {
    assert.equal(command, 'get_recording_health');
    assert.equal(args.sessionId, 'session-a');
    return snapshot;
  });
  assert.equal(view.render(), null);
  await flush();
  assert.equal(view.render(), null);
  snapshot = health([gap]);
  snapshot.details.backend = 'Nemotron · CPU fallback encoder, DirectML decoder';
  await view.poll();
  assert.ok(content(view.render()).includes(gap));
  assert.ok(content(view.render()).some(message => message.includes('Check acceleration settings')));
});

test('a replacement recording never displays the prior session warnings', async t => {
  const old = deferred();
  const view = createWarningView(t, async (_command, args) => args.sessionId === 'session-a' ? old.promise : health(['New warning']));
  view.render();
  view.state.sessionId = 'session-b';
  view.render();
  old.resolve(health(['Old warning']));
  await flush();
  assert.ok(content(view.render()).includes('New warning'));
  assert.ok(!content(view.render()).includes('Old warning'));
});

test('a failed manual reconnect leaves Retry bound to the current missing source', async t => {
  const snapshot = health();
  snapshot.disconnected_devices.push({ name: 'Synthetic output', device_type: 'SystemAudio' });
  const attempts = [];
  const view = createWarningView(t, async (command, args) => {
    if (command === 'get_recording_health') return snapshot;
    assert.equal(command, 'attempt_device_reconnect');
    attempts.push({ ...args });
    return { reconnected: false };
  });
  view.render();
  await flush();
  const retry = () => view.render().props.children.flat(Infinity).find(node => node?.props?.children?.[1]?.type === 'button')
    .props.children[1].props.onClick();
  retry();
  await flush();
  assert.ok(content(view.render()).includes('Audio device still unavailable. Check its connection and try again.'));
  retry();
  await flush();
  assert.deepEqual(attempts, Array(2).fill({ sessionId: 'session-a', deviceName: 'Synthetic output', deviceType: 'SystemAudio' }));
});
