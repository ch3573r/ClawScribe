import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
const text = node => Array.isArray(node) ? node.map(text).join('') : node?.props ? text(node.props.children) : node ?? '';
const common = {
  'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
  '@/components/ui/button': names('Button'),
  '@/components/ui/progress': names('Progress'),
  '@/components/ConfirmationModel/confirmation-modal': names('ConfirmationModal'),
  '@/components/ui/dialog': names('Dialog DialogClose DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
  'lucide-react': {},
};

const catalog = {
  '@/components/settings/ParakeetModelTable': names('TranscriptionModelTable'),
  '@/components/settings/pcTestResults': { usePcTestResults: () => ({}), recommendedForSpeed: () => new Map() },
  '@/components/ui/accordion': names('Accordion AccordionContent AccordionItem AccordionTrigger'),
  '@/components/ui/settings-layout': names('SettingsCard SettingsRow'),
  '@tauri-apps/api/event': { listen: async () => () => {} },
  'framer-motion': { motion: () => 'MotionButton' },
  sonner: { toast: { success() {} } },
};

test('Whisper routes table deletion through confirmation before calling Tauri', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const deleted = [];
  const { ModelManager } = loadTsModule('src/components/WhisperModelManager.tsx', {
    ...common, ...catalog, react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'whisper_get_available_models') return [{ name: 'small', size_mb: 466, status: 'Available' }];
      if (command === 'whisper_delete_corrupted_model') deleted.push(args.modelName);
    } },
  });
  const render = () => hooks.render(() => ModelManager({ selectedModel: 'small' }));
  render();
  await flush();
  const find = type => nodes(render()).find(node => node.type === type);
  assert.ok(find('TranscriptionModelTable'));
  const trigger = {};
  find('TranscriptionModelTable').props.onDelete('small', trigger);
  assert.equal(find('ConfirmationModal').props.isOpen, true);
  assert.equal(find('ConfirmationModal').props.returnFocus.current, trigger);
  assert.deepEqual(deleted, []);
  await find('ConfirmationModal').props.onConfirm();
  assert.deepEqual(deleted, ['small']);
});

test('Nemotron normalizes backend progress for the shared table and keeps cancellation wired', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const cancelled = [];
  const { NemotronModelManager } = loadTsModule('src/components/NemotronModelManager.tsx', {
    ...common, ...catalog, react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'nemotron_get_available_models') return [{ name: 'nemotron-fp16', size_mb: 1440, status: { Downloading: { progress: 42 } } }];
      if (command === 'nemotron_cancel_download') cancelled.push(args.modelName);
    } },
    sonner: { toast: { info() {} } },
  });
  const render = () => hooks.render(() => NemotronModelManager({}));
  render();
  await flush();
  const findTable = () => nodes(render()).find(node => node.type === 'TranscriptionModelTable');
  assert.ok(findTable());
  assert.equal(findTable().props.models[0].status.Downloading, 42);
  assert.equal(findTable().props.onDelete, undefined);
  await findTable().props.onCancel('nemotron-fp16');
  assert.deepEqual(cancelled, ['nemotron-fp16']);
  assert.equal(findTable().props.models[0].status, 'Missing');
});

