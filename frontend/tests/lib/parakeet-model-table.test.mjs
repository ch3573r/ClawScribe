import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
const slots = props => [props?.title, props?.description, props?.actions, props?.children, props?.testAction];
// Stateless function components (cells, actions) are expanded in place.
const expand = root => typeof root.type === 'function' ? root.type(root.props) : null;
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
  ? [root, ...slots(root.props).flatMap(nodes), ...nodes(expand(root))] : [];
const text = root => typeof root === 'string' || typeof root === 'number' ? String(root) : Array.isArray(root) ? root.map(text).join('')
  : root && typeof root === 'object' ? (expand(root) ? text(expand(root)) : slots(root.props).map(text).join('')) : '';

const layout = names('Meta SettingsCard SettingsFooter SettingsRow SettingsTag');
const common = {
  'react/jsx-runtime': { jsx, jsxs: jsx },
  'lucide-react': names('Download MoreHorizontal'),
  '@/components/ui/button': names('Button'),
  '@/components/ui/dropdown-menu': names('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuTrigger'),
  '@/components/ui/progress': names('Progress'),
  '@/components/ui/tooltip': names('Tooltip TooltipContent TooltipTrigger'),
  '@/components/ui/settings-layout': { ...layout, StatusText: ({ tone, children }) => ({ type: 'StatusText', props: { tone, children } }) },
};

const model = (name, status, size = 670) => ({ name, path: '', size_mb: size, accuracy: 'High', speed: 'Fast', status, quantization: 'Int8' });
const record = (factor, extra = {}) => ({ realtimeFactor: factor, measuredAt: 1, backend: 'CPU', missedSpeech: false, ...extra });

function table(overrides = {}) {
  const { ParakeetModelTable } = loadTsModule('src/components/settings/ParakeetModelTable.tsx', common);
  const calls = [];
  const props = {
    models: [
      model('parakeet-tdt-0.6b-ultra-int8', 'Available', 671),
      model('parakeet-tdt-0.6b-v3-int8', 'Available', 670),
      model('parakeet-tdt-0.6b-v3-smoothquant-int8', 'Missing', 813),
      model('parakeet-tdt-0.6b-ultra-fp16', { Downloading: 40 }, 1276),
      model('parakeet-tdt-0.6b-v2-int8', { Error: 'Network dropped' }, 661),
    ],
    loading: false, error: null, selectedModel: 'parakeet-tdt-0.6b-ultra-int8', results: {},
    onSelect: name => calls.push(['select', name]), onDownload: name => calls.push(['download', name]),
    onCancel: name => calls.push(['cancel', name]), onDelete: name => calls.push(['delete', name]),
    onRetryLoad: () => calls.push(['retry']),
    ...overrides,
  };
  const tree = ParakeetModelTable(props);
  const rows = nodes(tree).filter(node => node.type === 'tr' && nodes(node.props.children).some(n => n.type === 'td'));
  return { tree, rows, calls, props };
}
const rowFor = (rows, name) => rows.find(row => text(row).includes(name));
const button = (row, label) => nodes(row).find(node => node.type === 'Button' && node.props['aria-label'] === label);

test('one table, the recommended model first, tags beside the name and no emoji', () => {
  const { rows, tree } = table();
  const order = ['Lightning', 'Parakeet Ultra', 'SmoothQuant', 'Parakeet Ultra fp16', 'Compact'];
  rows.forEach((row, index) => assert.ok(text(nodes(row).find(n => n.type === 'td')).startsWith(order[index]), `row ${index} is ${order[index]}`));
  assert.equal(rows.length, 5);
  assert.match(text(rowFor(rows, 'Lightning')), /Recommended/);
  assert.match(text(rowFor(rows, 'SmoothQuant')), /Experimental/);
  assert.equal(nodes(tree).find(node => node.type === 'SettingsCard').props.title, 'Models');
  assert.doesNotMatch(text(tree), /[\u{1F300}-\u{1FAFF}☀-➿]/u);
  assert.match(text(tree), /1,276 MB/);
  assert.match(text(rowFor(rows, 'Parakeet Ultra fp16')), /Parakeet Ultra fp16/);
});

test('every state keeps the action that applies to it', () => {
  const { rows, calls } = table();
  const inUse = rowFor(rows, 'German and English accuracy');
  assert.match(text(rowFor(rows, 'German and English accuracy')), /In use/);
  assert.equal(button(inUse, 'Use Parakeet Ultra'), undefined, 'the model in use has no Use button');
  assert.match(inUse.props.className, /bg-primary/, 'the model in use is highlighted');

  const ready = rowFor(rows, 'Everyday meetings');
  assert.match(text(ready), /Ready/);
  button(ready, 'Use Lightning').props.onClick();
  const more = nodes(ready).find(node => node.type === 'Button' && node.props['aria-label'] === 'More actions for Lightning');
  assert.ok(more, 'rare actions sit in an overflow menu');
  nodes(ready).find(node => node.type === 'DropdownMenuItem').props.onSelect();

  const missing = rowFor(rows, 'SmoothQuant');
  assert.match(text(missing), /Not downloaded/);
  assert.equal(button(missing, 'Download SmoothQuant').props.variant, 'outline', 'Download is a quiet button');
  button(missing, 'Download SmoothQuant').props.onClick();

  const downloading = rowFor(rows, 'Parakeet Ultra fp16');
  assert.match(text(downloading), /Downloading 40%/);
  assert.equal(nodes(downloading).find(node => node.type === 'Progress').props.value, 40);
  assert.match(text(downloading), /510 MB \/ 1\.3GB|510MB \/ 1\.3GB/);
  button(downloading, 'Cancel download of Parakeet Ultra fp16').props.onClick();

  const failed = rowFor(rows, 'Compact');
  assert.match(text(failed), /Download failed/);
  assert.match(text(failed), /Network dropped/);
  button(failed, 'Retry download of Compact').props.onClick();

  assert.deepEqual(calls.map(call => call.join(':')), [
    'select:parakeet-tdt-0.6b-v3-int8', 'delete:parakeet-tdt-0.6b-v3-int8', 'download:parakeet-tdt-0.6b-v3-smoothquant-int8',
    'cancel:parakeet-tdt-0.6b-ultra-fp16', 'download:parakeet-tdt-0.6b-v2-int8',
  ]);
});

