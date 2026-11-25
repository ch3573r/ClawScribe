import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const devices = [
  { name: 'Desk mic', device_type: 'Input' },
  { name: 'Desk speakers', device_type: 'Output' },
];
const inventory = { devices, defaultMicrophone: 'Desk mic', defaultOutput: null };

test('focus refreshes device and live readiness without hiding the previous results', async t => {
  const previousWindow = globalThis.window;
  const window = globalThis.window = new EventTarget();
  const deviceHooks = createHookHarness();
  const liveHooks = createHookHarness();
  t.after(() => {
    deviceHooks.unmount();
    liveHooks.unmount();
    globalThis.window = previousWindow;
  });
  const deviceRefresh = deferred();
  const liveRefresh = deferred();
  const ready = { provider: 'parakeet', hostedBatchProvider: null, modelIssue: null };
  let deviceReads = 0, liveReads = 0;
  const { useAudioDeviceInventory } = loadTsModule('src/hooks/useAudioDeviceInventory', {
    react: deviceHooks.react,
    '@tauri-apps/api/core': { invoke: () => ++deviceReads === 1 ? Promise.resolve(inventory) : deviceRefresh.promise },
  });
  const { useLiveTranscriptionReadiness } = loadTsModule('src/hooks/useLiveTranscriptionReadiness', {
    react: liveHooks.react,
    '@tauri-apps/api/core': { invoke: () => ++liveReads === 1 ? Promise.resolve(ready) : liveRefresh.promise },
    '@tauri-apps/api/event': { listen: async () => () => {} },
  });
  const renderDevice = () => deviceHooks.render(useAudioDeviceInventory);
  const renderLive = (model = 'default') => liveHooks.render(() => useLiveTranscriptionReadiness(true, 'parakeet', model));
  renderLive();
  await renderDevice().refresh();
  await flush();
  assert.equal(renderDevice().data, inventory);
  assert.equal(renderLive().data, ready);

  window.dispatchEvent(new Event('focus'));
  assert.equal(deviceReads, 2);
  assert.equal(liveReads, 2);
  assert.equal(renderDevice().status, 'ready');
  assert.equal(renderDevice().data, inventory);
  assert.equal(renderLive().data, ready);
  assert.equal(renderLive('different-model').data, null, 'another model needs its own result');

  deviceRefresh.resolve({ ...inventory, devices: [] });
  liveRefresh.reject(new Error('Fixture readiness failure'));
  await flush();
  assert.equal(renderDevice().data.devices.length, 0);
  assert.equal(renderLive('different-model').error, true);
});

test('a preferred device can disappear without changing the saved preference', () => {
  const { audioSourceReadiness } = loadTsModule('src/lib/audioDevicePreferences');
  assert.equal(audioSourceReadiness(inventory, 'Input', 'Headset (input)'), 'preferred_unavailable');
  assert.equal(audioSourceReadiness(inventory, 'Input', 'Desk mic (input)'), 'available');
  assert.equal(audioSourceReadiness(inventory, 'Input', null), 'available');
  assert.equal(audioSourceReadiness(inventory, 'Output', null), 'default_unavailable');
  assert.equal(audioSourceReadiness(inventory, 'Output', 'Desk speakers (output)'), 'available');
});

test('audio-only and unknown modes cannot edit the live save-audio preference', () => {
  const { canEditLiveAudioSaving } = loadTsModule('src/lib/audioDevicePreferences');
  assert.equal(canEditLiveAudioSaving('audio_only'), false);
  assert.equal(canEditLiveAudioSaving(null), false);
  assert.equal(canEditLiveAudioSaving('live'), true);
});

test('start remediation uses native error codes, not words in raw error text', () => {
  const { liveModelStartFailure } = loadTsModule('src/lib/start-error');
  assert.equal(liveModelStartFailure('Could not open model-device (input)'), null);
  assert.equal(liveModelStartFailure('live_model:missing'), 'missing');
  assert.equal(liveModelStartFailure('live_model:load_failed'), 'load_failed');
  assert.equal(liveModelStartFailure('live_model:unsupported_provider'), 'unsupported_provider');
  assert.equal(liveModelStartFailure('live_model:vulkan_runtime_missing'), 'vulkan_runtime_missing');
  assert.equal(liveModelStartFailure('live_model:unknown'), null);
});

