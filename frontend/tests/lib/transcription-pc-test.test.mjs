import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
// Layout components take some content as props, so look there as well as at children.
const slots = props => [props.title, props.children, props.actions, props.description];
// Stateless function components (the action and report) are expanded in place.
const expand = root => typeof root.type === 'function' ? root.type(root.props) : null;
function nodes(root) {
  if (Array.isArray(root)) return root.flatMap(nodes);
  if (!root?.props) return [];
  const inner = expand(root);
  return [root, ...slots(root.props).flatMap(nodes), ...(inner ? nodes(inner) : [])];
}
function text(root) {
  if (Array.isArray(root)) return root.map(text).join('');
  if (!root?.props) return String(root ?? '');
  const inner = expand(root);
  return slots(root.props).map(text).join('') + (inner ? text(inner) : '');
}

function setup(t, provider, model) {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const calls = [], modes = [], run = deferred();
  const { TranscriptionPcTest } = loadTsModule('src/components/TranscriptionPcTest.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' }, 'lucide-react': { Gauge: 'Gauge' },
    '@/components/ui/settings-layout': { Meta: 'Meta', SettingsCard: 'SettingsCard', SettingsRow: 'SettingsRow' },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ setRecordingMode: async mode => modes.push(mode) }) },
    '@/lib/parakeet': { getModelDisplayName: () => 'Lightning' },
    '@/components/settings/pcTestResults': {
      savePcTestResult: (model, measured) => stored.push([model, measured.realtimeFactor]),
      usePcTestResults: () => ({}), formatMeasuredAt: () => '', formatSpeed: () => '', speedOf: () => 0,
    },
    '@tauri-apps/api/core': { invoke: (name, args) => { calls.push([name, args]); return run.promise; } },
  });
  const render = () => hooks.render(() => TranscriptionPcTest({ provider, model }));
  const button = label => nodes(render()).find(node => node.type === 'Button' && text(node) === label);
  const stored = [];
  return { render, button, calls, modes, run, stored };
}

test('hosted selections benchmark only their local model and offer audio-only after a slow or incomplete run', async t => {
  const { render, button, calls, modes, run, stored } = setup(t, 'cloud-whisper', 'cloud-model');
  const start = button('Test this PC').props.onClick;
  start(); start();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'run_transcription_pc_test');
  assert.equal(calls[0][1].provider, 'parakeet');
  assert.equal(calls[0][1].model, 'parakeet-tdt-0.6b-v3-int8');
  assert.equal(button('Testing this PC…').props.disabled, true);
  run.resolve({ engine: 'Parakeet', model: calls[0][1].model, backend: 'CPU', audioSeconds: 30,
    elapsedSeconds: 38, realtimeFactor: 38 / 30, fallbackReason: 'The worker switched to CPU.',
    samples: [{ language: 'de', audioSeconds: 15, elapsedSeconds: 20, recognized: false },
      { language: 'en', audioSeconds: 15, elapsedSeconds: 18, recognized: true }] });
  await flush();
  assert.match(text(render()), /missed speech/);
  assert.match(text(render()), /fell behind/);
  assert.match(text(render()), /38\.0 s for 30\.0 s/);
  assert.match(text(render()), /worker switched to CPU/);
  assert.deepEqual(stored, [['parakeet-tdt-0.6b-v3-int8', 38 / 30]], 'the measurement is kept under the local model that was tested');
  button('Use record now, transcribe later').props.onClick();
  assert.deepEqual(modes, ['audio_only']);
});

test('both installed Nemotron variants use the selected engine and model', async t => {
  for (const variant of ['fp16', 'int8']) {
    const model = 'nemotron-streaming-0.6b-' + variant;
    const { render, button, calls, run } = setup(t, 'nemotron', model);
    assert.match(text(render()), new RegExp('Nemotron · ' + variant.toUpperCase()));
    assert.equal(button('Test this PC').props.disabled, false);
    button('Test this PC').props.onClick();
    assert.deepEqual(structuredClone(calls), [['run_transcription_pc_test', { provider: 'nemotron', model }]]);
    run.resolve({ engine: 'Nemotron', model, backend: 'CPU', audioSeconds: 30, elapsedSeconds: 14,
      realtimeFactor: 14 / 30, samples: [] });
    await flush();
    assert.match(text(render()), /could not be confirmed/);
  }
});

test('the basic Whisper recommendation identifies the catalog default and downloads do not switch it', async t => {
  const hooks = createHookHarness(), listeners = new Map(), selections = [];
  const { DEFAULT_WHISPER_MODEL } = loadTsModule('src/constants/modelDefaults');
  const { ModelManager } = loadTsModule('src/components/WhisperModelManager.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' }, '@/components/ui/accordion': {},
    '@/components/ConfirmationModel/confirmation-modal': {},
    '@/components/ui/settings-layout': { SettingsCard: 'SettingsCard', SettingsRow: 'SettingsRow' },
    '@/components/settings/ParakeetModelTable': { TranscriptionModelTable: 'TranscriptionModelTable' },
    '@/components/settings/pcTestResults': { usePcTestResults: () => ({}), recommendedForSpeed: () => new Map() },
    '@tauri-apps/api/event': { listen: async (event, fn) => { listeners.set(event, fn); return () => {}; } },
    '@tauri-apps/api/core': { invoke: async command => command === 'whisper_get_available_models'
      ? ['base', 'small', DEFAULT_WHISPER_MODEL].map(name => ({ name, status: 'Available' })) : null },
    sonner: { toast: { success() {} } },
  });
  t.after(() => hooks.unmount());
  const render = () => hooks.render(() => ModelManager({ selectedModel: 'small', onModelSelect: name => selections.push(name) }));
  render();
  await flush();
  const table = nodes(render()).find(node => node.type === 'TranscriptionModelTable');
  assert.deepEqual(Array.from(table.props.models.filter(row => row.recommendation), row => row.name), [DEFAULT_WHISPER_MODEL]);
  listeners.get('model-download-complete')({ payload: { modelName: DEFAULT_WHISPER_MODEL } });
  assert.deepEqual(selections, []);
  table.props.onSelect(DEFAULT_WHISPER_MODEL);
  assert.deepEqual(selections, [DEFAULT_WHISPER_MODEL]);
});
