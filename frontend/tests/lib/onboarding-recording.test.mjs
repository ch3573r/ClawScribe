import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const dialog = Object.fromEntries(
  ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle']
    .map(name => [name, name]),
);

test('audio-only setup completes without a recommendation, model check, or download', async () => {
  const calls = [];
  const { finishRecordingSetup } = loadTsModule('src/lib/onboarding-recording', {
    '@tauri-apps/api/core': { invoke: async (...args) => calls.push(args) },
  });
  await finishRecordingSetup('audio_only', null);
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [
    ['check_first_launch'],
    ['import_legacy_database_if_present'],
    ['complete_onboarding', { mode: 'audio_only', localNotesModel: null }],
  ]);
});

test('an unreadable existing library stops setup before creating an empty library', async () => {
  const calls = [];
  const { finishRecordingSetup } = loadTsModule('src/lib/onboarding-recording', {
    '@tauri-apps/api/core': { invoke: async command => {
      calls.push(command);
      if (command === 'check_first_launch') return true;
      if (command === 'import_legacy_database_if_present') throw new Error('private diagnostic detail');
    } },
  });
  await assert.rejects(finishRecordingSetup('audio_only', null), /existing meeting library/);
  assert.deepEqual(calls, ['check_first_launch', 'import_legacy_database_if_present']);
});

test('an opted-in notes model remains separate from the live transcription choice', async () => {
  const calls = [];
  const { finishRecordingSetup } = loadTsModule('src/lib/onboarding-recording', {
    '@tauri-apps/api/core': { invoke: async (...args) => {
      calls.push(args);
      return args[0] === 'check_first_launch';
    } },
  });
  await finishRecordingSetup('audio_only', 'fixture-model', true);
  assert.deepEqual(calls.slice(0, -1).map(([command]) => command), [
    'check_first_launch', 'import_legacy_database_if_present', 'initialize_fresh_database',
  ]);
  assert.equal(calls.at(-1)[1].mode, 'audio_only');
  assert.equal(calls.at(-1)[1].localNotesModel, 'fixture-model');
  assert.equal(calls.at(-1)[1].automaticNotes, true);
});

test('the setup screen allows audio before downloads, and speech setup never starts a notes download', async t => {
  const hook = createHookHarness();
  t.after(() => hook.unmount());
  const calls = [];
  let downloaded = false;
  const jsx = (type, props) => ({ type, props });
  const { SetupOverviewStep } = loadTsModule('src/components/onboarding/steps/SetupOverviewStep', {
    react: hook.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/switch': { Switch: 'Switch' },
    '@/components/ui/progress': { Progress: 'Progress' },
    '@/components/ui/dialog': dialog,
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '../OnboardingContainer': { OnboardingContainer: 'OnboardingContainer' },
    '@tauri-apps/api/core': {
      invoke: async (command, args) => {
        calls.push([command, args]);
        if (command === 'builtin_ai_get_recommended_model') return 'qwen3.5:2b';
        if (command === 'parakeet_download_model') downloaded = true;
        if (command === 'parakeet_has_available_models') return false;
        if (command === 'parakeet_get_available_models') return downloaded ? [{ name: 'parakeet-tdt-0.6b-v3-int8', status: 'Available' }] : [];
      },
    },
  });
  const choices = [];
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const button = label => nodes(hook.render(() => SetupOverviewStep({
    onChoose: (...args) => choices.push(args), finishing: false, error: null,
  }))).find(node => node.type === 'Button' && node.props.children === label);

  button('Finish setup').props.onClick();
  assert.deepEqual(choices, [['audio_only', null]]);
  await flush();
  assert.deepEqual(calls.map(([command]) => command), ['builtin_ai_get_recommended_model']);

  button('Set up live transcription').props.onClick();
  await flush();
  assert.deepEqual(calls.map(([command]) => command), [
    'builtin_ai_get_recommended_model',
    'parakeet_init', 'parakeet_has_available_models', 'parakeet_download_model', 'parakeet_get_available_models',
  ]);
  button('Finish setup').props.onClick();
  assert.deepEqual(choices[1], ['live', null]);
});

