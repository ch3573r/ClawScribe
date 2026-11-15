import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
  ? [root, ...nodes(root.props?.children)] : [];
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('') : text(root?.props?.children ?? '');

function setup(invoke) {
  const hooks = createHookHarness();
  const { CustomVocabularySettings } = loadTsModule('src/components/CustomVocabularySettings.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke }, 'lucide-react': { X: 'X' },
    '@/components/ui/button': { Button: 'Button' }, '@/components/ui/input': { Input: 'Input' },
    '@/components/ui/label': { Label: 'Label' }, '@/components/ui/page-section': { PageSection: 'PageSection' },
  });
  const render = () => hooks.render(() => CustomVocabularySettings());
  const find = (type, predicate = () => true) => nodes(render()).find(node => node.type === type && predicate(node));
  return { hooks, render, find };
}

test('vocabulary retries loading, then adds once on repeated Enter while saving', async t => {
  const read = deferred(), save = deferred(), calls = [];
  let reads = 0;
  const ui = setup((command, args) => {
    calls.push({ command, args });
    if (command === 'get_custom_vocabulary') return ++reads === 1 ? read.promise : Promise.resolve([]);
    return save.promise;
  });
  t.after(() => ui.hooks.unmount());
  assert.match(text(ui.render()), /Loading custom vocabulary/);
  assert.equal(ui.find('Input'), undefined);
  read.reject(new Error('store unavailable'));
  await flush();
  assert.match(text(ui.render()), /Could not load custom vocabulary/);
  await ui.find('Button', n => text(n) === 'Retry').props.onClick();
  await flush();
  assert.match(text(ui.render()), /No words yet/);
  ui.find('Input').props.onChange({ target: { value: '  ChargeBee  ' } });
  const form = ui.find('form');
  form.props.onSubmit({ preventDefault() {} });
  form.props.onSubmit({ preventDefault() {} });
  assert.equal(calls.filter(c => c.command === 'set_custom_vocabulary').length, 1);
  assert.deepEqual(Array.from(calls.at(-1).args.words), ['ChargeBee']);
  assert.equal(ui.find('Input').props.disabled, true);
  save.resolve(['ChargeBee']);
  await flush();
  assert.match(text(ui.render()), /ChargeBee/);
  assert.equal(ui.find('Input').props.value, '');
  assert.equal(ui.find('Button', n => n.props['aria-label'] === 'Remove ChargeBee').props.disabled, false);
});

test('failed vocabulary removal keeps the word visible and can be retried', async t => {
  const removal = deferred();
  let writes = 0;
  const ui = setup(command => command === 'get_custom_vocabulary' ? Promise.resolve(['Rismondo'])
    : ++writes === 1 ? removal.promise : Promise.resolve([]));
  t.after(() => ui.hooks.unmount());
  ui.render();
  await flush();
  ui.find('Button', n => n.props['aria-label'] === 'Remove Rismondo').props.onClick();
  removal.reject(new Error('disk full'));
  await flush();
  assert.match(text(ui.render()), /Could not save custom vocabulary/);
  assert.ok(ui.find('Button', n => n.props['aria-label'] === 'Remove Rismondo'));
  ui.find('Button', n => n.props['aria-label'] === 'Remove Rismondo').props.onClick();
  await flush();
  assert.match(text(ui.render()), /No words yet/);
  assert.equal(ui.find('Button', n => n.props['aria-label'] === 'Remove Rismondo'), undefined);
});

test('duplicate, overlong, and punctuation-only entries explain the problem without saving', async t => {
  let writes = 0;
  const ui = setup(command => {
    if (command !== 'get_custom_vocabulary') writes++;
    return Promise.resolve(['Müller']);
  });
  t.after(() => ui.hooks.unmount());
  ui.render();
  await flush();
  ui.find('Input').props.onChange({ target: { value: ' MÜLLER ' } });
  ui.find('form').props.onSubmit({ preventDefault() {} });
  assert.match(text(ui.render()), /already in your vocabulary/);
  ui.find('Input').props.onChange({ target: { value: 'ü'.repeat(65) } });
  ui.find('form').props.onSubmit({ preventDefault() {} });
  assert.match(text(ui.render()), /at most 64 characters/);
  ui.find('Input').props.onChange({ target: { value: '+++' } });
  ui.find('form').props.onSubmit({ preventDefault() {} });
  assert.match(text(ui.render()), /at least one letter or number/);
  assert.equal(writes, 0);
});

test('native vocabulary read errors remain actionable in settings', async t => {
  let reads = 0;
  const writes = [];
  const ui = setup((command, args) => {
    if (command === 'get_custom_vocabulary') return ++reads === 1
      ? Promise.reject('Custom vocabulary is unavailable. Retry.') : Promise.resolve([]);
    writes.push(args.words);
    return Promise.resolve(args.words);
  });
  t.after(() => ui.hooks.unmount());
  ui.render();
  await flush();
  assert.match(text(ui.render()), /Custom vocabulary is unavailable. Retry./);
  assert.ok(ui.find('Button', n => text(n) === 'Retry'));
  assert.equal(ui.find('Input'), undefined);
  ui.find('Button', n => text(n) === 'Retry').props.onClick();
  await flush();
  assert.equal(reads, 2);
  assert.equal(ui.find('p', n => n.props.role === 'alert'), undefined);
  assert.equal(ui.find('Input').props.disabled, false);
  ui.find('Input').props.onChange({ target: { value: 'Müller' } });
  ui.find('form').props.onSubmit({ preventDefault() {} });
  await flush();
  assert.deepEqual(writes.map(words => Array.from(words)), [['Müller']]);
  assert.ok(ui.find('Button', n => n.props['aria-label'] === 'Remove Müller'));
});

test('an empty recovered vocabulary can be replaced from settings', async t => {
  const writes = [];
  const ui = setup((command, args) => {
    if (command === 'get_custom_vocabulary') return Promise.resolve([]);
    writes.push(args.words);
    return Promise.resolve(args.words);
  });
  t.after(() => ui.hooks.unmount());
  ui.render();
  await flush();
  ui.find('Input').props.onChange({ target: { value: 'Zoë' } });
  ui.find('form').props.onSubmit({ preventDefault() {} });
  await flush();
  assert.deepEqual(writes.map(words => Array.from(words)), [['Zoë']]);
  assert.match(text(ui.render()), /Word added/);
});
