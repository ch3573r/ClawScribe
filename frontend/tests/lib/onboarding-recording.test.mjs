import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

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

test('an opted-in notes model remains separate from the live transcription choice', async () => {
  const calls = [];
  const { finishRecordingSetup } = loadTsModule('src/lib/onboarding-recording', {
    '@tauri-apps/api/core': { invoke: async (...args) => {
      calls.push(args);
      return args[0] === 'check_first_launch';
    } },
  });
  await finishRecordingSetup('audio_only', 'fixture-model');
  assert.deepEqual(calls.slice(0, -1).map(([command]) => command), [
    'check_first_launch', 'import_legacy_database_if_present', 'initialize_fresh_database',
  ]);
  assert.equal(calls.at(-1)[1].mode, 'audio_only');
  assert.equal(calls.at(-1)[1].localNotesModel, 'fixture-model');
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
    '../OnboardingContainer': { OnboardingContainer: 'OnboardingContainer' },
    '@tauri-apps/api/core': {
      invoke: async (command, args) => {
        calls.push([command, args]);
        if (command === 'parakeet_download_model') downloaded = true;
        if (command === 'parakeet_has_available_models') return downloaded;
      },
    },
  });
  const choices = [];
  const nodes = root => Array.isArray(root) ? root.flatMap(nodes) :
    root && typeof root === 'object' ? [root, ...nodes(root.props.children)] : [];
  const button = label => nodes(hook.render(() => SetupOverviewStep({
    onChoose: (...args) => choices.push(args), finishing: false, error: null,
  }))).find(node => node.type === 'Button' && node.props.children === label);

  button('Record audio now').props.onClick();
  assert.deepEqual(choices, [['audio_only', null]]);
  assert.deepEqual(calls, []);

  button('Set up live transcription').props.onClick();
  await flush();
  assert.deepEqual(calls.map(([command]) => command), [
    'parakeet_init', 'parakeet_has_available_models', 'parakeet_download_model', 'parakeet_has_available_models',
  ]);
  button('Continue with live transcription').props.onClick();
  assert.deepEqual(choices[1], ['live', null]);
});
