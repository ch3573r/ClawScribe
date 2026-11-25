import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });

function setup(t, meetingsReply, recording = { isRecording: false }, recordingMocks = {}) {
  const previousWindow = globalThis.window;
  globalThis.window = {
    innerWidth: 1600,
    localStorage: { getItem: () => null, setItem() {} },
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    addEventListener() {}, removeEventListener() {},
  };
  t.after(() => { globalThis.window = previousWindow; });
  const hooks = createHookHarness();
  const noop = () => {};
  const { SidebarProvider } = loadTsModule('src/components/Sidebar/SidebarProvider.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'next/navigation': { usePathname: () => '/settings', useRouter: () => ({ push: noop }) },
    '@/lib/analytics': { __esModule: true, default: { trackBackendConnection() {}, trackButtonClick() {} } },
    '@tauri-apps/api/core': { invoke: async command => {
      if (command === 'api_get_meetings') return meetingsReply();
      if (command === 'list_meeting_tags') return [];
      return [];
    } },
    '@tauri-apps/api/event': { listen: async () => noop },
    '@/contexts/RecordingStateContext': { useRecordingState: () => recording },
    '@/contexts/ConfigContext': { useConfig: () => ({
      audioDeviceInventory: { data: null, status: 'checking', refresh() {} },
      selectedDevices: {}, transcriptModelConfig: {},
    }) },
    '@/hooks/useRecordingStart': { useStartRecordingSession: () => () => {} },
    '@/hooks/useLiveTranscriptionReadiness': { useLiveTranscriptionReadiness: () => ({}) },
    ...recordingMocks,
  });
  t.after(() => hooks.unmount());
  const value = async () => {
    // Render, let the server address and the first load settle, render again.
    let tree = hooks.render(() => SidebarProvider({ children: null }));
    for (let i = 0; i < 3; i++) { await flush(); tree = hooks.render(() => SidebarProvider({ children: null })); }
    return tree.props.value;
  };
  return { value, render: () => hooks.render(() => SidebarProvider({ children: null })).props.value, hooks };
}

test('Sidebar starts on the current route and ignores a repeated or blocked Start', async t => {
  const recording = { isRecording: false, recordingMode: 'audio_only' };
  const attempts = [];
  const pending = deferred();
  const config = {
    audioDeviceInventory: { data: null, status: 'checking', refresh() {} },
    selectedDevices: { micDevice: null, systemDevice: null }, devicePreferencesStatus: 'ready', transcriptModelConfig: {},
  };
  const app = setup(t, () => [], recording, {
    'next/navigation': { usePathname: () => '/meeting-details', useRouter() { throw new Error('Start must not use navigation'); } },
    '@/contexts/ConfigContext': { useConfig: () => config },
    '@/hooks/useRecordingStart': { useStartRecordingSession: () => (...args) => {
      attempts.push(args);
      return pending.promise;
    } },
  });
  let sidebar = await app.value();
  assert.equal(sidebar.recordingActionReason, 'Checking audio devices');
  sidebar.handleRecordingToggle();
  config.audioDeviceInventory = {
    status: 'ready', data: { devices: [], defaultMicrophone: 'Synthetic mic', defaultOutput: null }, refresh() {},
  };
  sidebar = app.render();
  assert.equal(sidebar.recordingActionReason, null);
  sidebar.setNextMeetingTitle('Next meeting');
  sidebar = app.render();
  sidebar.handleRecordingToggle();
  sidebar.handleRecordingToggle();
  assert.deepEqual(attempts, [['sidebar_direct', undefined, 'Next meeting']]);
  recording.isStarting = true;
  assert.equal(app.render().recordingActionReason, 'Starting recording…');
  pending.resolve('native-session');
  await flush();
  recording.isStopping = true;
  assert.equal(app.render().recordingActionReason, 'Saving recording…');
  assert.equal(app.render().nextMeetingTitle, '');
});