function recordingSettings(t, { preferences, save }) {
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  const calls = [];
  let behavior = { showRecordingNotification: true, showConversationMetrics: true };
  const preferred = { micDevice: 'Desk mic (input)', systemDevice: null };
  const jsx = (type, props) => ({ type, props });
  const { RecordingSettings } = loadTsModule('src/components/RecordingSettings', {
    react: { ...hook.react, useSyncExternalStore: (_subscribe, getSnapshot) => getSnapshot() },
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'get_recording_preferences') return preferences();
      if (command === 'set_recording_preferences') return save(args.preferences);
      if (command === 'set_conversation_metrics_preference') {
        behavior = { ...behavior, showConversationMetrics: args.enabled };
      }
    } },
    '@/components/ui/switch': { Switch: 'Switch' },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/select': Object.fromEntries(['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'].map(name => [name, name])),
    '@/components/ui/settings-layout': Object.fromEntries(['Meta', 'SettingsCard', 'SettingsColumns', 'SettingsRow'].map(name => [name, name])),
    '@/components/settings/AudioDevicesCard': { AudioDevicesCard: 'AudioDevicesCard' },
    '@/components/settings/CallDetectionSettings': { CallDetectionSettings: 'CallDetectionSettings' },
    '@/components/settings/SignalCheck': { SignalCheck: 'SignalCheck', MicrophoneCheck: 'MicrophoneCheck' },
    '@/components/settings/useSignalCheck': { useSignalCheck: () => ({}) },
    '@/components/settings/useAudioLevelFeed': { useAudioLevelFeed: () => ({}) },
    '@/lib/sourceAttribution': { setSourceAttribution: () => calls.push(['setSourceAttribution']) },
    '@/lib/analytics': { default: { track: () => {} }, __esModule: true },
    '@/lib/behaviorSettings': {
      loadBehaviorSettings: async () => behavior,
      getBehaviorSettings: () => behavior,
      subscribeBehaviorSettings: () => () => {},
      refreshBehaviorSettings: async () => behavior,
    },
    '@/contexts/ConfigContext': {
      useConfig: () => ({ selectedDevices: preferred, devicePreferencesStatus: 'ready', saveSelectedDevices }),
    },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ recordingMode: 'audio_only' }) },
    sonner: { toast: { error: message => calls.push(['error', message]), success: () => {} } },
  });
  async function saveSelectedDevices(next) { calls.push(['saveSelectedDevices', next]); }
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const find = predicate => nodes(hook.render(() => RecordingSettings({}))).find(predicate);
  return { calls, find, remount: () => hook.reset() };
}

const recordingPreferences = {
  save_folder: 'Recordings', auto_save: false, file_format: 'mp4',
  preferred_mic_device: 'Desk mic (input)', preferred_system_device: null,
};

test('Recording settings keeps audio-only saving read-only and uses the shared device writer', async t => {
  const { calls, find } = recordingSettings(t, {
    preferences: () => recordingPreferences,
    save() { throw new Error('could not save'); },
  });
  find(() => false);
  await flush();
  calls.length = 0;

  const saveAudio = find(node => node.type === 'Switch' && node.props.id === 'save-audio');
  assert.equal(saveAudio.props.checked, true);
  assert.equal(saveAudio.props.disabled, true);
  await saveAudio.props.onCheckedChange(false);
  assert.deepEqual(calls, []);

  const devices = () => find(node => node.type === 'AudioDevicesCard').props;
  await devices().onDeviceChange({ micDevice: 'Other mic (input)', systemDevice: null });
  assert.deepEqual(calls.map(([command]) => command), ['saveSelectedDevices']);
  assert.equal(devices().selectedDevices.micDevice, 'Desk mic (input)');
});

