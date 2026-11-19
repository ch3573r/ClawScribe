import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
const slots = props => [props?.title, props?.description, props?.children, props?.icon];
// Small stateless components (the Beta title, the status note) are expanded in place.
const expand = root => typeof root.type === 'function' ? root.type(root.props) : null;
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
  ? [root, ...slots(root.props).flatMap(nodes), ...nodes(expand(root))] : [];
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('')
  : root && typeof root === 'object' ? (expand(root) ? text(expand(root)) : slots(root.props).map(text).join('')) : '';

function setup(t, options = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const state = {
    backend: { directml: false, worker: false, directmlSessionDisabled: false },
    storedDirectml: false, storedWorker: false, audioSaving: true, attribution: false, cloud: false,
    ...options,
  };
  const { TranscriptionOptions } = loadTsModule('src/components/settings/TranscriptionOptions.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('Cloud Cpu Layers Users'),
    '@/components/ui/button': names('Button'), '@/components/ui/switch': names('Switch'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    '@/components/ui/settings-layout': names('SettingsCard SettingsRow SettingsTag'),
    '@/lib/parakeetAccel': {
      getParakeetBackendSettings: async () => ({ ...state.backend }),
      getParakeetDirectml: () => state.storedDirectml,
      getParakeetWorker: () => state.storedWorker,
      setParakeetDirectml: async enabled => { calls.push(['directml', enabled]); state.backend.directml = enabled; state.storedDirectml = enabled; },
      setParakeetWorker: async enabled => {
        calls.push(['worker', enabled]);
        if (state.workerBusy) throw new Error('busy');
        state.backend.worker = enabled; state.storedWorker = enabled;
      },
    },
    '@/lib/sourceAttribution': {
      getRecordingAudioSavingEnabled: async () => state.audioSaving,
      refreshSourceAttributionAvailability: async () => { calls.push(['refresh-attribution']); },
      setSourceAttribution: async enabled => { calls.push(['attribution', enabled]); return enabled && state.audioSaving; },
    },
    '@/lib/cloudTranscription': { setCloudTranscription: async enabled => { calls.push(['cloud', enabled]); } },
    '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ enabled: state.cloud, loaded: true }) },
    '@/hooks/useSourceAttribution': { useSourceAttribution: () => state.attribution },
    '@/components/settings/transcriptionAnchors': { TRANSCRIPTION_OPTIONS_ID: 'transcription-options' },
  });
  t.after(() => hooks.unmount());
  const render = () => hooks.render(() => TranscriptionOptions());
  const find = (type, predicate = () => true) => nodes(render()).find(node => node.type === type && predicate(node));
  const row = id => find('SettingsRow', node => node.props.htmlFor === id);
  const control = id => find('Switch', node => node.props.id === id);
  return { render, find, row, control, calls, state };
}

test('the card has the four beta options, each tagged Beta', async t => {
  const ui = setup(t);
  ui.render();
  await flush();
  const tree = ui.render();
  assert.equal(nodes(tree).find(node => node.type === 'SettingsCard').props.id, 'transcription-options');
  assert.equal(nodes(tree).find(node => node.type === 'SettingsCard').props.title, 'Options');
  const rows = nodes(tree).filter(node => node.type === 'SettingsRow');
  assert.deepEqual(rows.map(row => text(row.props.title)), [
    'Run on the GPU with DirectMLBeta', 'Run transcription in a separate processBeta',
    'Label lines as Me or ParticipantsBeta', 'Cloud transcriptionBeta',
  ]);
  assert.equal(nodes(tree).filter(node => node.type === 'SettingsTag').length, 4);
});

test('GPU and separate-process switches follow backend truth and report an unapplied change', async t => {
  const ui = setup(t);
  ui.render();
  assert.equal(ui.control('parakeet-directml').props.disabled, true, 'disabled until the backend answers');
  await flush();
  assert.equal(ui.control('parakeet-directml').props.disabled, false);
  assert.equal(ui.control('parakeet-worker').props.checked, false);

  await ui.control('parakeet-worker').props.onCheckedChange(true);
  assert.deepEqual(ui.calls.at(-1), ['worker', true]);
  assert.equal(ui.control('parakeet-worker').props.checked, true);

  // Stored preference differs from what the backend runs: say it is not active yet.
  ui.state.storedDirectml = true;
  await flush();
  ui.render();
  await flush();
  assert.match(text(ui.row('parakeet-directml')), /Not active yet/);
});

test('a refused separate-process change is explained and the switch keeps backend state', async t => {
  const ui = setup(t, { workerBusy: true });
  ui.render();
  await flush();
  await ui.control('parakeet-worker').props.onCheckedChange(true);
  assert.match(text(ui.row('parakeet-worker')), /Could not change this setting/);
  assert.equal(ui.control('parakeet-worker').props.checked, false);
});

test('a session GPU downgrade is shown on the GPU option', async t => {
  const ui = setup(t, { backend: { directml: true, worker: false, directmlSessionDisabled: true } });
  ui.render();
  await flush();
  assert.match(text(ui.row('parakeet-directml')), /turned off for this session/);
});

test('source attribution needs saved audio and says so', async t => {
  const ui = setup(t, { audioSaving: false, attribution: true });
  ui.render();
  await flush();
  assert.equal(ui.control('source-attribution').props.disabled, true);
  assert.equal(ui.control('source-attribution').props.checked, false, 'stays off while audio saving is off');
  assert.match(text(ui.row('source-attribution')), /Turn on Save audio recordings/);
  assert.ok(ui.calls.some(call => call[0] === 'refresh-attribution'));

  const enabled = setup(t);
  enabled.render();
  await flush();
  await enabled.control('source-attribution').props.onCheckedChange(true);
  assert.deepEqual(enabled.calls.at(-1), ['attribution', true]);
});

test('cloud transcription asks for consent before turning on and turns off without asking', async t => {
  const ui = setup(t);
  ui.render();
  assert.equal(ui.find('Dialog').props.open, false);
  ui.control('cloud-transcription').props.onCheckedChange(true);
  assert.equal(ui.find('Dialog').props.open, true);
  assert.deepEqual(ui.calls, [], 'nothing changes until the user agrees');

  const cancel = ui.find('Button', node => text(node) === 'Cancel');
  cancel.props.onClick();
  assert.equal(ui.find('Dialog').props.open, false);
  assert.deepEqual(ui.calls, []);

  ui.control('cloud-transcription').props.onCheckedChange(true);
  ui.find('Button', node => text(node) === 'Enable').props.onClick();
  await flush();
  assert.deepEqual(ui.calls.at(-1), ['cloud', true]);
  assert.equal(ui.find('Dialog').props.open, false);

  ui.state.cloud = true;
  ui.control('cloud-transcription').props.onCheckedChange(false);
  await flush();
  assert.deepEqual(ui.calls.at(-1), ['cloud', false]);
  assert.equal(ui.find('Dialog').props.open, false);
});