test('a library that cannot be loaded is an error, not an empty library', async t => {
  const app = setup(t, () => { throw new Error('Synthetic failure'); });
  const state = await app.value();
  assert.equal(state.meetingsError, 'Could not load your meetings.');
  assert.equal(state.meetings.length, 0);
});

test('a successful load shows the meetings and no error', async t => {
  const app = setup(t, () => [{ id: 'a', title: 'First' }]);
  const state = await app.value();
  assert.equal(state.meetingsError, null);
  assert.deepEqual(state.meetings.map(meeting => meeting.title), ['First']);
});

test('retrying after a failure loads the library and clears the error', async t => {
  let fail = true;
  const app = setup(t, () => { if (fail) throw new Error('Synthetic failure'); return [{ id: 'a', title: 'First' }]; });
  const failed = await app.value();
  assert.ok(failed.meetingsError);
  fail = false;
  await failed.refetchMeetings();
  const state = app.render();
  assert.equal(state.meetingsError, null);
  assert.deepEqual(state.meetings.map(meeting => meeting.title), ['First']);
});

test('a later failure keeps the meetings already loaded and reports the error', async t => {
  let fail = false;
  const app = setup(t, () => { if (fail) throw new Error('Synthetic failure'); return [{ id: 'a', title: 'First' }]; });
  const loaded = await app.value();
  fail = true;
  await loaded.refetchMeetings();
  const state = app.render();
  assert.ok(state.meetingsError);
  assert.deepEqual(state.meetings.map(meeting => meeting.title), ['First'], 'the stale list is better than an empty one');
});

test('the library reports that a reload is running, so Retry can be disabled, and ends it on failure too', async t => {
  let release;
  let reply = () => { throw new Error('Synthetic failure'); };
  const app = setup(t, () => reply());
  const failed = await app.value();
  assert.equal(failed.meetingsLoading, false, 'idle after the failed first load');
  reply = () => new Promise((resolve, reject) => { release = { resolve, reject }; });
  const reloading = failed.refetchMeetings();
  assert.equal(app.render().meetingsLoading, true, 'Retry shows as loading while the reload runs');
  release.reject(new Error('Still failing'));
  await reloading;
  const again = app.render();
  assert.equal(again.meetingsLoading, false, 'a failed retry leaves loading');
  assert.ok(again.meetingsError);
  reply = () => new Promise(resolve => { release = { resolve }; });
  const third = again.refetchMeetings();
  assert.equal(app.render().meetingsLoading, true);
  release.resolve([{ id: 'a', title: 'First' }]);
  await third;
  const done = app.render();
  assert.equal(done.meetingsLoading, false);
  assert.equal(done.meetingsError, null);
});

test('an older refresh cannot overwrite a newer result or report its failure', async t => {
  const old = deferred();
  let reply = () => [{ id: 'a', title: 'First' }];
  const app = setup(t, () => reply());
  const state = await app.value();
  reply = () => old.promise;
  const older = state.refetchMeetings();
  reply = () => [{ id: 'a', title: 'New' }];
  await state.refetchMeetings();
  assert.equal(app.render().meetingsLoading, false);
  old.reject(new Error('Obsolete result'));
  await older;
  assert.equal(app.render().meetingsError, null);
  assert.equal(app.render().meetings[0].title, 'New');
});

test('a load begun before rename and deletion cannot revert those changes', async t => {
  const old = deferred();
  const before = [{ id: 'a', title: 'First' }, { id: 'b', title: 'Second' }];
  let reply = () => before;
  const app = setup(t, () => reply());
  const state = await app.value();
  reply = () => old.promise;
  const loading = state.refetchMeetings();
  reply = () => [{ id: 'a', title: 'Renamed' }];
  state.setMeetings(rows => rows.map(row => row.id === 'a' ? { ...row, title: 'Renamed' } : row));
  state.setMeetings(rows => rows.filter(row => row.id !== 'b'));
  old.resolve(before);
  await loading;
  await flush();
  assert.deepEqual(app.render().meetings.map(({ id, title }) => [id, title]), [['a', 'Renamed']]);
});