test('conversation metrics can be hidden in Recording settings', async t => {
  const { calls, find, remount } = recordingSettings(t, {
    preferences: () => recordingPreferences,
    save() { assert.fail('Conversation visibility uses the behavior store'); },
  });
  const toggle = () => find(node => node.type === 'Switch' && node.props.id === 'show-conversation-metrics');
  toggle();
  await flush();
  assert.equal(toggle().props.checked, true);
  await toggle().props.onCheckedChange(false);
  assert.equal(calls.filter(([command]) => command === 'set_conversation_metrics_preference').at(-1)[1].enabled, false);
  remount();
  toggle();
  await flush();
  assert.equal(toggle().props.checked, false);
});

test('automatic floating controls can be turned off in Recording settings and back on after reopening', async t => {
  let stored = { ...recordingPreferences, auto_open_floating_controls: true };
  const { find, remount } = recordingSettings(t, {
    preferences: () => ({ ...stored }),
    save(next) { stored = { ...next }; },
  });
  const toggle = () => find(node => node.type === 'Switch' && node.props.id === 'auto-open-floating-controls');
  toggle();
  await flush();
  assert.ok(toggle(), 'Recording settings must offer the automatic floating controls switch');
  assert.equal(toggle().props.checked, true);
  await toggle().props.onCheckedChange(false);
  assert.deepEqual(stored, { ...recordingPreferences, auto_open_floating_controls: false });
  remount();
  toggle();
  await flush();
  assert.equal(toggle().props.checked, false);
  await toggle().props.onCheckedChange(true);
  assert.equal(stored.auto_open_floating_controls, true);
});

test('Home can change a disconnected device without leaving the signal path', async t => {
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  let rechecks = 0;
  const state = { status: 'ready', data: inventory, refresh: () => { rechecks++; } };
  const preferred = { micDevice: null, systemDevice: null };
  const liveReadiness = { data: { provider: 'parakeet', hostedBatchProvider: 'mai-transcribe', modelIssue: null }, error: false };
  let nextMeetingTitle = '';
  const savedDevices = [];
  const jsx = (type, props) => ({ type, props });
  const stub = (...names) => Object.fromEntries(names.map(name => [name, name]));
  const { HomeDashboard } = loadTsModule('src/components/HomeDashboard', {
    react: { ...hook.react, useSyncExternalStore: (_subscribe, getSnapshot) => getSnapshot() },
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@/components/ui/button': stub('Button'),
    '@/components/ui/input': stub('Input'),
    '@/components/AudioDeviceSelect': stub('AudioDeviceSelect'),
    '@/components/RecordingControls': stub('RecordingControls'),
    '@/components/RecordingModeSelect': stub('RecordingModeSelect'),
    '@/components/UpcomingMeetings': stub('UpcomingMeetings'),
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
      meetings: [], liveReadiness, nextMeetingTitle, setNextMeetingTitle: title => { nextMeetingTitle = title; },
      recordingActionReason: state.status === 'error' ? 'Could not check audio devices'
        : preferred.micDevice === 'Unplugged mic (input)' ? 'Preferred microphone unavailable' : null,
    }) },
    '@/contexts/ConfigContext': { useConfig: () => ({
      selectedDevices: preferred, selectedLanguage: 'auto', audioDeviceInventory: state, devicePreferencesStatus: 'ready',
      saveSelectedDevices: async devices => { savedDevices.push(devices); Object.assign(preferred, devices); },
    }) },
    '@/contexts/ImportDialogContext': { useImportDialog: () => ({}) },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ recordingMode: 'live' }), RecordingStatus: { ERROR: 'error' },
    },
    '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ enabled: true, loaded: true }) },
    'next/navigation': { useRouter: () => ({ push() {} }) },
    'lucide-react': stub('ArrowRight', 'AudioLines', 'ChevronRight', 'Clock3', 'FileText', 'Languages', 'Mic', 'Settings2', 'RefreshCw', 'Sparkles', 'Upload', 'Volume2'),
  });
  const render = () => hook.render(() => HomeDashboard({}));
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const text = root => Array.isArray(root) ? root.map(text).join('') :
    typeof root === 'string' ? root : root && typeof root === 'object' ? text(root.props.children) : '';

  const home = render();
  assert.match(text(home), /Live transcriptionParakeet/);
  assert.match(text(home), /Azure Speech · Hosted file transcription/);
  assert.match(text(home), /Only your microphone will be recorded/);
  assert.equal(nodes(home).find(node => node.type === 'RecordingControls').props.isRecordingDisabled, false);
  const titleInput = () => nodes(render()).find(node => node.props.id === 'next-meeting-title');
  titleInput().props.onChange({ target: { value: 'Synthetic\tplanning\u0085meeting' } });
  assert.equal(titleInput().props.value, 'Synthetic planning meeting');

  preferred.micDevice = 'Unplugged mic (input)';
  const stale = render();
  assert.match(text(stale), /Preferred microphone unavailable/);
  assert.equal(nodes(stale).find(node => node.type === 'RecordingControls').props.isRecordingDisabled, true);
  const microphone = nodes(stale).find(node => node.type === 'AudioDeviceSelect' && node.props.label === 'Microphone');
  assert.equal(microphone.props.value, 'Unplugged mic (input)');
  await microphone.props.onChange('Desk mic (input)');
  assert.equal(JSON.stringify(savedDevices), JSON.stringify([{ micDevice: 'Desk mic (input)', systemDevice: null }]));
  assert.equal(nodes(render()).find(node => node.type === 'RecordingControls').props.isRecordingDisabled, false);
  nodes(stale).find(node => node.type === 'Button' && /Recheck devices/.test(text(node))).props.onClick();
  assert.equal(rechecks, 1);

  state.status = 'error';
  state.data = null;
  const failed = render();
  assert.match(text(failed), /Could not check audio devices/);
  assert.doesNotMatch(text(failed), /Ready to record/);
});