test('a downloaded Built-in AI model has a selection button and deletion requires confirmation with its size', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const selected = [];
  const deleted = [];
  const deletion = deferred();
  const { BuiltInModelManager } = loadTsModule('src/components/BuiltInModelManager.tsx', {
    ...common,
    react: hooks.react,
    '@/components/ui/alert': names('Alert AlertDescription'),
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'builtin_ai_list_models') return [{
        name: 'example:4b', display_name: 'Example model', status: { type: 'available' }, size_mb: 2048, context_size: 4096,
      }];
      assert.equal(command, 'builtin_ai_delete_model');
      deleted.push(args.modelName);
      await deletion.promise;
    } },
    sonner: { toast: { success() {} } },
  });
  const render = (selectedModel = '') => hooks.render(() => BuiltInModelManager({
    selectedModel, onModelSelect: name => selected.push(name),
  }));
  render();
  await flush();
  const button = (label, selectedModel) => nodes(render(selectedModel)).find(node => node.type === 'Button'
    && (node.props['aria-label'] === label || text(node) === label));
  const selection = button('Example model');
  assert.equal(selection?.props.type, 'button');
  assert.equal(selection.props['aria-pressed'], false);
  selection.props.onClick();
  assert.deepEqual(selected, ['example:4b']);
  assert.equal(button('Example model', 'example:4b').props['aria-pressed'], true);
  assert.equal(button('Delete Example model', 'example:4b'), undefined);
  const dialog = () => nodes(render()).find(node => node.type === 'ConfirmationModal');
  button('Delete Example model').props.onClick({ currentTarget: {} });
  assert.equal(dialog().props.isOpen, true);
  assert.match(dialog().props.text, /2\.0 GiB/);
  assert.deepEqual(deleted, []);
  dialog().props.onCancel();
  assert.equal(dialog().props.isOpen, false);
  assert.deepEqual(deleted, []);
  button('Delete Example model').props.onClick({ currentTarget: {} });
  const confirm = dialog().props.onConfirm;
  const pending = confirm();
  await confirm();
  assert.deepEqual(deleted, ['example:4b']);
  deletion.resolve();
  await pending;
});

test('Home modals have shared dialog titles, descriptions and controlled dismissal', () => {
  const hooks = createHookHarness();
  const { SettingsModals } = loadTsModule('src/app/_components/SettingsModal.tsx', {
    ...common,
    react: hooks.react,
    '@/components/TranscriptSettings': names('TranscriptSettings'),
    '@/components/ui/alert': names('Alert AlertDescription AlertTitle'),
    '@/contexts/ConfigContext': { useTranscriptionConfig: () => ({}) },
  });
  for (const name of ['modelSelector', 'errorAlert', 'chunkDropWarning']) {
    const closed = [];
    const tree = hooks.render(() => SettingsModals({ modals: { [name]: true }, messages: { [name]: 'Example guidance' }, onClose: key => closed.push(key) }));
    const dialog = nodes(tree).find(node => node.type === 'Dialog' && node.props.open);
    assert.ok(dialog, `${name} uses a controlled Dialog`);
    assert.ok(nodes(dialog).some(node => node.type === 'DialogTitle'));
    assert.equal(text(nodes(dialog).find(node => node.type === 'DialogDescription')), 'Example guidance');
    dialog.props.onOpenChange(false);
    assert.deepEqual(closed, [name]);
  }
});

test('audio backend help uses a focusable shared tooltip trigger', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const { AudioBackendSelector } = loadTsModule('src/components/AudioBackendSelector.tsx', {
    ...common,
    react: hooks.react,
    '@/components/ui/input': names('Input'),
    '@/components/ui/tooltip': names('Tooltip TooltipContent TooltipTrigger'),
    '@tauri-apps/api/core': { invoke: async () => [
      { id: 'coreaudio', name: 'Core Audio', description: 'Default capture method' },
      { id: 'screencapturekit', name: 'ScreenCaptureKit', description: 'Alternative capture method' },
    ] },
  });
  const render = () => hooks.render(() => AudioBackendSelector({ currentBackend: 'coreaudio' }));
  render();
  await flush();
  const tooltip = nodes(render()).find(node => node.type === 'Tooltip');
  assert.ok(tooltip);
  const trigger = nodes(tooltip).find(node => node.type === 'TooltipTrigger');
  assert.equal(trigger.props.asChild, true);
  assert.equal(trigger.props.children.type, 'Button');
  assert.equal(trigger.props.children.props['aria-label'], 'Audio capture methods');
  assert.equal(trigger.props.children.props.disabled, undefined);
  assert.match(text(nodes(tooltip).find(node => node.type === 'TooltipContent')), /Alternative capture method/);
});