test('an incomplete download offers delete and download again', () => {
  const { rows, calls } = table({ models: [model('parakeet-tdt-0.6b-v3-int8', { Corrupted: { file_size: 1, expected_min_size: 2 } })] });
  assert.match(text(rows[0]), /Incomplete download/);
  button(rows[0], 'Delete Lightning').props.onClick();
  button(rows[0], 'Download Lightning again').props.onClick();
  assert.deepEqual(calls.map(call => call[0]), ['delete', 'download']);
});

test('speed shows measured results only, scaled to the fastest measured model', () => {
  const results = {
    'parakeet-tdt-0.6b-v3-int8': record(1 / 22),
    'parakeet-tdt-0.6b-ultra-int8': record(1 / 11, { missedSpeech: true }),
  };
  const { rows } = table({ results });
  const fast = rowFor(rows, 'Everyday meetings');
  const slow = rowFor(rows, 'German and English accuracy');
  assert.match(text(fast), /22× real time/);
  assert.match(text(slow), /11× real time/);
  assert.match(text(slow), /Missed speech/);
  assert.match(text(rowFor(rows, 'Long recordings')), /Not tested/);
  const width = row => nodes(row).find(node => node.type === 'span' && node.props.style?.width)?.props.style.width;
  assert.equal(width(fast), '8rem', 'the fastest model gets the full bar');
  assert.equal(width(slow), '4rem');
  assert.doesNotMatch(text(table().rows.map(row => text(row)).join(' ')), /real time/, 'catalogue descriptions are never shown as speed');
});

test('loading, failure, empty and unsupported-selection states', () => {
  assert.match(text(table({ loading: true, models: [] }).tree), /Loading models/);
  const failed = table({ error: 'Parakeet is unavailable', models: [] });
  assert.match(text(failed.tree), /Parakeet is unavailable/);
  nodes(failed.tree).find(node => node.type === 'Button' && text(node) === 'Try again').props.onClick();
  assert.deepEqual(failed.calls, [['retry']]);
  assert.match(text(table({ models: [] }).tree), /No models available/);
  assert.match(text(table({ selectedModel: 'parakeet-tdt-0.6b-gone' }).tree), /no longer supported/);
  assert.doesNotMatch(text(table().tree), /no longer supported/);
});

test('the manager loads models, selects with auto-save, and clears the selection when it is deleted', async t => {
  const hooks = createHookHarness();
  const calls = [], selected = [];
  let models = [model('parakeet-tdt-0.6b-v3-int8', 'Available'), model('parakeet-tdt-0.6b-ultra-int8', 'Available')];
  const listeners = new Map();
  const { ParakeetModelManager } = loadTsModule('src/components/ParakeetModelManager.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/event': { listen: async (name, callback) => { listeners.set(name, callback); return () => {}; } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args && JSON.parse(JSON.stringify(args))]);
      if (command === 'parakeet_get_available_models') return models;
      if (command === 'parakeet_delete_corrupted_model') { models = models.filter(m => m.name !== args.modelName); return 'ok'; }
      return undefined;
    } },
    sonner: { toast: { success() {}, error() {}, info() {} } },
    '@/components/settings/ParakeetModelTable': { ParakeetModelTable: 'ParakeetModelTable' },
    '@/components/settings/pcTestResults': { usePcTestResults: () => ({}) },
    '@/components/TranscriptionPcTest': {
      PcTestAction: 'PcTestAction', PcTestReport: 'PcTestReport',
      useTranscriptionPcTest: arg => ({ target: arg }),
    },
  });
  t.after(() => hooks.unmount());
  const render = () => hooks.render(() => ParakeetModelManager({
    selectedModel: 'parakeet-tdt-0.6b-ultra-int8', autoSave: true, onModelSelect: name => selected.push(name),
    pcTest: { provider: 'parakeet', model: 'parakeet-tdt-0.6b-ultra-int8' },
  }));
  render();
  await flush();
  let element = render().props.children;
  assert.equal(element.props.models.length, 2);
  assert.equal(element.props.loading, false);
  assert.equal(element.props.testAction.type, 'PcTestAction');

  await element.props.onSelect('parakeet-tdt-0.6b-v3-int8');
  assert.deepEqual(selected, ['parakeet-tdt-0.6b-v3-int8']);
  assert.deepEqual(calls.at(-1), ['api_save_transcript_config', { provider: 'parakeet', model: 'parakeet-tdt-0.6b-v3-int8', apiKey: null }]);

  await element.props.onDelete('parakeet-tdt-0.6b-ultra-int8');
  await flush();
  assert.deepEqual(selected.at(-1), '', 'deleting the model in use clears the selection');
  assert.equal(render().props.children.props.models.length, 1);
});