test('the shared preference owner ignores stale reads and keeps the prior devices if saving fails', async t => {
  const hook = createHookHarness();
  const initial = deferred();
  const saves = [];
  const errors = [];
  let changeDevices;
  let unsubscribed = false;
  let saving = deferred();
  const { useRecordingDevicePreferences } = loadTsModule('src/hooks/useRecordingDevicePreferences.ts', {
    react: hook.react,
    '@tauri-apps/api/event': { listen: async (name, handler) => {
      assert.equal(name, 'recording-device-preferences-changed');
      changeDevices = payload => handler({ payload });
      return () => { unsubscribed = true; };
    } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      assert.equal(command, 'set_recording_device_preferences');
      saves.push(args.preferences);
      return saving.promise;
    } },
    '@/services/configService': { configService: { getRecordingPreferences: () => initial.promise } },
    sonner: { toast: { error: message => errors.push(message) } },
  });
  const render = () => hook.render(useRecordingDevicePreferences);
  render();
  await flush();
  changeDevices({ preferred_mic_device: 'Shared mic (input)', preferred_system_device: null });
  initial.resolve({ preferred_mic_device: 'Old mic (input)', preferred_system_device: null });
  await flush();
  assert.equal(render().selectedDevices.micDevice, 'Shared mic (input)');

  const devices = { micDevice: 'New mic (input)', systemDevice: null };
  const first = render().saveSelectedDevices(devices);
  assert.equal(render().isSavingDevicePreferences, true);
  await render().saveSelectedDevices(devices);
  saving.reject(new Error('Synthetic write error'));
  await first;
  assert.equal(render().selectedDevices.micDevice, 'Shared mic (input)');
  assert.deepEqual(errors, ['Could not save audio device selection']);
  assert.equal(saves.length, 1);

  saving = deferred();
  const retry = render().saveSelectedDevices(devices);
  saving.resolve({ preferred_mic_device: 'New mic (input)', preferred_system_device: null });
  await retry;
  assert.equal(render().selectedDevices.micDevice, 'New mic (input)');
  assert.equal(render().isSavingDevicePreferences, false);
  hook.unmount();
  await flush();
  assert.equal(unsubscribed, true);
});
