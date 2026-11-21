import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const devices = [
  { name: 'Desk mic', device_type: 'Input' },
  { name: 'Desk speakers', device_type: 'Output' },
];
const inventory = { devices, defaultMicrophone: 'Desk mic', defaultOutput: null };

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
  assert.equal(liveModelStartFailure('live_model:unknown'), null);
});

function recordingSettings(t, { preferences, save }) {
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  const calls = [];
  const preferred = { micDevice: 'Desk mic (input)', systemDevice: null };
  const jsx = (type, props) => ({ type, props });
  const { RecordingSettings } = loadTsModule('src/components/RecordingSettings', {
    react: hook.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'get_recording_preferences') return preferences();
      if (command === 'set_recording_preferences') return save(args.preferences);
    } },
    '@/components/ui/switch': { Switch: 'Switch' },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/select': Object.fromEntries(['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'].map(name => [name, name])),
    '@/components/ui/settings-layout': Object.fromEntries(['Meta', 'SettingsCard', 'SettingsColumns', 'SettingsRow'].map(name => [name, name])),
    '@/components/settings/AudioDevicesCard': { AudioDevicesCard: 'AudioDevicesCard' },
    '@/components/settings/CallDetectionSettings': { CallDetectionSettings: 'CallDetectionSettings' },
    '@/components/settings/SignalCheck': { SignalCheck: 'SignalCheck' },
    '@/components/settings/useAudioLevelFeed': { useAudioLevelFeed: () => ({}) },
    '@/lib/sourceAttribution': { setSourceAttribution: () => calls.push(['setSourceAttribution']) },
    '@/lib/analytics': { default: { track: () => {} }, __esModule: true },
    '@/lib/behaviorSettings': { loadBehaviorSettings: async () => ({ showRecordingNotification: true }) },
    '@/contexts/ConfigContext': {
      useConfig: () => ({ selectedDevices: preferred, setSelectedDevices }),
    },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ recordingMode: 'audio_only' }) },
    sonner: { toast: { error: message => calls.push(['error', message]), success: () => {} } },
  });
  function setSelectedDevices(next) { calls.push(['publishDevices', next]); }
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const find = predicate => nodes(hook.render(() => RecordingSettings({}))).find(predicate);
  return { calls, find, remount: () => hook.reset() };
}

const recordingPreferences = {
  save_folder: 'Recordings', auto_save: false, file_format: 'mp4',
  preferred_mic_device: 'Desk mic (input)', preferred_system_device: null,
};

test('Recording settings keeps audio-only saving read-only and rolls back rejected device saves', async t => {
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
  const oldError = console.error;
  console.error = () => {};
  try {
    await devices().onDeviceChange({ micDevice: 'Other mic (input)', systemDevice: null });
  } finally {
    console.error = oldError;
  }
  assert.deepEqual(calls.map(([command]) => command), ['set_recording_preferences', 'error']);
  assert.equal(devices().selectedDevices.micDevice, 'Desk mic (input)');
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

test('Home labels native live and hosted file transcription separately and can recheck disconnected devices', t => {
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  let rechecks = 0;
  const state = { status: 'ready', data: inventory, refresh: () => { rechecks++; } };
  const preferred = { micDevice: null, systemDevice: null };
  const jsx = (type, props) => ({ type, props });
  const stub = (...names) => Object.fromEntries(names.map(name => [name, name]));
  const { HomeDashboard } = loadTsModule('src/components/HomeDashboard', {
    react: hook.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@/components/ui/button': stub('Button'),
    '@/components/RecordingControls': stub('RecordingControls'),
    '@/components/RecordingModeSelect': stub('RecordingModeSelect'),
    '@/components/UpcomingMeetings': stub('UpcomingMeetings'),
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ meetings: [] }) },
    '@/contexts/ConfigContext': { useConfig: () => ({
      selectedDevices: preferred, selectedLanguage: 'auto', audioDeviceInventory: state, devicePreferencesStatus: 'ready',
    }) },
    '@/contexts/ImportDialogContext': { useImportDialog: () => ({}) },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ recordingMode: 'live' }), RecordingStatus: { ERROR: 'error' },
    },
    '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ enabled: true, loaded: true }) },
    'next/navigation': { useRouter: () => ({ push() {} }) },
    'lucide-react': stub('ArrowRight', 'AudioLines', 'ChevronRight', 'Clock3', 'FileText', 'Languages', 'Mic', 'Settings2', 'RefreshCw', 'Sparkles', 'Upload', 'Volume2'),
  });
  const render = () => hook.render(() => HomeDashboard({
    liveReadiness: { data: { provider: 'parakeet', hostedBatchProvider: 'mai-transcribe', modelIssue: null }, error: false },
  }));
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const text = root => Array.isArray(root) ? root.map(text).join('') :
    typeof root === 'string' ? root : root && typeof root === 'object' ? text(root.props.children) : '';

  const home = render();
  assert.match(text(home), /Live transcriptionParakeet/);
  assert.match(text(home), /Microsoft AI · Hosted file transcription/);
  assert.match(text(home), /Only your microphone will be recorded/);
  assert.equal(nodes(home).find(node => node.type === 'RecordingControls').props.isRecordingDisabled, false);

  preferred.micDevice = 'Unplugged mic (input)';
  const stale = render();
  assert.match(text(stale), /Preferred microphone unavailable/);
  assert.equal(nodes(stale).find(node => node.type === 'RecordingControls').props.isRecordingDisabled, true);
  nodes(stale).find(node => node.type === 'Button' && /Recheck devices/.test(text(node))).props.onClick();
  assert.equal(rechecks, 2);

  state.status = 'error';
  state.data = null;
  const failed = render();
  assert.match(text(failed), /Could not check audio devices/);
  assert.doesNotMatch(text(failed), /Ready to record/);
});