const testConcurrentSetup = settles => test(settles
  ? 'concurrent setup downloads show only their own progress and settle before exit'
  : 'Cancel and finish completes with audio only when a download never resolves', async t => {
  if (!settles) t.mock.timers.enable({ apis: ['setTimeout'] });
  const hooks = createHookHarness(), listeners = new Map(), completed = [];
  const cancelled = [];
  const choices = [];
  const speech = deferred(), notes = deferred();
  const speechEvent = 'parakeet-model-download-progress', notesEvent = 'builtin-ai-download-progress';
  const jsx = (type, props) => ({ type, props });
  const { SetupOverviewStep } = loadTsModule('src/components/onboarding/steps/SetupOverviewStep', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' }, '@/components/ui/progress': { Progress: 'Progress' },
    '@/components/ui/switch': { Switch: 'Switch' },
    '@/components/ui/dialog': dialog,
    '../OnboardingContainer': { OnboardingContainer: 'OnboardingContainer' },
    '@tauri-apps/api/event': { listen: async (event, fn) => {
      listeners.set(event, payload => fn({ payload }));
      return () => { completed.push(event); listeners.delete(event); };
    } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'builtin_ai_get_recommended_model') return 'notes-model';
      if (command === 'parakeet_download_model') return speech.promise;
      if (command === 'builtin_ai_download_model') return notes.promise;
      if (command === 'builtin_ai_cancel_download') cancelled.push(args.modelName);
      if (command === 'parakeet_get_available_models') return [];
      return false;
    } },
  });
  const tree = root => Array.isArray(root) ? root.flatMap(tree) : !root?.props ? [] :
    [root, ...tree(typeof root.type === 'function' ? root.type(root.props) : root.props.children)];
  const render = () => tree(hooks.render(() => SetupOverviewStep({
    onChoose: (...args) => choices.push(args), finishing: false, error: null,
  })));
  const action = label => render().find(node => node.type === 'Button' && node.props.children === label);
  const values = () => render().filter(node => node.type === 'Progress').map(node => node.props.value);
  action('Set up live transcription').props.onClick();
  action('Download local notes model').props.onClick();
  await flush();
  assert.deepEqual(values(), []);
  const emit = (event, model, progress, total_mb = 700) => listeners.get(event)({
    [event === speechEvent ? 'modelName' : 'model']: model, progress, total_mb, status: 'downloading',
  });
  emit(speechEvent, 'another-model', 80);
  emit(notesEvent, 'notes-model', 50, 0);
  assert.deepEqual(values(), []);
  emit(speechEvent, 'parakeet-tdt-0.6b-v3-int8', 29);
  emit(notesEvent, 'notes-model', 34.1);
  assert.deepEqual(values(), [29, 34]);
  assert.equal(action('Finish setup').props.disabled, false);
  speech.resolve();
  await flush();
  assert.equal(listeners.has(speechEvent), false);
  assert.ok(action('Retry speech model setup'), 'download completion alone does not establish readiness');
  assert.deepEqual(values(), [34]);
  action('Cancel notes setup').props.onClick();
  assert.deepEqual(cancelled, [], 'the explicit cancel also requires confirmation');
  action('Keep downloading').props.onClick();
  assert.deepEqual(values(), [34]);
  action('Finish setup').props.onClick();
  assert.deepEqual(cancelled, []);
  assert.deepEqual(choices, []);
  action('Cancel and finish').props.onClick();
  assert.deepEqual(cancelled, ['notes-model']);
  assert.equal(action('Download local notes model').props.disabled, true);
  assert.deepEqual(choices, [], 'setup gives the download time to settle');
  if (settles) {
    notes.resolve();
  } else {
    t.mock.timers.tick(9_999);
    assert.deepEqual(choices, []);
    t.mock.timers.tick(1);
  }
  await flush();
  assert.deepEqual(choices, [['audio_only', null]]);
  assert.equal(action('Finish setup').props.disabled, false);
  assert.equal(action('Download local notes model').props.disabled, !settles);
  hooks.unmount();
  assert.equal(listeners.size, 0);
  assert.deepEqual(completed.sort(), [notesEvent, speechEvent].sort());
});
testConcurrentSetup(true);
testConcurrentSetup(false);

test('welcome distinguishes loading audio preferences from unavailable devices', () => {
  const jsx = (type, props) => ({ type, props });
  let status = 'loading';
  const enabled = [];
  const { WelcomeStep } = loadTsModule('src/components/onboarding/steps/WelcomeStep', {
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' },
    '../OnboardingContainer': { OnboardingContainer: 'OnboardingContainer' },
    '@/contexts/OnboardingContext': { useOnboarding: () => ({ goNext() {} }) },
    '@/hooks/usePlatform': { usePlatform: () => 'windows' },
    '@/contexts/ConfigContext': { useConfig: () => ({
      selectedDevices: { micDevice: null, systemDevice: null },
      devicePreferencesStatus: status,
    }) },
    '@/components/settings/SignalCheck': { SignalCheck: 'SignalCheck' },
    '@/components/settings/useSignalCheck': { useSignalCheck: active => enabled.push(active) },
  });
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root?.props ? [root, ...nodes(root.props.children)] : [];
  const reason = () => nodes(WelcomeStep({ finishing: false, error: null, onRecordAudio() {} }))
    .find(node => node.type === 'SignalCheck').props.unavailableReason;
  assert.equal(reason(), 'Checking audio devices…');
  status = 'error';
  assert.equal(reason(), 'Audio devices are unavailable');
  status = 'ready';
  assert.equal(reason(), null);
  assert.deepEqual(enabled, [false, false, true]);
});
